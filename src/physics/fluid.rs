//! Fluids.
//!
//! `WaterSurface` is a body of water as a height field obeying the wave equation: disturbances
//! spread as rings at the speed of shallow-water waves, interfere, reflect off the edges and
//! die away. Dynamic bodies in it float (buoyancy from how much of them is under the surface),
//! are slowed by the water, and make waves as they move.
//!
//! `ParticleFluid` is water as particles (position-based fluids, Macklin & Müller 2013): each
//! step every particle is moved so the fluid keeps its density, which gives it cohesion and
//! incompressibility, with a little viscosity. Particles collide with every collider, and
//! those that fall into a `WaterSurface` join it, making waves where they land. Emitters make
//! jets and pours. It draws as a mesh of droplets stretched along their motion, so a fast
//! stream looks continuous and breaks into drops as it slows and spreads.

use std::collections::HashMap;

use glam::{IVec3, Vec2, Vec3};

use super::{collide, shape::{Iso, Shape}, BodyKind, Collider, PhysicsWorld, RigidBody};
use crate::{
    assets::{Assets, Handle},
    ecs::{Component, Query, Res, ResMut},
    render::{Mesh, Vertex},
    time::FixedTime,
    transform::Transform,
};

const WATER_DENSITY: f32 = 1000.0;

/// A body of water, `cells` across at `cell` metres each, centred on its entity's translation
/// (which is the water's resting level). Cells outside `inside` are dry land or walls: waves
/// reflect off them.
pub struct WaterSurface {
    pub size: (usize, usize),
    pub cell: f32,
    /// Speed of the waves, metres a second: sqrt(g × depth) for shallow water.
    pub wave_speed: f32,
    /// How fast waves die away.
    pub damping: f32,
    /// How strongly floating and moving bodies are pushed and make waves.
    pub coupling: f32,
    pub(crate) height: Vec<f32>,
    velocity: Vec<f32>,
    inside: Vec<bool>,
    center: Vec3,
    /// A mesh kept matching the surface, if set (see `WaterSurface::mesh`).
    pub mesh: Option<Handle<Mesh>>,
}

impl Component for WaterSurface {}

impl WaterSurface {
    /// A rectangle of water `width` by `depth` metres.
    pub fn new(width: f32, depth: f32, cell: f32) -> Self {
        let size = ((width / cell).ceil() as usize + 1, (depth / cell).ceil() as usize + 1);
        Self {
            size,
            cell,
            wave_speed: 2.0,
            damping: 0.4,
            coupling: 1.0,
            height: vec![0.0; size.0 * size.1],
            velocity: vec![0.0; size.0 * size.1],
            inside: vec![true; size.0 * size.1],
            center: Vec3::ZERO,
            mesh: None,
        }
    }

    /// Marks which cells hold water, by their offset from the centre.
    pub fn with_shape(mut self, inside: impl Fn(Vec2) -> bool) -> Self {
        for j in 0..self.size.1 {
            for i in 0..self.size.0 {
                self.inside[j * self.size.0 + i] = inside(self.offset(i, j));
            }
        }
        self
    }

    fn offset(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(i as f32 - (self.size.0 - 1) as f32 * 0.5, j as f32 - (self.size.1 - 1) as f32 * 0.5) * self.cell
    }

    fn cell_at(&self, p: Vec3) -> Option<(f32, f32)> {
        let local = Vec2::new(p.x - self.center.x, p.z - self.center.z) / self.cell
            + Vec2::new((self.size.0 - 1) as f32 * 0.5, (self.size.1 - 1) as f32 * 0.5);
        (local.x >= 0.0 && local.y >= 0.0 && local.x <= (self.size.0 - 1) as f32 && local.y <= (self.size.1 - 1) as f32).then_some((local.x, local.y))
    }

    /// Whether a point (world x/z) lies over the water.
    pub fn contains(&self, p: Vec3) -> bool {
        self.cell_at(p).is_some_and(|(x, y)| self.inside[(y.round() as usize) * self.size.0 + x.round() as usize])
    }

    /// The water's surface height at a world point (its resting level off the water).
    pub fn height_at(&self, p: Vec3) -> f32 {
        let Some((x, y)) = self.cell_at(p) else { return self.center.y };
        let (i, j) = ((x as usize).min(self.size.0 - 2), (y as usize).min(self.size.1 - 2));
        let (fx, fy) = (x - i as f32, y - j as f32);
        let h = |a: usize, b: usize| self.height[b * self.size.0 + a];
        let top = h(i, j) * (1.0 - fx) + h(i + 1, j) * fx;
        let bottom = h(i, j + 1) * (1.0 - fx) + h(i + 1, j + 1) * fx;
        self.center.y + top * (1.0 - fy) + bottom * fy
    }

    /// The surface's normal at a world point.
    pub fn normal_at(&self, p: Vec3) -> Vec3 {
        let e = self.cell;
        let dx = self.height_at(p + Vec3::X * e) - self.height_at(p - Vec3::X * e);
        let dz = self.height_at(p + Vec3::Z * e) - self.height_at(p - Vec3::Z * e);
        Vec3::new(-dx, 2.0 * e, -dz).normalize()
    }

    /// Pushes the surface down (negative `speed`: something falling in) or up, in metres a
    /// second, over a round patch.
    pub fn disturb(&mut self, p: Vec3, radius: f32, speed: f32) {
        let Some((x, y)) = self.cell_at(p) else { return };
        let r = (radius / self.cell).ceil() as isize;
        for dj in -r..=r {
            for di in -r..=r {
                let (i, j) = (x.round() as isize + di, y.round() as isize + dj);
                if i < 0 || j < 0 || i >= self.size.0 as isize || j >= self.size.1 as isize {
                    continue;
                }
                let k = j as usize * self.size.0 + i as usize;
                let d = Vec2::new(i as f32 - x, j as f32 - y).length() * self.cell / radius.max(1e-4);
                if d < 1.0 && self.inside[k] {
                    self.velocity[k] += speed * (1.0 - d * d);
                }
            }
        }
    }

    /// Steps the wave equation by `dt`.
    pub fn step(&mut self, dt: f32) {
        let (w, h) = self.size;
        let steps = ((self.wave_speed * dt / (self.cell * 0.5)).ceil() as usize).max(1);
        let sub = dt / steps as f32;
        let k = self.wave_speed * self.wave_speed / (self.cell * self.cell);
        for _ in 0..steps {
            for j in 1..h - 1 {
                for i in 1..w - 1 {
                    let c = j * w + i;
                    if !self.inside[c] {
                        continue;
                    }
                    // Walls reflect: a dry neighbour counts as level with this cell.
                    let side = |n: usize| if self.inside[n] { self.height[n] } else { self.height[c] };
                    let laplacian = side(c - 1) + side(c + 1) + side(c - w) + side(c + w) - 4.0 * self.height[c];
                    self.velocity[c] += (laplacian * k - self.velocity[c] * self.damping) * sub;
                }
            }
            for c in 0..w * h {
                if self.inside[c] {
                    self.height[c] = (self.height[c] + self.velocity[c] * sub).clamp(-self.cell * 2.0, self.cell * 2.0);
                } else {
                    self.height[c] = 0.0;
                }
            }
        }
    }

    /// A grid mesh of the surface at rest, in world space; keep its handle in `mesh` to have it
    /// follow the waves. Cells over dry land are left out.
    pub fn mesh(&self, center: Vec3) -> Mesh {
        let (w, h) = self.size;
        let mut mesh = Mesh::default();
        for j in 0..h {
            for i in 0..w {
                let o = self.offset(i, j);
                let p = center + Vec3::new(o.x, 0.0, o.y);
                mesh.vertices.push(Vertex::new(p, Vec3::Y, Vec2::new(p.x, p.z) / 4.0));
            }
        }
        for j in 0..h - 1 {
            for i in 0..w - 1 {
                let a = (j * w + i) as u32;
                let (b, c, d) = (a + 1, a + w as u32, a + w as u32 + 1);
                if [a, b, c, d].iter().any(|&k| self.inside[k as usize]) {
                    mesh.indices.extend([a, c, b, b, c, d]);
                }
            }
        }
        mesh
    }
}

/// Makes a `ParticleFluid` jet: particles leave `origin` at `velocity`, `rate` a second,
/// spread by `spread` (metres a second, at random) and a nozzle `radius` wide.
#[derive(Clone, Copy, Debug)]
pub struct Emitter {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub rate: f32,
    pub spread: f32,
    pub radius: f32,
    /// Varies the flow, 0 for steady: gouts and slackening like a real spout.
    pub pulse: f32,
}

/// Water as particles. Each particle stands for a blob `spacing` metres across.
pub struct ParticleFluid {
    pub spacing: f32,
    pub max_particles: usize,
    /// Seconds a particle lives, at most.
    pub lifetime: f32,
    /// Stickiness between particles (XSPH), 0 to about 0.1.
    pub viscosity: f32,
    pub emitters: Vec<Emitter>,
    /// Particles that land in a `WaterSurface` vanish into it, making waves.
    pub join_surfaces: bool,
    /// Surface tension, 0 to about 0.5: how strongly thinned-out water pulls itself back
    /// together, so a jet holds as a rope and breaks into drops rather than fanning into spray.
    pub cohesion: f32,
    /// How big each drawn droplet is, as a share of `spacing` (its radius): larger merges a
    /// jet into a continuous stream.
    pub drop_size: f32,
    /// A mesh of droplets kept up to date, if set.
    pub mesh: Option<Handle<Mesh>>,
    pub positions: Vec<Vec3>,
    pub velocities: Vec<Vec3>,
    ages: Vec<f32>,
    owed: Vec<f32>,
    clock: f32,
    rng: u32,
}

impl Component for ParticleFluid {}

impl ParticleFluid {
    pub fn new(spacing: f32) -> Self {
        Self {
            spacing,
            max_particles: 4000,
            lifetime: 6.0,
            viscosity: 0.02,
            emitters: Vec::new(),
            join_surfaces: true,
            drop_size: 0.55,
            cohesion: 0.0,
            mesh: None,
            positions: Vec::new(),
            velocities: Vec::new(),
            ages: Vec::new(),
            owed: Vec::new(),
            clock: 0.0,
            rng: 0x2545_f491,
        }
    }

    pub fn with_emitter(mut self, e: Emitter) -> Self {
        self.emitters.push(e);
        self
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }

    /// Adds a particle, if there's room.
    pub fn spawn(&mut self, p: Vec3, v: Vec3) {
        if self.positions.len() < self.max_particles {
            self.positions.push(p);
            self.velocities.push(v);
            self.ages.push(0.0);
        }
    }

    fn emit(&mut self, dt: f32) {
        self.clock += dt;
        self.owed.resize(self.emitters.len(), 0.0);
        for k in 0..self.emitters.len() {
            let e = self.emitters[k];
            let pulse = 1.0 + e.pulse * ((self.clock * 5.3 + k as f32 * 1.7).sin() * 0.6 + (self.clock * 13.1 + k as f32).sin() * 0.4);
            self.owed[k] += e.rate * pulse.max(0.0) * dt;
            while self.owed[k] >= 1.0 {
                self.owed[k] -= 1.0;
                let (a, r) = (self.random() * std::f32::consts::TAU, self.random().sqrt() * e.radius);
                let dir = e.velocity.normalize_or(Vec3::Y);
                let side = dir.any_orthonormal_vector();
                let up = dir.cross(side);
                let offset = (side * a.cos() + up * a.sin()) * r;
                let jitter = Vec3::new(self.random() - 0.5, self.random() - 0.5, self.random() - 0.5) * e.spread;
                // Spread along the step so a fast jet isn't a string of beads.
                let along = self.random();
                let v = e.velocity + jitter;
                self.spawn(e.origin + offset + v * dt * along, v);
            }
        }
    }

    /// Steps the fluid: emission, gravity, keeping density, collisions.
    pub fn step(&mut self, dt: f32, gravity: Vec3, world: Option<&PhysicsWorld>, surfaces: &mut [&mut WaterSurface]) {
        self.emit(dt);
        let n = self.positions.len();
        if n == 0 {
            return;
        }
        let h = self.spacing * 2.0;
        let h2 = h * h;
        // Kernel constants (Müller et al. 2003): poly6 for density, spiky for its gradient.
        let poly6 = 315.0 / (64.0 * std::f32::consts::PI * h.powi(9));
        let spiky = -45.0 / (std::f32::consts::PI * h.powi(6));
        let rest = {
            // The density of particles packed on a grid of `spacing`, so a still blob keeps
            // its size.
            let mut d = 0.0;
            let r = (h / self.spacing).ceil() as i32;
            for x in -r..=r {
                for y in -r..=r {
                    for z in -r..=r {
                        let q = (Vec3::new(x as f32, y as f32, z as f32) * self.spacing).length_squared();
                        if q < h2 {
                            d += poly6 * (h2 - q).powi(3);
                        }
                    }
                }
            }
            d
        };
        let old = self.positions.clone();
        let mut predicted: Vec<Vec3> = (0..n).map(|i| {
            self.velocities[i] += gravity * dt;
            self.positions[i] + self.velocities[i] * dt
        }).collect();

        // Neighbours, by a hash grid of cell `h`.
        let key = |p: Vec3| (p / h).floor().as_ivec3();
        let mut grid: HashMap<IVec3, Vec<u32>> = HashMap::new();
        for (i, p) in predicted.iter().enumerate() {
            grid.entry(key(*p)).or_default().push(i as u32);
        }
        let neighbours: Vec<Vec<u32>> = predicted
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let c = key(*p);
                let mut out = Vec::new();
                for dz in -1..=1 {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if let Some(cell) = grid.get(&(c + IVec3::new(dx, dy, dz))) {
                                out.extend(cell.iter().copied().filter(|&j| j as usize != i && (predicted[j as usize] - *p).length_squared() < h2));
                            }
                        }
                    }
                }
                out
            })
            .collect();

        // Keep the density: a few passes of the position-based fluid constraint.
        let mut lambda = vec![0.0f32; n];
        for _ in 0..3 {
            for i in 0..n {
                let mut density = poly6 * h2.powi(3);
                let mut grad_sum = 0.0;
                let mut grad_i = Vec3::ZERO;
                for &j in &neighbours[i] {
                    let r = predicted[i] - predicted[j as usize];
                    let d2 = r.length_squared();
                    density += poly6 * (h2 - d2).powi(3);
                    let d = d2.sqrt();
                    if d > 1e-6 {
                        let g = r / d * spiky * (h - d).powi(2) / rest;
                        grad_sum += g.length_squared();
                        grad_i += g;
                    }
                }
                grad_sum += grad_i.length_squared();
                // Resist compression fully, and thinning only as much as surface tension does:
                // with none, free spray and thin streams stay loose.
                let c = density / rest - 1.0;
                let c = if c > 0.0 { c } else { c * self.cohesion };
                lambda[i] = -c / (grad_sum + 100.0);
            }
            let corr_q = poly6 * (h2 - (0.2 * h) * (0.2 * h)).powi(3);
            let deltas: Vec<Vec3> = (0..n)
                .map(|i| {
                    let mut dp = Vec3::ZERO;
                    for &j in &neighbours[i] {
                        let r = predicted[i] - predicted[j as usize];
                        let d = r.length();
                        if d > 1e-6 {
                            // A small repulsion (tensile correction) keeps particles from clumping.
                            let w = poly6 * (h2 - d * d).powi(3);
                            let s_corr = -0.001 * (w / corr_q).powi(4);
                            dp += r / d * spiky * (h - d).powi(2) * (lambda[i] + lambda[j as usize] + s_corr);
                        }
                    }
                    dp / rest
                })
                .collect();
            // Never more than a fraction of a particle per pass: where many are packed in at
            // once (a nozzle), the correction would otherwise overshoot and scatter them.
            let most = self.spacing * 0.3;
            for i in 0..n {
                predicted[i] += deltas[i].clamp_length_max(most);
            }
        }

        // Collide with the world.
        if let Some(world) = world {
            let drop = Shape::sphere(self.spacing * 0.5);
            let mut scratch = Vec::new();
            for p in predicted.iter_mut() {
                let here = Iso::new(*p, glam::Quat::IDENTITY);
                let bounds = drop.aabb(&here);
                for c in &world.colliders {
                    if c.sensor || !c.aabb.overlaps(&bounds) {
                        continue;
                    }
                    scratch.clear();
                    collide::contact(&drop, &here, &c.shape, &c.iso, &mut scratch);
                    for m in &scratch {
                        let depth = m.points.iter().map(|q| q.1).fold(0.0f32, f32::max);
                        if depth > 0.0 {
                            *p -= m.normal * depth;
                        }
                    }
                }
            }
        }

        // New velocities, a little viscosity, ageing.
        for i in 0..n {
            self.velocities[i] = ((predicted[i] - old[i]) / dt).clamp_length_max(40.0);
        }
        if self.viscosity > 0.0 {
            let v = self.velocities.clone();
            for i in 0..n {
                let mut sum = Vec3::ZERO;
                for &j in &neighbours[i] {
                    let d2 = (predicted[i] - predicted[j as usize]).length_squared();
                    sum += (v[j as usize] - v[i]) * poly6 * (h2 - d2).powi(3) / rest;
                }
                self.velocities[i] += sum * self.viscosity;
            }
        }
        self.positions = predicted;
        for a in &mut self.ages {
            *a += dt;
        }

        // Particles falling into a body of water join it, pushing its surface down.
        let mut i = 0;
        while i < self.positions.len() {
            let p = self.positions[i];
            let mut gone = self.ages[i] > self.lifetime || p.y < -100.0 || !p.is_finite();
            if self.join_surfaces && !gone {
                for s in surfaces.iter_mut() {
                    if s.contains(p) && p.y < s.height_at(p) {
                        let v = self.velocities[i];
                        s.disturb(p, self.spacing * 1.5, v.y.min(0.0) * 0.25 * s.coupling);
                        gone = true;
                        break;
                    }
                }
            }
            if gone {
                self.positions.swap_remove(i);
                self.velocities.swap_remove(i);
                self.ages.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Droplets: each particle a small ellipsoid stretched along its motion.
    pub fn droplets(&self, stretch: f32) -> Mesh {
        let mut mesh = Mesh::default();
        // A small sphere: a pole at each end and three rings of eight between.
        const SECTORS: u32 = 8;
        const RINGS: u32 = 3;
        let mut base = vec![Vec3::Y];
        for i in 1..=RINGS {
            let theta = std::f32::consts::PI * i as f32 / (RINGS + 1) as f32;
            for j in 0..SECTORS {
                let phi = std::f32::consts::TAU * j as f32 / SECTORS as f32;
                base.push(Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()));
            }
        }
        base.push(Vec3::NEG_Y);
        let bottom = base.len() as u32 - 1;
        let ring = |i: u32, j: u32| 1 + (i - 1) * SECTORS + j % SECTORS;
        let mut faces: Vec<[u32; 3]> = Vec::new();
        for j in 0..SECTORS {
            faces.push([0, ring(1, j + 1), ring(1, j)]);
            faces.push([bottom, ring(RINGS, j), ring(RINGS, j + 1)]);
            for i in 1..RINGS {
                let (a, b, c, d) = (ring(i, j), ring(i, j + 1), ring(i + 1, j), ring(i + 1, j + 1));
                faces.push([a, b, c]);
                faces.push([b, d, c]);
            }
        }
        // Wind every face outward (counter-clockwise seen from outside).
        for f in &mut faces {
            let (pa, pb, pc) = (base[f[0] as usize], base[f[1] as usize], base[f[2] as usize]);
            if (pb - pa).cross(pc - pa).dot(pa + pb + pc) < 0.0 {
                f.swap(1, 2);
            }
        }
        let r = self.spacing * self.drop_size;
        for (p, v) in self.positions.iter().zip(&self.velocities) {
            let speed = v.length();
            let dir = if speed > 1e-4 { *v / speed } else { Vec3::Y };
            let length = 1.0 + speed * stretch / r;
            let start = mesh.vertices.len() as u32;
            for b in &base {
                let along = b.dot(dir);
                let q = *b + dir * along * (length - 1.0);
                // The normal of the stretched ellipsoid.
                let n = (*b - dir * along * (1.0 - 1.0 / length)).normalize();
                mesh.vertices.push(Vertex::new(*p + q * r, n, glam::Vec2::ZERO));
            }
            for f in &faces {
                mesh.indices.extend([start + f[0], start + f[1], start + f[2]]);
            }
        }
        mesh
    }
}

pub(super) fn step_water(fixed: Res<FixedTime>, mut surfaces: Query<(&Transform, &mut WaterSurface)>, mut bodies: Query<(&Transform, &mut RigidBody, &Collider)>) {
    let dt = fixed.timestep_secs();
    for (transform, mut water) in &mut surfaces {
        water.center = transform.translation;
        // Floating: buoyancy from the part of each body below the surface, drag in the water,
        // and waves where it moves.
        for (bt, mut body, collider) in &mut bodies {
            if body.kind != BodyKind::Dynamic || !water.contains(bt.translation) {
                continue;
            }
            let iso = Iso::new(bt.translation, bt.rotation);
            let bounds = collider.shapes.iter().fold(super::Aabb::EMPTY, |b, (o, s)| b.union(&s.aabb(&iso.mul(o))));
            let level = water.height_at(bt.translation);
            let depth = bounds.max.y - bounds.min.y;
            if depth <= 0.0 || bounds.min.y > level {
                continue;
            }
            let submerged = ((level - bounds.min.y) / depth).clamp(0.0, 1.0);
            let volume = collider.shapes.iter().map(|(_, s)| s.mass_properties().0).sum::<f32>();
            let lift = WATER_DENSITY * 9.81 * volume * submerged;
            let v = body.linear_velocity;
            body.apply_force(glam::Vec3::Y * lift - v * (WATER_DENSITY * volume * submerged * 2.0));
            let w = body.angular_velocity;
            body.apply_torque(-w * volume * WATER_DENSITY * submerged * 0.2);
            let size = (bounds.max - bounds.min).max_element() * 0.5;
            let wake = -v.y * 0.15 + Vec2::new(v.x, v.z).length() * 0.1;
            if wake.abs() > 0.01 {
                let c = water.coupling;
                let radius = size.max(water.cell);
                water.disturb(bt.translation, radius, -wake * c * submerged);
            }
        }
        water.step(dt);
    }
}

pub(super) fn step_particles(
    fixed: Res<FixedTime>,
    world: Option<Res<PhysicsWorld>>,
    mut fluids: Query<&mut ParticleFluid>,
    mut surfaces: Query<&mut WaterSurface>,
) {
    let dt = fixed.timestep_secs();
    let gravity = world.as_ref().map_or(Vec3::new(0.0, -9.81, 0.0), |w| w.gravity);
    for mut fluid in &mut fluids {
        let mut s: Vec<_> = surfaces.iter_mut().collect();
        let mut refs: Vec<&mut WaterSurface> = s.iter_mut().map(|m| &mut **m).collect();
        fluid.step(dt, gravity, world.as_deref(), &mut refs);
    }
}

/// Keeps water surface and droplet meshes matching the simulation.
pub fn update_fluid_meshes(meshes: Option<ResMut<Assets<Mesh>>>, surfaces: Query<&WaterSurface>, fluids: Query<&ParticleFluid>) {
    let Some(mut meshes) = meshes else { return };
    for water in &surfaces {
        let Some(handle) = water.mesh else { continue };
        let Some(mesh) = meshes.get_mut(handle) else { continue };
        let (w, h) = water.size;
        if mesh.vertices.len() != w * h {
            continue;
        }
        for j in 0..h {
            for i in 0..w {
                let k = j * w + i;
                let at = |di: isize, dj: isize| {
                    let (x, y) = ((i as isize + di).clamp(0, w as isize - 1) as usize, (j as isize + dj).clamp(0, h as isize - 1) as usize);
                    water.height[y * w + x]
                };
                let v = &mut mesh.vertices[k];
                v.position[1] = water.center.y + water.height[k];
                v.normal = Vec3::new(at(-1, 0) - at(1, 0), 2.0 * water.cell, at(0, -1) - at(0, 1)).normalize().into();
            }
        }
    }
    for fluid in &fluids {
        let Some(handle) = fluid.mesh else { continue };
        if let Some(mesh) = meshes.get_mut(handle) {
            *mesh = fluid.droplets(0.012);
        }
    }
}
