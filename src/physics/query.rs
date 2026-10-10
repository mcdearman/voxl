//! Asking the world things: what a ray hits, what overlaps a shape; and moving characters
//! through it without passing into walls.

use glam::{Quat, Vec3};

use super::{
    collide::{self, Manifold},
    shape::{Aabb, Iso, Shape},
    PhysicsWorld,
};
use crate::{
    ecs::{Component, Entity, Query, Res},
    time::FixedTime,
    transform::Transform,
};

#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub entity: Entity,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

impl PhysicsWorld {
    /// The first collider a ray meets within `max_distance`, ignoring sensors and `exclude`.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_distance: f32, exclude: Option<Entity>) -> Option<RayHit> {
        self.raycast_where(origin, dir, max_distance, |entity, _| Some(entity) != exclude)
    }

    /// The first solid collider a ray meets that `keep` accepts, given its entity and
    /// whether it belongs to a moving body (not fixed scenery).
    pub fn raycast_where(&self, origin: Vec3, dir: Vec3, max_distance: f32, keep: impl Fn(Entity, bool) -> bool) -> Option<RayHit> {
        let dir = dir.normalize_or(Vec3::NEG_Y);
        let inv = dir.recip();
        let mut best: Option<RayHit> = None;
        for c in &self.colliders {
            if c.sensor || !keep(c.entity, c.body.is_some()) {
                continue;
            }
            let limit = best.map_or(max_distance, |b| b.distance);
            if !matches!(c.shape, Shape::HalfSpace { .. }) && c.aabb.ray(origin, inv, limit).is_none() {
                continue;
            }
            if let Some((t, n)) = c.shape.ray(&c.iso, origin, dir, limit) {
                best = Some(RayHit { entity: c.entity, point: origin + dir * t, normal: n, distance: t });
            }
        }
        best
    }

    /// The height of the ground below `p` (the first surface a ray straight down meets).
    pub fn ground_height(&self, p: Vec3, exclude: Option<Entity>) -> Option<f32> {
        self.raycast(p + Vec3::Y * 2.0, Vec3::NEG_Y, 50.0, exclude).map(|h| h.point.y)
    }

    /// Every entity whose collider overlaps `shape` placed at `iso`.
    pub fn overlaps(&self, shape: &Shape, iso: &Iso, exclude: Option<Entity>) -> Vec<Entity> {
        let bounds = shape.aabb(iso);
        let mut out = Vec::new();
        let mut scratch = Vec::new();
        for c in &self.colliders {
            if Some(c.entity) == exclude || out.contains(&c.entity) || !c.aabb.overlaps(&bounds) {
                continue;
            }
            scratch.clear();
            collide::contact(shape, iso, &c.shape, &c.iso, &mut scratch);
            if scratch.iter().any(|m| m.points.iter().any(|p| p.1 > 0.0)) {
                out.push(c.entity);
            }
        }
        out
    }

    /// Moves `shape` from `iso` by `motion`, sliding along whatever it runs into, and returns
    /// where it ends up and the steepest ground normal it stood on (if any). Solid colliders
    /// that `ignore` returns true for are passed through.
    pub fn slide(&self, shape: &Shape, iso: Iso, motion: Vec3, exclude: Option<Entity>, ignore: impl Fn(Entity) -> bool) -> (Vec3, Option<Vec3>) {
        let mut position = iso.position;
        let mut ground: Option<Vec3> = None;
        // Move in small enough pieces not to pass through thin things.
        let size = match shape {
            Shape::Sphere { radius } | Shape::Capsule { radius, .. } => *radius,
            _ => 0.2,
        };
        let steps = ((motion.length() / (size * 0.5)).ceil() as usize).clamp(1, 16);
        let step = motion / steps as f32;
        let mut scratch: Vec<Manifold> = Vec::new();
        for _ in 0..steps {
            position += step;
            // Push out of everything overlapping, a few passes.
            for _ in 0..4 {
                let here = Iso::new(position, iso.rotation);
                let bounds: Aabb = shape.aabb(&here).expand(0.01);
                let mut pushed = false;
                for c in &self.colliders {
                    if c.sensor || Some(c.entity) == exclude || ignore(c.entity) || !c.aabb.overlaps(&bounds) {
                        continue;
                    }
                    scratch.clear();
                    collide::contact(shape, &here, &c.shape, &c.iso, &mut scratch);
                    for m in &scratch {
                        let depth = m.points.iter().map(|p| p.1).fold(0.0f32, f32::max);
                        if depth > 1e-4 {
                            // The manifold normal points from us to them; move the other way.
                            position -= m.normal * depth;
                            pushed = true;
                            let up = -m.normal;
                            if up.y > 0.5 && ground.is_none_or(|g| up.y > g.y) {
                                ground = Some(up);
                            }
                        }
                    }
                }
                if !pushed {
                    break;
                }
            }
        }
        (position, ground)
    }
}

/// A walking body: a kinematic capsule the game steers by setting `desired_velocity` (the
/// horizontal part is used). It falls, stands on the ground, walks up steps and gentle slopes,
/// and slides along walls. Its entity's translation is the capsule's centre.
#[derive(Clone, Debug, crate::reflect::Reflect)]
#[reflect(name = "mira.CharacterController")]
pub struct CharacterController {
    pub height: f32,
    pub radius: f32,
    pub desired_velocity: Vec3,
    /// Tallest step it walks up without a jump.
    pub step_height: f32,
    pub jump_speed: f32,
    /// Set to jump on the next step, if on the ground.
    pub jump: bool,
    pub grounded: bool,
    pub vertical_velocity: f32,
}

impl Component for CharacterController {}

impl CharacterController {
    pub fn new(height: f32, radius: f32) -> Self {
        Self {
            height,
            radius,
            desired_velocity: Vec3::ZERO,
            step_height: 0.3,
            jump_speed: 4.5,
            jump: false,
            grounded: false,
            vertical_velocity: 0.0,
        }
    }

    pub fn shape(&self) -> Shape {
        Shape::capsule(self.height, self.radius)
    }
}

pub(super) fn move_characters(fixed: Res<FixedTime>, world: Res<PhysicsWorld>, mut characters: Query<(Entity, &mut Transform, &mut CharacterController)>) {
    let dt = fixed.timestep_secs();
    for (entity, mut transform, mut c) in &mut characters {
        let shape = c.shape();
        if c.grounded && c.jump {
            c.vertical_velocity = c.jump_speed;
        }
        c.jump = false;
        c.vertical_velocity += world.gravity.y * dt;
        let horizontal = Vec3::new(c.desired_velocity.x, 0.0, c.desired_velocity.z) * dt;
        let iso = Iso::new(transform.translation, Quat::IDENTITY);
        // On the way up it is not looking for ground to stand on: it goes where its speed
        // takes it, and stops rising if something is overhead.
        if c.vertical_velocity > 0.0 {
            let rise = c.vertical_velocity * dt;
            let (free, _) = world.slide(&shape, iso, horizontal + Vec3::Y * rise, Some(entity), |_| false);
            if free.y - iso.position.y < rise * 0.5 {
                c.vertical_velocity = 0.0;
            }
            transform.translation = free;
            c.grounded = false;
            continue;
        }
        // Lift by the step height, move across, then settle back down: steps are climbed.
        let lifted = Iso::new(iso.position + Vec3::Y * c.step_height, Quat::IDENTITY);
        let (across, _) = world.slide(&shape, lifted, horizontal, Some(entity), |_| false);
        let fall = (c.vertical_velocity * dt - c.step_height).min(-c.step_height);
        let (settled, ground) = world.slide(&shape, Iso::new(across, Quat::IDENTITY), Vec3::Y * fall, Some(entity), |_| false);
        let grounded = ground.is_some() && c.vertical_velocity <= 0.0;
        // Only snap down onto ground within a step; otherwise fall freely from where we were.
        let dropped = across.y - settled.y;
        transform.translation = if grounded || dropped <= c.step_height + 1e-3 {
            settled
        } else {
            let (free, _) = world.slide(&shape, iso, horizontal + Vec3::Y * c.vertical_velocity * dt, Some(entity), |_| false);
            free
        };
        c.grounded = grounded;
        if grounded {
            c.vertical_velocity = 0.0;
        }
    }
}
