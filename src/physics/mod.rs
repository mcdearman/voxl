//! Rigid body physics: bodies that fall, collide, bounce, slide, roll, stack and sleep; joints
//! that hold them together; ray casts and overlap queries; a character controller that walks
//! and slides along the world; and fluids (see `fluid`).
//!
//! Put a `Collider` on an entity to make it solid. Add a `RigidBody` to make it move: dynamic
//! bodies are moved by forces and collisions; kinematic ones by setting their velocity, pushing
//! dynamic ones aside and never pushed back. Colliders without a body are static. Bodies must
//! be root entities (no `Parent`); add `Interpolate` for smooth motion between steps.
//!
//! Each fixed step gathers every collider, finds candidate pairs by sweep and prune along X,
//! finds contacts (see `collide`), and solves contacts and joints together with sequential
//! impulses, warm started from the last step, with Coulomb friction and restitution.

mod collide;
pub mod fluid;
mod query;
mod shape;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use glam::{Mat3, Quat, Vec3};

pub use collide::{closest_on_segment, closest_on_triangle, closest_segments, Manifold, MARGIN};
pub use query::{CharacterController, RayHit};
pub use shape::{ray_triangle, Aabb, Iso, Shape, TriMesh};

use crate::{
    app::{App, Plugin, Stage},
    ecs::{Component, Entity, Query, Res, ResMut, Without},
    reflect::Reflect,
    time::FixedTime,
    transform::{GlobalTransform, Parent, Transform},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
#[reflect(name = "mira.BodyKind")]
pub enum BodyKind {
    /// Moved by gravity, forces and collisions.
    Dynamic,
    /// Moved only by its velocity, which the game sets; pushes dynamic bodies, never pushed.
    Kinematic,
}

/// Makes a collider's entity move. The entity's `Transform` is the body's position and
/// orientation; its centre of mass is at the entity's origin.
#[derive(Clone, Debug, Reflect)]
#[reflect(name = "mira.RigidBody", default)]
pub struct RigidBody {
    pub kind: BodyKind,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    /// Kilograms; 0 to work it out from the collider's volume and `density`.
    pub mass: f32,
    /// Kilograms per cubic metre, when `mass` is 0. Water is 1000, wood about 600, stone 2500.
    pub density: f32,
    /// Fraction of velocity lost each second, to air and the like.
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
    /// Keeps it upright (characters, animals).
    pub lock_rotation: bool,
    /// For a kinematic body the game moves by setting its `Transform` (a walker steered by
    /// its own code): its velocity is measured from how far it moved each step, so what it
    /// bumps into is pushed as hard as it was hit.
    pub follows_transform: bool,
    #[reflect(skip)]
    last_position: Option<Vec3>,
    #[reflect(skip)]
    force: Vec3,
    #[reflect(skip)]
    torque: Vec3,
    #[reflect(skip)]
    impulse: Vec3,
    #[reflect(skip)]
    idle: f32,
    #[reflect(skip)]
    sleeping: bool,
}

impl Component for RigidBody {}

impl Default for RigidBody {
    fn default() -> Self {
        Self::dynamic()
    }
}

impl RigidBody {
    pub fn dynamic() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            mass: 0.0,
            density: 1000.0,
            linear_damping: 0.02,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            lock_rotation: false,
            follows_transform: false,
            last_position: None,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            impulse: Vec3::ZERO,
            idle: 0.0,
            sleeping: false,
        }
    }

    pub fn kinematic() -> Self {
        Self { kind: BodyKind::Kinematic, ..Self::dynamic() }
    }

    /// A kinematic body moved by setting its `Transform` (see `follows_transform`).
    pub fn animated() -> Self {
        Self { follows_transform: true, ..Self::kinematic() }
    }

    pub fn with_mass(mut self, mass: f32) -> Self {
        self.mass = mass;
        self
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    pub fn with_velocity(mut self, v: Vec3) -> Self {
        self.linear_velocity = v;
        self
    }

    /// A force (newtons) through the centre of mass for the next step.
    pub fn apply_force(&mut self, force: Vec3) {
        self.force += force;
        self.wake();
    }

    pub fn apply_torque(&mut self, torque: Vec3) {
        self.torque += torque;
        self.wake();
    }

    /// A sudden push (newton-seconds) through the centre of mass.
    pub fn apply_impulse(&mut self, impulse: Vec3) {
        self.impulse += impulse;
        self.wake();
    }

    pub fn wake(&mut self) {
        self.sleeping = false;
        self.idle = 0.0;
    }

    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }
}

/// What an entity collides as: one or more shapes, placed relative to the entity.
#[derive(Clone, Debug, Reflect)]
#[reflect(name = "mira.Collider")]
pub struct Collider {
    pub shapes: Vec<(Iso, Shape)>,
    pub friction: f32,
    /// Bounciness, 0 to 1.
    pub restitution: f32,
    /// Detects overlaps (see `PhysicsWorld::sensor_overlaps`) without colliding.
    pub sensor: bool,
    /// Collision layers: two colliders collide if each one's `layers` meets the other's `mask`.
    pub layers: u32,
    pub mask: u32,
}

impl Component for Collider {}

impl Collider {
    pub fn new(shape: Shape) -> Self {
        Self { shapes: vec![(Iso::IDENTITY, shape)], friction: 0.6, restitution: 0.1, sensor: false, layers: 1, mask: u32::MAX }
    }

    pub fn sphere(radius: f32) -> Self {
        Self::new(Shape::sphere(radius))
    }

    pub fn cuboid(half: Vec3) -> Self {
        Self::new(Shape::cuboid(half))
    }

    /// A capsule standing `height` tall.
    pub fn capsule(height: f32, radius: f32) -> Self {
        Self::new(Shape::capsule(height, radius))
    }

    pub fn ground() -> Self {
        Self::new(Shape::ground())
    }

    pub fn trimesh(mesh: TriMesh) -> Self {
        Self::new(Shape::TriMesh(std::sync::Arc::new(mesh)))
    }

    /// Several shapes as one.
    pub fn compound(shapes: Vec<(Iso, Shape)>) -> Self {
        Self { shapes, ..Self::new(Shape::sphere(0.0)) }
    }

    /// Moves the (single) shape off the entity's origin.
    pub fn at(mut self, offset: Vec3) -> Self {
        for (iso, _) in &mut self.shapes {
            iso.position += offset;
        }
        self
    }

    pub fn with_friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    pub fn with_restitution(mut self, restitution: f32) -> Self {
        self.restitution = restitution;
        self
    }

    pub fn as_sensor(mut self) -> Self {
        self.sensor = true;
        self
    }

    pub fn with_layers(mut self, layers: u32, mask: u32) -> Self {
        self.layers = layers;
        self.mask = mask;
        self
    }

    fn mass_properties(&self, density: f32) -> (f32, Vec3) {
        let mut mass = 0.0;
        let mut inertia = Vec3::ZERO;
        for (iso, shape) in &self.shapes {
            let (volume, unit) = shape.mass_properties();
            let m = volume * density;
            let r = Mat3::from_quat(iso.rotation);
            // The shape's inertia turned into the body's axes (diagonal kept), moved by the
            // parallel axis theorem.
            let local = Vec3::new(
                (r.x_axis * r.x_axis * unit).element_sum(),
                (r.y_axis * r.y_axis * unit).element_sum(),
                (r.z_axis * r.z_axis * unit).element_sum(),
            );
            let d = iso.position;
            inertia += local * density + m * Vec3::new(d.y * d.y + d.z * d.z, d.x * d.x + d.z * d.z, d.x * d.x + d.y * d.y);
            mass += m;
        }
        (mass, inertia)
    }
}

#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.JointKind")]
pub enum JointKind {
    /// The anchors held together; free to turn any way.
    Ball,
    /// The anchors held together, turning only about `axis` (in each body's own space), within
    /// `limits` (radians) if given.
    Hinge { axis_a: Vec3, axis_b: Vec3, limits: Option<(f32, f32)> },
    /// The anchors kept between `min` and `max` apart: a rod when equal, a rope when min is 0.
    Distance { min: f32, max: f32 },
    /// Welded: the anchors held together and the relative orientation kept.
    Fixed,
}

/// Joins two bodies (or a body to the world, with `b` none). Anchors are in each body's own
/// space; with no `b`, `anchor_b` is a point in the world.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.Joint")]
pub struct Joint {
    pub a: Entity,
    pub b: Option<Entity>,
    pub anchor_a: Vec3,
    pub anchor_b: Vec3,
    pub kind: JointKind,
}

impl Component for Joint {}

/// A contact this step, for sounds, splashes and game logic.
#[derive(Clone, Copy, Debug)]
pub struct ContactReport {
    pub a: Entity,
    pub b: Entity,
    pub point: Vec3,
    /// From `a` toward `b`.
    pub normal: Vec3,
    /// Newton-seconds pushed apart this step.
    pub impulse: f32,
}

/// Settings, and what the last step found: contacts, sensor overlaps, and every collider, for
/// queries.
pub struct PhysicsWorld {
    pub gravity: Vec3,
    /// Solver passes each step; more make stacks stiffer.
    pub iterations: usize,
    pub contacts: Vec<ContactReport>,
    pub sensor_overlaps: Vec<(Entity, Entity)>,
    pub(crate) colliders: Vec<ColliderRecord>,
    cache: HashMap<(Entity, Entity, u32), Vec<CachedPoint>>,
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            iterations: 12,
            contacts: Vec::new(),
            sensor_overlaps: Vec::new(),
            colliders: Vec::new(),
            cache: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ColliderRecord {
    pub entity: Entity,
    pub shape: Shape,
    pub iso: Iso,
    pub aabb: Aabb,
    pub body: Option<usize>,
    pub friction: f32,
    pub restitution: f32,
    pub sensor: bool,
    pub layers: u32,
    pub mask: u32,
    /// Which of its entity's shapes.
    pub part: u32,
}

#[derive(Clone, Copy, Debug)]
struct CachedPoint {
    local: Vec3,
    normal: f32,
    tangent: [f32; 2],
}

struct Body {
    kind: BodyKind,
    position: Vec3,
    rotation: Quat,
    v: Vec3,
    w: Vec3,
    inv_mass: f32,
    inv_inertia: Mat3,
    sleeping: bool,
    follows: bool,
}

impl Body {
    fn is_static(&self) -> bool {
        self.inv_mass == 0.0
    }
}

/// The name of the event native plugins read to learn of contacts (`MiraContact` in mira.h).
pub const CONTACT_EVENT: &str = "mira.Contact";

/// Publishes each step's contacts as events, so plugins in any language can react to them.
fn publish_contacts(world: Res<PhysicsWorld>, mut events: ResMut<crate::plugin::PluginEvents>) {
    let Some(id) = events.id(CONTACT_EVENT) else {
        return;
    };
    for contact in &world.contacts {
        // Laid out as `MiraContact`: two entities, a point, a normal, an impulse, padding.
        let mut bytes = [0u8; 48];
        bytes[0..8].copy_from_slice(&contact.a.to_bits().to_ne_bytes());
        bytes[8..16].copy_from_slice(&contact.b.to_bits().to_ne_bytes());
        let floats = contact
            .point
            .to_array()
            .into_iter()
            .chain(contact.normal.to_array())
            .chain([contact.impulse]);
        for (i, value) in floats.enumerate() {
            bytes[16 + i * 4..20 + i * 4].copy_from_slice(&value.to_ne_bytes());
        }
        events.send(id, &bytes);
    }
}

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        if let Err(err) = app
            .world
            .resource_mut::<crate::plugin::PluginEvents>()
            .register(CONTACT_EVENT, 48)
        {
            log::error!("{err}");
        }
        app.register_type::<RigidBody>()
            .register_type::<Collider>()
            .register_type::<Joint>()
            .register_type::<CharacterController>();
        app.init_resource::<PhysicsWorld>()
            .add_systems(
                Stage::FixedUpdate,
                (step, publish_contacts, query::move_characters, fluid::step_water, fluid::step_particles),
            )
            .add_systems(Stage::PostUpdate, fluid::update_fluid_meshes);
    }
}

/// Constraint rows for one contact point.
struct ContactPoint {
    ra: Vec3,
    rb: Vec3,
    normal_mass: f32,
    tangent_mass: [f32; 2],
    target: f32,
    normal_impulse: f32,
    tangent_impulse: [f32; 2],
    local: Vec3,
    world: Vec3,
}

struct ContactConstraint {
    a: Option<usize>,
    b: Option<usize>,
    normal: Vec3,
    tangents: [Vec3; 2],
    friction: f32,
    points: Vec<ContactPoint>,
    key: (Entity, Entity, u32),
    entities: (Entity, Entity),
}

#[allow(clippy::type_complexity)]
fn step(
    fixed: Res<FixedTime>,
    mut world: ResMut<PhysicsWorld>,
    mut bodies: Query<(Entity, &mut Transform, &mut RigidBody, &Collider)>,
    statics: Query<(Entity, &Transform, &GlobalTransform, Option<&Parent>, &Collider), Without<RigidBody>>,
    joints: Query<&Joint>,
) {
    let dt = fixed.timestep_secs();
    let gravity = world.gravity;

    // ---- Gather.
    let mut list: Vec<Body> = Vec::new();
    let mut index: HashMap<Entity, usize> = HashMap::new();
    let mut records: Vec<ColliderRecord> = Vec::new();
    for (entity, transform, mut body, collider) in &mut bodies {
        let (mass, inertia) = if body.mass > 0.0 {
            let (m0, i0) = collider.mass_properties(1.0);
            let scale = if m0 > 0.0 { body.mass / m0 } else { 0.0 };
            (body.mass, i0 * scale)
        } else {
            collider.mass_properties(body.density)
        };
        let dynamic = body.kind == BodyKind::Dynamic && mass > 0.0;
        let inv_mass = if dynamic { 1.0 / mass } else { 0.0 };
        let inv_inertia_local = if dynamic && !body.lock_rotation { inertia.max(Vec3::splat(1e-6)).recip() } else { Vec3::ZERO };
        let mut v = body.linear_velocity;
        let mut w = body.angular_velocity;
        if dynamic && !body.sleeping {
            v += (gravity * body.gravity_scale + body.force * inv_mass) * dt + body.impulse * inv_mass;
            w += Mat3::from_quat(transform.rotation) * (inv_inertia_local * (transform.rotation.inverse() * body.torque)) * dt;
            v *= (1.0 - body.linear_damping * dt).max(0.0);
            w *= (1.0 - body.angular_damping * dt).max(0.0);
        }
        body.force = Vec3::ZERO;
        body.torque = Vec3::ZERO;
        body.impulse = Vec3::ZERO;
        let follows = body.kind == BodyKind::Kinematic && body.follows_transform;
        if follows {
            // Smoothed, as the game may move it once for several steps (or several for one).
            let measured = body.last_position.map_or(Vec3::ZERO, |last| (transform.translation - last) / dt);
            let measured = if measured.length() > 30.0 { Vec3::ZERO } else { measured };
            v = body.linear_velocity * 0.5 + measured * 0.5;
            body.linear_velocity = v;
            body.last_position = Some(transform.translation);
            w = Vec3::ZERO;
        }
        let i = list.len();
        index.insert(entity, i);
        let r = Mat3::from_quat(transform.rotation);
        list.push(Body {
            kind: body.kind,
            position: transform.translation,
            rotation: transform.rotation,
            v,
            w,
            inv_mass,
            inv_inertia: r * Mat3::from_diagonal(inv_inertia_local) * r.transpose(),
            sleeping: body.sleeping && body.kind == BodyKind::Dynamic,
            follows,
        });
        let base = Iso::new(transform.translation, transform.rotation);
        push_records(&mut records, entity, collider, &base, Some(i), v, dt);
    }
    for (entity, transform, global, parent, collider) in &statics {
        let base = if parent.is_some() {
            let (_, rotation, translation) = global.0.to_scale_rotation_translation();
            Iso::new(translation, rotation)
        } else {
            Iso::new(transform.translation, transform.rotation)
        };
        push_records(&mut records, entity, collider, &base, None, Vec3::ZERO, dt);
    }

    // ---- Broad phase: sweep and prune along X.
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by(|&a, &b| records[a].aabb.min.x.total_cmp(&records[b].aabb.min.x));
    let mut pairs = Vec::new();
    // Half spaces span everything; keep them out of the sweep.
    let (planes, sweep): (Vec<usize>, Vec<usize>) = order.into_iter().partition(|&i| matches!(records[i].shape, Shape::HalfSpace { .. }));
    for (k, &i) in sweep.iter().enumerate() {
        for &j in &sweep[k + 1..] {
            if records[j].aabb.min.x > records[i].aabb.max.x {
                break;
            }
            if records[i].aabb.overlaps(&records[j].aabb) {
                pairs.push((i, j));
            }
        }
        for &p in &planes {
            pairs.push((i, p));
        }
    }

    // ---- Narrow phase.
    world.sensor_overlaps.clear();
    let mut manifolds: Vec<(usize, usize, Manifold)> = Vec::new();
    let mut scratch = Vec::new();
    for (i, j) in pairs {
        let (ra, rb) = (&records[i], &records[j]);
        if ra.entity == rb.entity || (ra.body.is_none() && rb.body.is_none()) {
            continue;
        }
        if ra.layers & rb.mask == 0 || rb.layers & ra.mask == 0 {
            continue;
        }
        // Static: no body, or a dynamic one with no mass. Resting: static, asleep, or a
        // kinematic body standing still.
        let fixed = |r: &ColliderRecord| r.body.is_none_or(|b| list[b].is_static() && list[b].kind == BodyKind::Dynamic);
        let resting = |r: &ColliderRecord| {
            r.body.is_none_or(|b| {
                let x = &list[b];
                x.sleeping || (x.kind == BodyKind::Dynamic && x.is_static()) || (x.kind == BodyKind::Kinematic && x.v == Vec3::ZERO && x.w == Vec3::ZERO)
            })
        };
        let (static_a, static_b, asleep_a, asleep_b) = (fixed(ra), fixed(rb), resting(ra), resting(rb));
        if (static_a && static_b) || (asleep_a && asleep_b && !ra.sensor && !rb.sensor) {
            continue;
        }
        // Only something that can be pushed makes a contact worth solving: a kinematic body
        // passes through statics and other kinematics (though sensors still notice it).
        let pushable = |r: &ColliderRecord| r.body.is_some_and(|b| list[b].inv_mass > 0.0);
        if !pushable(ra) && !pushable(rb) && !ra.sensor && !rb.sensor {
            continue;
        }
        scratch.clear();
        collide::contact(&ra.shape, &ra.iso, &rb.shape, &rb.iso, &mut scratch);
        if scratch.is_empty() {
            continue;
        }
        if ra.sensor || rb.sensor {
            if scratch.iter().any(|m| m.points.iter().any(|p| p.1 > 0.0)) {
                let (s, o) = if ra.sensor { (ra.entity, rb.entity) } else { (rb.entity, ra.entity) };
                world.sensor_overlaps.push((s, o));
            }
            continue;
        }
        for m in scratch.drain(..) {
            manifolds.push((i, j, m));
        }
    }

    // A sleeping body touched by a moving one wakes.
    for (i, j, _) in &manifolds {
        let (bi, bj) = (records[*i].body, records[*j].body);
        for (x, y) in [(bi, bj), (bj, bi)] {
            if let (Some(x), Some(y)) = (x, y) {
                if list[x].sleeping && !list[y].sleeping && (list[y].v.length() + list[y].w.length()) > 0.05 {
                    list[x].sleeping = false;
                }
            }
        }
    }

    // ---- Prepare contacts, warm started from last step's impulses.
    let mut constraints: Vec<ContactConstraint> = Vec::with_capacity(manifolds.len());
    let mut new_cache: HashMap<(Entity, Entity, u32), Vec<CachedPoint>> = HashMap::new();
    for (i, j, m) in manifolds {
        let (ra, rb) = (&records[i], &records[j]);
        let (a, b) = (ra.body, rb.body);
        let key = (ra.entity, rb.entity, ra.part * 1024 + rb.part);
        let cached = world.cache.get(&key);
        let n = m.normal;
        let t1 = if n.x.abs() < 0.57 { n.cross(Vec3::X) } else { n.cross(Vec3::Y) }.normalize();
        let t2 = n.cross(t1);
        let friction = (ra.friction * rb.friction).sqrt();
        let restitution = ra.restitution.max(rb.restitution);
        let mut points = Vec::with_capacity(m.points.len());
        for (p, depth) in m.points {
            let (pa, qa) = a.map_or((p, Quat::IDENTITY), |a| (list[a].position, list[a].rotation));
            let pb = b.map_or(p, |b| list[b].position);
            let (ra_v, rb_v) = (p - pa, p - pb);
            let velocity_at = |body: Option<usize>, r: Vec3| body.map_or(Vec3::ZERO, |b| list[b].v + list[b].w.cross(r));
            let vn = (velocity_at(b, rb_v) - velocity_at(a, ra_v)).dot(n);
            let mass_along = |dir: Vec3| {
                let term = |body: Option<usize>, r: Vec3| {
                    body.map_or(0.0, |b| list[b].inv_mass + (list[b].inv_inertia * r.cross(dir)).cross(r).dot(dir))
                };
                let k = term(a, ra_v) + term(b, rb_v);
                if k > 0.0 { 1.0 / k } else { 0.0 }
            };
            // Push apart what overlaps, a little at a time; let what's apart approach.
            let mut target = if depth > 0.005 { 0.2 / dt * (depth - 0.005) } else if depth < 0.0 { depth / dt } else { 0.0 };
            if vn < -1.0 {
                target = target.max(-restitution * vn);
            }
            let local = qa.inverse() * (p - pa);
            let warm = cached
                .and_then(|c| c.iter().find(|c| c.local.distance(local) < 0.05))
                .copied()
                .unwrap_or(CachedPoint { local, normal: 0.0, tangent: [0.0; 2] });
            points.push(ContactPoint {
                ra: ra_v,
                rb: rb_v,
                normal_mass: mass_along(n),
                tangent_mass: [mass_along(t1), mass_along(t2)],
                target,
                normal_impulse: warm.normal * 0.9,
                tangent_impulse: [warm.tangent[0] * 0.9, warm.tangent[1] * 0.9],
                local,
                world: p,
            });
        }
        constraints.push(ContactConstraint { a, b, normal: n, tangents: [t1, t2], friction, points, key, entities: (ra.entity, rb.entity) });
    }
    for c in &constraints {
        for p in &c.points {
            let impulse = c.normal * p.normal_impulse + c.tangents[0] * p.tangent_impulse[0] + c.tangents[1] * p.tangent_impulse[1];
            apply(&mut list, c.a, -impulse, p.ra);
            apply(&mut list, c.b, impulse, p.rb);
        }
    }

    // ---- Joints.
    let mut joint_rows: Vec<JointRow> = joints.iter().filter_map(|j| JointRow::new(j, &index, &list, dt)).collect();
    for row in &mut joint_rows {
        if let Some(a) = row.a {
            list[a].sleeping = false;
        }
        if let Some(b) = row.b {
            list[b].sleeping = false;
        }
    }

    // ---- Solve.
    for _ in 0..world.iterations {
        for row in &mut joint_rows {
            row.solve(&mut list);
        }
        for c in &mut constraints {
            for p in &mut c.points {
                let rel = |list: &[Body]| {
                    let va = c.a.map_or(Vec3::ZERO, |a| list[a].v + list[a].w.cross(p.ra));
                    let vb = c.b.map_or(Vec3::ZERO, |b| list[b].v + list[b].w.cross(p.rb));
                    vb - va
                };
                // Friction first, bounded by last pass's normal impulse.
                for k in 0..2 {
                    let t = c.tangents[k];
                    let vt = rel(&list).dot(t);
                    let limit = c.friction * p.normal_impulse;
                    let new = (p.tangent_impulse[k] - vt * p.tangent_mass[k]).clamp(-limit, limit);
                    let delta = new - p.tangent_impulse[k];
                    p.tangent_impulse[k] = new;
                    apply(&mut list, c.a, -t * delta, p.ra);
                    apply(&mut list, c.b, t * delta, p.rb);
                }
                let vn = rel(&list).dot(c.normal);
                let new = (p.normal_impulse - (vn - p.target) * p.normal_mass).max(0.0);
                let delta = new - p.normal_impulse;
                p.normal_impulse = new;
                apply(&mut list, c.a, -c.normal * delta, p.ra);
                apply(&mut list, c.b, c.normal * delta, p.rb);
            }
        }
    }

    // ---- Remember impulses; report contacts.
    world.contacts.clear();
    for c in &constraints {
        let mut total = 0.0;
        let mut point = Vec3::ZERO;
        new_cache.insert(
            c.key,
            c.points
                .iter()
                .map(|p| {
                    total += p.normal_impulse;
                    point += p.world;
                    CachedPoint { local: p.local, normal: p.normal_impulse, tangent: p.tangent_impulse }
                })
                .collect(),
        );
        if !c.points.is_empty() {
            world.contacts.push(ContactReport {
                a: c.entities.0,
                b: c.entities.1,
                point: point / c.points.len() as f32,
                normal: c.normal,
                impulse: total,
            });
        }
    }
    world.cache = new_cache;

    // ---- Integrate positions and write back.
    for (entity, mut transform, mut body, _) in &mut bodies {
        let Some(&i) = index.get(&entity) else { continue };
        let b = &list[i];
        if b.sleeping {
            body.sleeping = true;
            continue;
        }
        let moving = (b.kind == BodyKind::Kinematic && !b.follows) || b.inv_mass > 0.0;
        if !moving {
            continue;
        }
        transform.translation += b.v * dt;
        if b.w.length_squared() > 1e-12 && !body.lock_rotation {
            let spin = Quat::from_scaled_axis(b.w * dt);
            transform.rotation = (spin * transform.rotation).normalize();
        }
        if b.kind == BodyKind::Dynamic {
            body.linear_velocity = b.v;
            body.angular_velocity = if body.lock_rotation { Vec3::ZERO } else { b.w };
            // Sleep when it has been still a while.
            if b.v.length() < 0.06 && b.w.length() < 0.08 {
                body.idle += dt;
                if body.idle > 0.6 {
                    body.sleeping = true;
                    body.linear_velocity = Vec3::ZERO;
                    body.angular_velocity = Vec3::ZERO;
                }
            } else {
                body.idle = 0.0;
                body.sleeping = false;
            }
        }
    }

    // Keep the colliders (where they now are) for queries.
    for r in &mut records {
        if let Some(b) = r.body {
            let body = &list[b];
            let moved = if (body.kind == BodyKind::Kinematic && !body.follows) || body.inv_mass > 0.0 { body.v * dt } else { Vec3::ZERO };
            r.iso.position += moved;
            r.aabb = r.shape.aabb(&r.iso);
        }
    }
    world.colliders = records;
}

fn push_records(records: &mut Vec<ColliderRecord>, entity: Entity, collider: &Collider, base: &Iso, body: Option<usize>, v: Vec3, dt: f32) {
    for (part, (offset, shape)) in collider.shapes.iter().enumerate() {
        let iso = base.mul(offset);
        let mut aabb = shape.aabb(&iso).expand(MARGIN);
        // Reach ahead along the way it's moving, so fast bodies find what they're about to hit.
        let ahead = v * dt;
        aabb = Aabb { min: aabb.min.min(aabb.min + ahead), max: aabb.max.max(aabb.max + ahead) };
        records.push(ColliderRecord {
            entity,
            shape: shape.clone(),
            iso,
            aabb,
            body,
            friction: collider.friction,
            restitution: collider.restitution,
            sensor: collider.sensor,
            layers: collider.layers,
            mask: collider.mask,
            part: part as u32,
        });
    }
}

fn apply(list: &mut [Body], body: Option<usize>, impulse: Vec3, r: Vec3) {
    if let Some(b) = body {
        let b = &mut list[b];
        if b.inv_mass == 0.0 {
            return;
        }
        b.v += impulse * b.inv_mass;
        b.w += b.inv_inertia * r.cross(impulse);
    }
}

fn skew(v: Vec3) -> Mat3 {
    Mat3::from_cols(Vec3::new(0.0, v.z, -v.y), Vec3::new(-v.z, 0.0, v.x), Vec3::new(v.y, -v.x, 0.0))
}

/// One joint's rows in the solver.
struct JointRow {
    a: Option<usize>,
    b: Option<usize>,
    ra: Vec3,
    rb: Vec3,
    kind: JointKind,
    /// Positional error to correct, per second.
    bias: Vec3,
    point_mass: Mat3,
    /// For hinges and welds: angular error and axes.
    angular_bias: Vec3,
    hinge_axis: Vec3,
    angular_mass: Mat3,
    limit: Option<(f32, f32, f32)>,
    distance_dir: Vec3,
    distance_target: Option<f32>,
}

impl JointRow {
    fn new(j: &Joint, index: &HashMap<Entity, usize>, list: &[Body], dt: f32) -> Option<Self> {
        let a = Some(*index.get(&j.a)?);
        let b = match j.b {
            Some(e) => Some(*index.get(&e)?),
            None => None,
        };
        let (pa, qa) = (list[a?].position, list[a?].rotation);
        let ra = qa * j.anchor_a;
        let (pb, qb) = b.map_or((Vec3::ZERO, Quat::IDENTITY), |b| (list[b].position, list[b].rotation));
        let rb = if b.is_some() { qb * j.anchor_b } else { Vec3::ZERO };
        let world_a = pa + ra;
        let world_b = if b.is_some() { pb + rb } else { j.anchor_b };
        let error = world_b - world_a;
        let inv_i = |body: Option<usize>| body.map_or(Mat3::ZERO, |b| list[b].inv_inertia);
        let inv_m = |body: Option<usize>| body.map_or(0.0, |b| list[b].inv_mass);
        let k = Mat3::from_diagonal(Vec3::splat(inv_m(a) + inv_m(b))) - skew(ra) * inv_i(a) * skew(ra) - skew(rb) * inv_i(b) * skew(rb);
        let point_mass = if k.determinant().abs() > 1e-12 { k.inverse() } else { Mat3::ZERO };
        let ang = inv_i(a) + inv_i(b);
        let angular_mass = if ang.determinant().abs() > 1e-12 { ang.inverse() } else { Mat3::ZERO };
        let mut row = JointRow {
            a,
            b,
            ra,
            rb,
            kind: j.kind,
            bias: error * (0.2 / dt),
            point_mass,
            angular_bias: Vec3::ZERO,
            hinge_axis: Vec3::ZERO,
            angular_mass,
            limit: None,
            distance_dir: Vec3::ZERO,
            distance_target: None,
        };
        match j.kind {
            JointKind::Hinge { axis_a, axis_b, limits } => {
                let (wa, wb) = ((qa * axis_a).normalize(), (qb * axis_b).normalize());
                row.hinge_axis = wa;
                // Turn the axes back into line.
                row.angular_bias = wa.cross(wb) * (0.2 / dt);
                if let Some((lo, hi)) = limits {
                    // The angle about the axis between reference directions perpendicular to it.
                    let refa = qa * axis_a.any_orthonormal_vector();
                    let refb = qb * axis_a.any_orthonormal_vector();
                    let angle = wa.dot(refa.cross(refb)).atan2(refa.dot(refb));
                    row.limit = Some((lo, hi, angle));
                }
            }
            JointKind::Fixed => {
                let rel = qb * qa.inverse();
                let (axis, angle) = rel.to_axis_angle();
                let angle = if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle };
                row.angular_bias = -axis * angle * (0.2 / dt);
            }
            JointKind::Distance { min, max } => {
                let d = error.length();
                row.distance_dir = if d > 1e-6 { error / d } else { Vec3::Y };
                row.distance_target = if d > max { Some(d - max) } else if d < min { Some(d - min) } else { None };
            }
            JointKind::Ball => {}
        }
        Some(row)
    }

    fn velocity(&self, list: &[Body]) -> (Vec3, Vec3) {
        let va = self.a.map_or(Vec3::ZERO, |a| list[a].v + list[a].w.cross(self.ra));
        let vb = self.b.map_or(Vec3::ZERO, |b| list[b].v + list[b].w.cross(self.rb));
        let wa = self.a.map_or(Vec3::ZERO, |a| list[a].w);
        let wb = self.b.map_or(Vec3::ZERO, |b| list[b].w);
        (vb - va, wb - wa)
    }

    fn push(&self, list: &mut [Body], impulse: Vec3, angular: Vec3) {
        if let Some(a) = self.a {
            let b = &mut list[a];
            b.v -= impulse * b.inv_mass;
            b.w -= b.inv_inertia * (self.ra.cross(impulse) + angular);
        }
        if let Some(bi) = self.b {
            let b = &mut list[bi];
            b.v += impulse * b.inv_mass;
            b.w += b.inv_inertia * (self.rb.cross(impulse) + angular);
        }
    }

    fn solve(&mut self, list: &mut [Body]) {
        match self.kind {
            JointKind::Distance { .. } => {
                let Some(err) = self.distance_target else { return };
                let (dv, _) = self.velocity(list);
                let n = self.distance_dir;
                let k = self.distance_mass(list, n);
                let lambda = -(dv.dot(n) + err * 10.0) * k;
                // A rope only pulls; a strut only pushes.
                let lambda = if err > 0.0 { lambda.min(0.0) } else { lambda.max(0.0) };
                self.push(list, n * lambda, Vec3::ZERO);
            }
            _ => {
                let (dv, dw) = self.velocity(list);
                let impulse = self.point_mass * -(dv + self.bias);
                self.push(list, impulse, Vec3::ZERO);
                match self.kind {
                    JointKind::Hinge { .. } => {
                        // Stop any turning off the axis.
                        let (_, dw) = self.velocity(list);
                        let off = dw - self.hinge_axis * dw.dot(self.hinge_axis);
                        let angular = self.angular_mass * -(off + self.angular_bias);
                        self.push(list, Vec3::ZERO, angular);
                        if let Some((lo, hi, angle)) = self.limit {
                            let (_, dw) = self.velocity(list);
                            let spin = dw.dot(self.hinge_axis);
                            let push_back = if angle < lo && spin < 0.0 {
                                -spin + (lo - angle) * 5.0
                            } else if angle > hi && spin > 0.0 {
                                -spin - (angle - hi) * 5.0
                            } else {
                                0.0
                            };
                            if push_back != 0.0 {
                                let k = self.hinge_axis.dot(self.angular_mass * self.hinge_axis);
                                self.push(list, Vec3::ZERO, self.hinge_axis * push_back * k);
                            }
                        }
                    }
                    JointKind::Fixed => {
                        let angular = self.angular_mass * -(dw + self.angular_bias);
                        self.push(list, Vec3::ZERO, angular);
                    }
                    _ => {}
                }
            }
        }
    }

    fn distance_mass(&self, list: &[Body], n: Vec3) -> f32 {
        let term = |body: Option<usize>, r: Vec3| body.map_or(0.0, |b| list[b].inv_mass + (list[b].inv_inertia * r.cross(n)).cross(r).dot(n));
        let k = term(self.a, self.ra) + term(self.b, self.rb);
        if k > 0.0 { 1.0 / k } else { 0.0 }
    }
}
