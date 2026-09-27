//! The village, its church, farms, windmill and bridge, modelled in code and textured with
//! the photographic materials.

use std::{
    f32::consts::{FRAC_PI_2, PI},
    sync::LazyLock,
};

use voxl::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::Vertex,
};

use crate::{
    materials::{GLASS, PLASTER, SLATE, STONE, WOOD},
    world::{tint, WorldMesh, NEUTRAL},
};

struct Shapes {
    cube: Mesh,
    cylinder: Mesh,
    prism: Mesh,
    pyramid: Mesh,
}

static SHAPES: LazyLock<Shapes> = LazyLock::new(|| Shapes {
    cube: Mesh::cube(1.0),
    cylinder: Mesh::cylinder(1.0, 1.0, 20),
    prism: prism(),
    pyramid: pyramid(),
});

fn push_triangle(mesh: &mut Mesh, a: Vec3, b: Vec3, c: Vec3) {
    let normal = (b - a).cross(c - a).normalize();
    let base = mesh.vertices.len() as u32;
    for p in [a, b, c] {
        mesh.vertices.push(Vertex::new(p, normal, Vec2::ZERO));
    }
    mesh.indices.extend([base, base + 1, base + 2]);
}

/// A gable-end wedge: 1 wide (X), 1 long (Z), 1 high, base on y = 0, ridge along Z.
fn prism() -> Mesh {
    let mut mesh = Mesh::default();
    let v = Vec3::new;
    push_triangle(&mut mesh, v(-0.5, 0.0, 0.5), v(0.5, 0.0, 0.5), v(0.0, 1.0, 0.5));
    push_triangle(&mut mesh, v(0.5, 0.0, -0.5), v(-0.5, 0.0, -0.5), v(0.0, 1.0, -0.5));
    for (a, b) in [(v(-0.5, 0.0, 0.5), v(-0.5, 0.0, -0.5)), (v(0.5, 0.0, -0.5), v(0.5, 0.0, 0.5))] {
        let (c, d) = (v(0.0, 1.0, b.z), v(0.0, 1.0, a.z));
        push_triangle(&mut mesh, a, b, c);
        push_triangle(&mut mesh, a, c, d);
    }
    mesh
}

/// A four-sided spire: 1 × 1 base on y = 0, apex at y = 1.
fn pyramid() -> Mesh {
    let mut mesh = Mesh::default();
    let c = [
        Vec3::new(-0.5, 0.0, -0.5),
        Vec3::new(-0.5, 0.0, 0.5),
        Vec3::new(0.5, 0.0, 0.5),
        Vec3::new(0.5, 0.0, -0.5),
    ];
    for i in 0..4 {
        push_triangle(&mut mesh, c[i], c[(i + 1) % 4], Vec3::Y);
    }
    mesh
}

/// Adds parts to a world mesh, all placed relative to `place`.
pub struct Build<'a> {
    pub mesh: &'a mut WorldMesh,
    pub place: Mat4,
}

impl Build<'_> {
    pub fn at(&mut self, translation: Vec3, yaw: f32) -> &mut Self {
        self.place = Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), translation);
        self
    }

    fn part(&mut self, shape: &Mesh, part: Mat4, layer: u32, t: [u8; 4]) {
        self.mesh.add(shape, part, self.place, layer, t);
    }

    pub fn cuboid(&mut self, center: Vec3, size: Vec3, layer: u32, t: [u8; 4]) {
        self.rotated(center, size, Quat::IDENTITY, layer, t);
    }

    pub fn rotated(&mut self, center: Vec3, size: Vec3, rotation: Quat, layer: u32, t: [u8; 4]) {
        let part = Mat4::from_scale_rotation_translation(size, rotation, center);
        self.part(&SHAPES.cube, part, layer, t);
    }

    /// A box between two points, `width` across and `depth` deep.
    pub fn beam(&mut self, from: Vec3, to: Vec3, width: f32, depth: f32, layer: u32, t: [u8; 4]) {
        let d = to - from;
        let rotation = Quat::from_rotation_arc(Vec3::Y, d.normalize());
        self.rotated((from + to) * 0.5, Vec3::new(width, d.length(), depth), rotation, layer, t);
    }

    pub fn cylinder(&mut self, center: Vec3, radius: f32, height: f32, layer: u32, t: [u8; 4]) {
        let part = Mat4::from_scale_rotation_translation(Vec3::new(radius, height, radius), Quat::IDENTITY, center);
        self.part(&SHAPES.cylinder, part, layer, t);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn frustum(&mut self, center: Vec3, bottom: f32, top: f32, height: f32, segments: u32, layer: u32, t: [u8; 4]) {
        let shape = Mesh::frustum(bottom, top, height, segments);
        self.part(&shape, Mat4::from_translation(center), layer, t);
    }

    pub fn spire(&mut self, base: Vec3, width: f32, height: f32, layer: u32, t: [u8; 4]) {
        let part = Mat4::from_scale_rotation_translation(Vec3::new(width, height, width), Quat::IDENTITY, base);
        self.part(&SHAPES.pyramid, part, layer, t);
    }

    /// A pitched roof over a `width` (X) × `depth` (Z) building whose walls end at
    /// `eaves`, ridge along X: gable walls in `wall`, two slabs of `roof`, ridge tiles on top.
    #[allow(clippy::too_many_arguments)]
    pub fn roof(&mut self, width: f32, depth: f32, eaves: f32, pitch: f32, roof: u32, roof_tint: [u8; 4], wall: u32, wall_tint: [u8; 4]) {
        let gable = Mat4::from_scale_rotation_translation(
            Vec3::new(depth, pitch, width),
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::new(0.0, eaves, 0.0),
        );
        self.part(&SHAPES.prism, gable, wall, wall_tint);
        let overhang = 0.45;
        let run = depth * 0.5;
        let slope = (pitch / run).atan();
        let length = (pitch * pitch + run * run).sqrt() + overhang;
        let thickness = 0.14;
        for side in [-1.0f32, 1.0] {
            // Each slab hangs from the ridge down past the eaves.
            let down = Vec3::new(0.0, -slope.sin(), side * slope.cos());
            let up_normal = Vec3::new(0.0, slope.cos(), side * slope.sin());
            let center = Vec3::new(0.0, eaves + pitch, 0.0) + down * (length * 0.5) + up_normal * (thickness * 0.5);
            let rotation = Quat::from_rotation_x(side * slope);
            self.rotated(center, Vec3::new(width + 0.7, thickness, length), rotation, roof, roof_tint);
        }
        let ridge = Mat4::from_scale_rotation_translation(
            Vec3::new(0.13, width + 0.7, 0.13),
            Quat::from_rotation_z(FRAC_PI_2),
            Vec3::new(0.0, eaves + pitch + 0.08, 0.0),
        );
        self.part(&SHAPES.cylinder, ridge, roof, roof_tint);
    }

    /// A window: glazing, a painted frame and glazing bars, a stone sill and lintel, and
    /// shutters folded back. Faces +Z from a wall at `z`.
    #[allow(clippy::too_many_arguments)]
    pub fn window(&mut self, x: f32, y: f32, z: f32, w: f32, h: f32, shutter: [u8; 4], shutters: bool) {
        let paint = tint(Vec3::new(1.2, 1.2, 1.15), 0.0);
        self.cuboid(Vec3::new(x, y, z + 0.01), Vec3::new(w, h, 0.04), GLASS, NEUTRAL);
        for s in [-1.0, 1.0] {
            self.cuboid(Vec3::new(x + s * (w * 0.5 + 0.03), y, z + 0.04), Vec3::new(0.07, h + 0.08, 0.08), PLASTER, paint);
        }
        self.cuboid(Vec3::new(x, y + h * 0.5 + 0.03, z + 0.04), Vec3::new(w + 0.12, 0.07, 0.08), PLASTER, paint);
        self.cuboid(Vec3::new(x, y, z + 0.04), Vec3::new(0.035, h, 0.05), PLASTER, paint);
        for k in [-1.0, 1.0] {
            self.cuboid(Vec3::new(x, y + k * h / 6.0, z + 0.04), Vec3::new(w, 0.03, 0.05), PLASTER, paint);
        }
        self.cuboid(Vec3::new(x, y - h * 0.5 - 0.06, z + 0.07), Vec3::new(w + 0.3, 0.1, 0.18), STONE, NEUTRAL);
        self.cuboid(Vec3::new(x, y + h * 0.5 + 0.14, z + 0.03), Vec3::new(w + 0.4, 0.2, 0.08), STONE, NEUTRAL);
        if shutters {
            for s in [-1.0, 1.0] {
                self.cuboid(Vec3::new(x + s * (w + 0.1) * 0.75 + s * 0.05, y, z + 0.07), Vec3::new(w * 0.5, h + 0.04, 0.04), PLASTER, shutter);
            }
        }
    }
}

pub struct Style {
    pub wall: u32,
    pub wall_tint: [u8; 4],
    pub roof: u32,
    pub roof_tint: [u8; 4],
    pub shutters: [u8; 4],
}

/// A house or barn, front (door side) facing -Z, on ground at y = 0. Walls and plinth run
/// three metres below ground so it sits on any slope.
pub fn house(b: &mut Build, width: f32, depth: f32, stories: u32, style: &Style, seed: f32, barn: bool) {
    let story = 2.8;
    let eaves = story * stories as f32 + 0.3;
    b.cuboid(Vec3::new(0.0, (eaves - 3.0) * 0.5, 0.0), Vec3::new(width, eaves + 3.0, depth), style.wall, style.wall_tint);
    // A stone plinth, and long-and-short quoins up the corners.
    b.cuboid(Vec3::new(0.0, -1.2, 0.0), Vec3::new(width + 0.12, 3.0, depth + 0.12), STONE, NEUTRAL);
    if style.wall != STONE {
        let mut y = 0.3;
        let mut long = true;
        while y < eaves - 0.2 {
            for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let (lx, lz) = if long { (0.55, 0.3) } else { (0.3, 0.55) };
                b.cuboid(
                    Vec3::new(sx * (width * 0.5 - lx * 0.5 + 0.03), y + 0.15, sz * (depth * 0.5 - lz * 0.5 + 0.03)),
                    Vec3::new(lx, 0.3, lz),
                    STONE,
                    NEUTRAL,
                );
            }
            y += 0.32;
            long = !long;
        }
    }
    let pitch = depth * if style.roof == SLATE { 0.62 } else { 0.5 };
    b.roof(width, depth, eaves, pitch, style.roof, style.roof_tint, style.wall, style.wall_tint);
    // Chimney through the ridge.
    let cx = width * (0.2 + seed * 0.2) * if seed > 0.5 { 1.0 } else { -1.0 };
    b.cuboid(Vec3::new(cx, eaves + pitch * 0.6, 0.0), Vec3::new(0.7, pitch * 1.1 + 0.8, 0.55), STONE, NEUTRAL);
    b.cuboid(Vec3::new(cx, eaves + pitch * 1.15 + 0.45, 0.0), Vec3::new(0.85, 0.12, 0.7), STONE, NEUTRAL);

    if barn {
        // Big double doors, and a hayloft opening in the gable.
        b.cuboid(Vec3::new(0.0, 1.6, -depth * 0.5 - 0.04), Vec3::new(3.2, 3.2, 0.1), WOOD, tint(Vec3::splat(0.75), 0.0));
        b.cuboid(Vec3::new(0.0, 3.3, -depth * 0.5 - 0.06), Vec3::new(3.6, 0.3, 0.14), WOOD, tint(Vec3::splat(0.6), 0.0));
        return;
    }
    let bays = ((width - 1.2) / 2.4).floor().max(1.0) as i32;
    let door_bay = (seed * bays as f32) as i32 % bays;
    for s in 0..stories {
        let y = 1.5 + s as f32 * story;
        for bay in 0..bays {
            let x = (bay as f32 - (bays - 1) as f32 * 0.5) * 2.4;
            for side in [-1.0f32, 1.0] {
                let front = side < 0.0;
                if front && s == 0 && bay == door_bay {
                    b.at_side(side, depth, |b, z| {
                        b.cuboid(Vec3::new(x, 1.05, z), Vec3::new(1.0, 2.1, 0.08), WOOD, tint(Vec3::splat(0.55), 0.0));
                        b.cuboid(Vec3::new(x, 2.22, z + 0.02), Vec3::new(1.35, 0.25, 0.1), STONE, NEUTRAL);
                        for k in [-1.0, 1.0] {
                            b.cuboid(Vec3::new(x + k * 0.6, 1.05, z + 0.02), Vec3::new(0.18, 2.2, 0.1), STONE, NEUTRAL);
                        }
                    });
                    continue;
                }
                let shutters = style.shutters;
                b.at_side(side, depth, |b, z| b.window(x, y, z, 0.8, 1.2, shutters, true));
            }
        }
    }
}

impl Build<'_> {
    /// Runs `f` in a frame turned to face out of the front (-Z) or back (+Z) wall, passing the
    /// wall's surface as z.
    fn at_side(&mut self, side: f32, depth: f32, f: impl FnOnce(&mut Self, f32)) {
        let saved = self.place;
        if side < 0.0 {
            self.place = saved * Mat4::from_rotation_y(PI);
        }
        f(self, depth * 0.5);
        self.place = saved;
    }
}

/// A village church: nave, apse, west tower and slate spire. The door faces +Z.
pub fn church(b: &mut Build) {
    let wall = tint(Vec3::new(1.05, 1.02, 0.95), 0.0);
    let slate = NEUTRAL;
    b.cuboid(Vec3::new(0.0, 2.5, -4.0), Vec3::new(10.0, 11.0, 22.0), STONE, wall);
    b.cuboid(Vec3::new(0.0, -1.0, -4.0), Vec3::new(10.3, 3.0, 22.3), STONE, NEUTRAL);
    let saved = b.place;
    b.place = saved * Mat4::from_rotation_translation(Quat::from_rotation_y(FRAC_PI_2), Vec3::new(0.0, 0.0, -4.0));
    b.roof(22.0, 10.0, 8.0, 6.5, SLATE, slate, STONE, wall);
    b.place = saved;
    b.frustum(Vec3::new(0.0, 2.5, -15.0), 4.8, 4.8, 11.0, 16, STONE, wall);
    b.frustum(Vec3::new(0.0, 10.0, -15.0), 5.3, 0.4, 4.0, 16, SLATE, slate);
    for i in 0..4 {
        let z = -12.0 + i as f32 * 5.0;
        for side in [-1.0, 1.0] {
            b.cuboid(Vec3::new(side * 5.3, 2.5, z), Vec3::new(0.8, 11.0, 1.0), STONE, wall);
            // Tall lancet windows between the buttresses.
            b.cuboid(Vec3::new(side * 5.03, 4.8, z + 2.5), Vec3::new(0.08, 3.6, 1.1), GLASS, NEUTRAL);
            b.cuboid(Vec3::new(side * 5.05, 2.95, z + 2.5), Vec3::new(0.2, 0.12, 1.4), STONE, NEUTRAL);
        }
    }
    b.cuboid(Vec3::new(0.0, 6.5, 9.5), Vec3::new(6.0, 19.0, 6.0), STONE, wall);
    b.cuboid(Vec3::new(0.0, 16.3, 9.5), Vec3::new(6.6, 0.5, 6.6), STONE, NEUTRAL);
    for (x, z, rot) in [(0.0, 12.52, 0.0), (0.0, 6.48, PI), (3.02, 9.5, FRAC_PI_2), (-3.02, 9.5, -FRAC_PI_2)] {
        let saved = b.place;
        b.place = saved * Mat4::from_rotation_translation(Quat::from_rotation_y(rot), Vec3::new(x, 0.0, z));
        // Louvred belfry openings.
        b.cuboid(Vec3::new(0.0, 14.0, 0.0), Vec3::new(1.3, 2.6, 0.06), GLASS, NEUTRAL);
        for k in 0..6 {
            b.cuboid(Vec3::new(0.0, 12.9 + k as f32 * 0.42, 0.06), Vec3::new(1.3, 0.06, 0.1), WOOD, tint(Vec3::splat(0.6), 0.0));
        }
        b.place = saved;
    }
    b.spire(Vec3::new(0.0, 16.5, 9.5), 6.4, 15.0, SLATE, slate);
    b.cylinder(Vec3::new(0.0, 32.0, 9.5), 0.05, 2.4, STONE, tint(Vec3::splat(0.2), 0.0));
    b.cuboid(Vec3::new(0.0, 32.6, 9.5), Vec3::new(0.8, 0.07, 0.07), STONE, tint(Vec3::splat(0.2), 0.0));
    b.cuboid(Vec3::new(0.0, 1.7, 12.53), Vec3::new(1.8, 3.4, 0.08), WOOD, tint(Vec3::splat(0.5), 0.0));
    b.cuboid(Vec3::new(0.0, 3.55, 12.56), Vec3::new(2.6, 0.35, 0.14), STONE, NEUTRAL);
}

/// A stone tower mill; the sails turn separately.
pub fn windmill_tower(b: &mut Build) {
    let wall = tint(Vec3::new(1.08, 1.06, 1.0), 0.0);
    b.frustum(Vec3::new(0.0, 3.5, 0.0), 3.6, 2.8, 13.0, 24, PLASTER, wall);
    b.frustum(Vec3::new(0.0, -2.0, 0.0), 3.75, 3.75, 2.0, 24, STONE, NEUTRAL);
    b.frustum(Vec3::new(0.0, 11.0, 0.0), 3.15, 3.15, 0.6, 24, WOOD, tint(Vec3::splat(0.6), 0.0));
    b.frustum(Vec3::new(0.0, 12.8, 0.0), 3.2, 0.25, 3.2, 24, WOOD, tint(Vec3::splat(0.7), 0.0));
    b.cuboid(Vec3::new(0.0, 1.1, -3.4), Vec3::new(1.1, 2.2, 0.3), WOOD, tint(Vec3::splat(0.55), 0.0));
    b.cylinder(Vec3::new(0.0, 11.0, -3.1), 0.25, 1.2, WOOD, NEUTRAL);
}

pub const SAIL_HUB: Vec3 = Vec3::new(0.0, 11.0, -3.8);

/// A stone bridge of three arches along local Z, deck at `deck`. The arches are open.
pub fn bridge(b: &mut Build, half_length: f32, half_width: f32, deck: f32) {
    let stone = NEUTRAL;
    let spans = [(-14.0f32, 6.0f32), (0.0, 6.5), (14.0, 6.0)];
    let springing = deck - 4.2;
    let w = half_width * 2.0;
    for (z, span) in spans {
        let r = span * 0.5;
        // Voussoirs round the arch ring, as a barrel vault through the bridge.
        let n = 15;
        for i in 0..n {
            let a0 = PI * i as f32 / n as f32;
            let a1 = PI * (i + 1) as f32 / n as f32;
            let p = |a: f32, rr: f32| Vec3::new(0.0, springing + a.sin() * rr, z - a.cos() * rr);
            let mid = (a0 + a1) * 0.5;
            let rot = Quat::from_rotation_x(mid - FRAC_PI_2);
            let center = (p(a0, r + 0.25) + p(a1, r + 0.25)) * 0.5;
            b.rotated(center, Vec3::new(w, 0.5, (p(a1, r) - p(a0, r)).length() + 0.05), rot, STONE, stone);
        }
        // Spandrel walls filling from the arch up to the deck, in slices.
        let slices = 12;
        for i in 0..slices {
            let u0 = -r - 0.5 + (2.0 * r + 1.0) * i as f32 / slices as f32;
            let u1 = u0 + (2.0 * r + 1.0) / slices as f32;
            let mid = (u0 + u1) * 0.5;
            let bottom = springing + (r * r - mid.min(r).max(-r).powi(2)).max(0.0).sqrt() + 0.35;
            let top = deck - 0.2;
            if top > bottom {
                b.cuboid(Vec3::new(0.0, (top + bottom) * 0.5, z + mid), Vec3::new(w, top - bottom, u1 - u0 + 0.02), STONE, stone);
            }
        }
    }
    // Piers with pointed cutwaters, between the arches, down into the riverbed.
    for z in [-7.0f32, 7.0] {
        let pier_len: f32 = 14.0 - 6.25 - 0.1;
        b.cuboid(Vec3::new(0.0, springing - 3.0, z), Vec3::new(w + 0.2, 6.2, pier_len.max(1.4)), STONE, stone);
        let fill = deck - 0.2 - springing;
        b.cuboid(Vec3::new(0.0, springing + fill * 0.5, z), Vec3::new(w, fill, pier_len + 0.4), STONE, stone);
        for s in [-1.0, 1.0] {
            b.rotated(Vec3::new(s * (half_width + 0.1), springing - 2.5, z), Vec3::new(1.4, 7.0, 1.4), Quat::from_rotation_y(0.785), STONE, stone);
            b.frustum(Vec3::new(s * (half_width + 0.1), springing + 1.2, z), 1.0, 0.1, 0.8, 4, STONE, stone);
        }
    }
    b.cuboid(Vec3::new(0.0, deck - 0.1, 0.0), Vec3::new(w, 0.2, half_length * 2.0), STONE, tint(Vec3::splat(0.85), 0.0));
    for s in [-1.0, 1.0] {
        b.cuboid(Vec3::new(s * (half_width - 0.22), deck + 0.45, 0.0), Vec3::new(0.44, 0.9, half_length * 2.0), STONE, stone);
        b.cuboid(Vec3::new(s * (half_width - 0.22), deck + 0.95, 0.0), Vec3::new(0.54, 0.12, half_length * 2.0 + 0.1), STONE, stone);
    }
    // Abutments running into the banks.
    for z in [-half_length + 2.5, half_length - 2.5] {
        b.cuboid(Vec3::new(0.0, deck - 4.6, z), Vec3::new(w + 1.0, 9.0, 6.0), STONE, stone);
    }
}

/// A post-and-rail fence from `a` to `b` on the ground heights given.
pub fn fence(b: &mut Build, from: Vec3, to: Vec3, ground: impl Fn(Vec2) -> f32) {
    let wood = tint(Vec3::splat(0.75), 0.0);
    let length = from.distance(to);
    let posts = (length / 2.5).ceil().max(1.0) as i32;
    let mut last: Option<Vec3> = None;
    for i in 0..=posts {
        let p2 = from.lerp(to, i as f32 / posts as f32);
        let p = Vec3::new(p2.x, ground(Vec2::new(p2.x, p2.z)), p2.z);
        b.cuboid(p + Vec3::Y * 0.5, Vec3::new(0.12, 1.4, 0.12), WOOD, wood);
        if let Some(q) = last {
            for h in [0.45, 0.95] {
                b.beam(q + Vec3::Y * h, p + Vec3::Y * h, 0.07, 0.05, WOOD, wood);
            }
        }
        last = Some(p);
    }
}

/// A dry-stone wall with a rough coping, following the ground.
pub fn stone_wall(b: &mut Build, from: Vec3, to: Vec3, ground: impl Fn(Vec2) -> f32) {
    let steps = (from.distance(to) / 2.0).ceil().max(1.0) as i32;
    for i in 0..steps {
        let a = from.lerp(to, i as f32 / steps as f32);
        let c = from.lerp(to, (i + 1) as f32 / steps as f32);
        let m = (a + c) * 0.5;
        let y = ground(Vec2::new(m.x, m.z));
        let dir = (c - a).normalize();
        let yaw = (-dir.x).atan2(-dir.z);
        b.rotated(Vec3::new(m.x, y + 0.3, m.z), Vec3::new(0.55, 1.6, a.distance(c) + 0.05), Quat::from_rotation_y(yaw), STONE, NEUTRAL);
        b.rotated(Vec3::new(m.x, y + 1.15, m.z), Vec3::new(0.62, 0.12, a.distance(c) + 0.05), Quat::from_rotation_y(yaw), STONE, tint(Vec3::splat(0.8), 0.0));
    }
}

