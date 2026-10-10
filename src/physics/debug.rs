//! Physics drawn as lines: what the solver sees, over what the renderer shows.

use glam::{Mat4, Quat, Vec3};

use super::{BodyKind, Collider, Joint, PhysicsWorld, RigidBody, Shape};
use crate::{
    ecs::{Query, Res, ResMut},
    reflect::Reflect,
    render::{Color, DebugLines},
    transform::GlobalTransform,
};

/// What of the physics to draw over the scene, as lines. Everything is off to begin with.
///
/// Colliders are coloured by what moves them: green for a dynamic body, dark green once it has
/// fallen asleep, yellow for a kinematic one, white for one that never moves, and purple
/// for a sensor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(name = "mira.PhysicsDebug", default)]
pub struct PhysicsDebug {
    /// The shape of every collider.
    pub colliders: bool,
    /// Where bodies touched in the last step, with an arrow along each contact's normal.
    pub contacts: bool,
    /// An arrow from each moving body as far as it goes in a tenth of a second.
    pub velocities: bool,
    /// A line between the two anchors of every joint.
    pub joints: bool,
}

impl PhysicsDebug {
    /// Everything shown.
    pub const ALL: Self = Self { colliders: true, contacts: true, velocities: true, joints: true };

    fn any(&self) -> bool {
        self.colliders || self.contacts || self.velocities || self.joints
    }
}

/// More triangles than this and a mesh collider is drawn as its bounds instead.
const MOST_TRIANGLES: usize = 4000;

const DYNAMIC: Color = Color::rgb(0.2, 0.9, 0.3);
const ASLEEP: Color = Color::rgb(0.1, 0.3, 0.12);
const KINEMATIC: Color = Color::rgb(0.95, 0.8, 0.15);
const FIXED: Color = Color::rgb(0.9, 0.9, 0.9);
const SENSOR: Color = Color::rgb(0.8, 0.3, 0.95);
const CONTACT: Color = Color::rgb(1.0, 0.25, 0.2);
const VELOCITY: Color = Color::rgb(0.2, 0.7, 1.0);
const JOINT: Color = Color::rgb(1.0, 0.5, 0.1);

/// Where an entity is as physics has it: placed and turned, never scaled.
fn placed(at: &GlobalTransform) -> (Vec3, Quat) {
    let (_, rotation, position) = at.0.to_scale_rotation_translation();
    (position, rotation)
}

/// The lines of one shape, placed and turned.
pub(crate) fn outline(lines: &mut DebugLines, shape: &Shape, position: Vec3, rotation: Quat, color: Color) {
    match shape {
        Shape::Sphere { radius } => lines.sphere(position, rotation, *radius, color),
        Shape::Cuboid { half } => {
            lines.cuboid(Mat4::from_rotation_translation(rotation, position), *half, color);
        }
        Shape::Capsule { half_height, radius } => {
            lines.capsule(position, rotation, *half_height, *radius, color);
        }
        Shape::HalfSpace { normal } => {
            let normal = rotation * *normal;
            lines.grid(position, normal, 2.0, 4, color);
            lines.arrow(position, position + normal.normalize_or_zero(), color);
        }
        Shape::TriMesh(mesh) => {
            let at = |index: u32| position + rotation * mesh.vertices[index as usize];
            if mesh.triangles.len() > MOST_TRIANGLES {
                let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
                for vertex in &mesh.vertices {
                    let vertex = position + rotation * *vertex;
                    (min, max) = (min.min(vertex), max.max(vertex));
                }
                lines.bounds(min, max, color);
                return;
            }
            for [a, b, c] in &mesh.triangles {
                lines.path(&[at(*a), at(*b), at(*c)], true, color);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn draw(
    wanted: Res<PhysicsDebug>,
    lines: Option<ResMut<DebugLines>>,
    world: Res<PhysicsWorld>,
    colliders: Query<(&GlobalTransform, &Collider, Option<&RigidBody>)>,
    bodies: Query<(&GlobalTransform, &RigidBody)>,
    joints: Query<&Joint>,
    places: Query<&GlobalTransform>,
) {
    let Some(mut lines) = lines else { return };
    if !wanted.any() {
        return;
    }
    if wanted.colliders {
        for (at, collider, body) in &colliders {
            let color = match body {
                _ if collider.sensor => SENSOR,
                None => FIXED,
                Some(body) if body.kind == BodyKind::Kinematic => KINEMATIC,
                Some(body) if body.sleeping => ASLEEP,
                Some(_) => DYNAMIC,
            };
            let (position, rotation) = placed(at);
            for (within, shape) in &collider.shapes {
                outline(&mut lines, shape, position + rotation * within.position, rotation * within.rotation, color);
            }
        }
    }
    if wanted.velocities {
        for (at, body) in &bodies {
            if body.linear_velocity.length_squared() > 1e-4 {
                let from = at.translation();
                lines.arrow(from, from + body.linear_velocity * 0.1, VELOCITY);
            }
        }
    }
    if wanted.contacts {
        for contact in &world.contacts {
            lines.sphere(contact.point, Quat::IDENTITY, 0.04, CONTACT);
            lines.arrow(contact.point, contact.point + contact.normal * 0.3, CONTACT);
        }
    }
    if wanted.joints {
        for joint in &joints {
            let Some(a) = places.get(joint.a) else { continue };
            let (position, rotation) = placed(a);
            let from = position + rotation * joint.anchor_a;
            // With no second body the other anchor is a place in the world.
            let to = match joint.b.map(|b| places.get(b)) {
                Some(Some(b)) => {
                    let (position, rotation) = placed(b);
                    position + rotation * joint.anchor_b
                }
                Some(None) => continue,
                None => joint.anchor_b,
            };
            lines.line(position, from, JOINT);
            lines.line(from, to, JOINT);
            lines.sphere(from, Quat::IDENTITY, 0.05, JOINT);
            lines.sphere(to, Quat::IDENTITY, 0.05, JOINT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ecs::{Schedule, World},
        transform::Transform,
    };

    fn drawn(wanted: PhysicsDebug, spawn: impl FnOnce(&mut World)) -> Vec<(Vec3, Vec3)> {
        let mut world = World::new();
        world.insert_resource(wanted);
        world.insert_resource(PhysicsWorld::default());
        world.insert_resource(DebugLines::default());
        spawn(&mut world);
        let mut schedule = Schedule::default();
        schedule.add_systems(draw);
        schedule.initialize(&mut world);
        schedule.run(&mut world);
        world.resource::<DebugLines>().segments().collect()
    }

    fn ball(world: &mut World) {
        let at = Vec3::new(4.0, 2.0, 0.0);
        world.spawn((
            Transform::from_translation(at),
            GlobalTransform(Mat4::from_translation(at)),
            Collider::sphere(0.5).at(Vec3::Y),
            RigidBody::dynamic().with_velocity(Vec3::X * 10.0),
        ));
    }

    #[test]
    fn nothing_is_drawn_until_it_is_asked_for() {
        assert!(drawn(PhysicsDebug::default(), ball).is_empty());
    }

    #[test]
    fn a_collider_is_drawn_where_physics_has_it() {
        let wanted = PhysicsDebug { colliders: true, ..Default::default() };
        let lines = drawn(wanted, ball);
        assert!(!lines.is_empty());
        // The shape sits a metre above its entity, and every line is on the ball's surface.
        let centre = Vec3::new(4.0, 3.0, 0.0);
        assert!(lines.iter().all(|(a, b)| {
            (a.distance(centre) - 0.5).abs() < 1e-3 && (b.distance(centre) - 0.5).abs() < 1e-3
        }));
    }

    #[test]
    fn a_moving_body_gets_an_arrow_a_tenth_of_a_second_long() {
        let wanted = PhysicsDebug { velocities: true, ..Default::default() };
        let lines = drawn(wanted, ball);
        let from = Vec3::new(4.0, 2.0, 0.0);
        assert!(lines.contains(&(from, from + Vec3::X)));
    }
}
