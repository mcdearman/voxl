//! Procedural walking: feet that plant on the ground and stay put while the body passes over
//! them, then lift and swing to the next footfall, for any number of legs.
//!
//! A `Gait` sits beside an `Animator`. Each frame, after the clips have posed the skeleton, it
//! measures how fast the entity moves and advances a stride cycle by that much; each leg has
//! its own offset into the cycle and spends `duty` of it on the ground. A planted foot holds
//! its place in the world; a lifted one arcs toward where it will next land, a little ahead of
//! its resting place so that it lands in the middle of its sweep. Two-bone inverse kinematics
//! then bends each leg to put its foot there, keeping the foot's own angle from the clip (plus
//! a curl as it swings). The body rises and falls with the steps, and follows the ground when
//! the feet stand at different heights, as on steps and kerbs. Ground comes from rays into the
//! `PhysicsWorld` when there is one, or else is level with the entity.
//!
//! The clip underneath should leave the legs standing (an idle, breathing and looking about);
//! the gait supplies all the leg motion, at whatever speed the entity is moved.

use glam::{Quat, Vec2, Vec3};

use super::{Animator, Skeleton};
use crate::{
    ecs::{Component, Query, Res},
    physics::PhysicsWorld,
    time::Time,
    transform::{GlobalTransform, Parent, Transform},
};

/// Offsets into the stride cycle of a four-beat walk, for legs in the order left hind, left
/// fore, right hind, right fore: each foot lands a quarter-cycle after the one before.
pub const WALK: [f32; 4] = [0.0, 0.25, 0.5, 0.75];
/// A trot, same order: diagonal pairs move together.
pub const TROT: [f32; 4] = [0.0, 0.5, 0.5, 0.0];
/// Two legs, left and right, half a cycle apart.
pub const BIPED: [f32; 2] = [0.0, 0.5];

/// How a set of legs moves at some speed.
#[derive(Clone, Debug)]
pub struct Pattern {
    /// Used from this speed (m/s) up, until a faster pattern takes over.
    pub from_speed: f32,
    /// Ground covered in one full cycle.
    pub stride: f32,
    /// Share of the cycle each foot is on the ground.
    pub duty: f32,
    /// Each leg's offset into the cycle, in the order the legs were added.
    pub phases: Vec<f32>,
    /// How high a foot lifts at the top of its swing (metres).
    pub lift: f32,
    /// The fewest cycles a second while moving at all: slower than `stride` × this, strides
    /// shorten instead.
    pub cadence: f32,
}

impl Pattern {
    pub fn new(from_speed: f32, stride: f32, duty: f32, phases: &[f32], lift: f32) -> Self {
        Self { from_speed, stride, duty, phases: phases.to_vec(), lift, cadence: 0.8 }
    }
}

/// One leg: a two-bone chain `upper` → `lower` → `end` that inverse kinematics bends, and the
/// node at its tip that touches the ground (the end and everything under it keep their angle).
#[derive(Clone, Debug)]
pub struct Leg {
    pub upper: usize,
    pub lower: usize,
    pub end: usize,
    pub tip: usize,
    /// Radians the end turns about the body's sideways axis at the top of the swing: positive
    /// curls a hanging foot back, as a hoof flips up behind.
    pub curl: f32,
    /// Moves the point the middle joint bends toward (model space, from where the clip puts it).
    pub pole: Vec3,
    /// Where the tip sits in the rest pose, in model space.
    rest: Vec3,
    foot: Vec3,
    from: Vec3,
    swing: f32,
    planted: bool,
    ready: bool,
    bend: f32,
}

impl Leg {
    /// A leg by node names; `None` if any is missing.
    pub fn new(skeleton: &Skeleton, upper: &str, lower: &str, end: &str, tip: &str) -> Option<Self> {
        let rest = skeleton.globals(&skeleton.rest);
        let tip = skeleton.find(tip)?;
        Some(Self {
            upper: skeleton.find(upper)?,
            lower: skeleton.find(lower)?,
            end: skeleton.find(end)?,
            tip,
            curl: 0.0,
            pole: Vec3::ZERO,
            rest: rest[tip].w_axis.truncate(),
            foot: Vec3::ZERO,
            from: Vec3::ZERO,
            swing: 0.0,
            planted: true,
            ready: false,
            bend: 0.0,
        })
    }

    pub fn with_curl(mut self, curl: f32) -> Self {
        self.curl = curl;
        self
    }

    /// Where the foot is now, in the world.
    pub fn foot(&self) -> Vec3 {
        self.foot
    }

    pub fn is_planted(&self) -> bool {
        self.planted
    }
}

#[derive(Clone, Debug)]
pub struct Gait {
    pub legs: Vec<Leg>,
    /// Slowest first; the fastest whose `from_speed` the entity exceeds is used.
    pub patterns: Vec<Pattern>,
    /// A node near the root that carries the body (the pelvis): it bobs, and follows the
    /// ground under the feet.
    pub body: Option<usize>,
    /// How far the body dips each footfall, as a share of the lift.
    pub bob: f32,
    /// How long a step takes when standing and shuffling a foot back under the body (seconds).
    pub step_time: f32,
    /// How far (as a share of the stride) a standing foot may be from its resting place before
    /// it steps back.
    pub tolerance: f32,
    /// 0 leaves the clip alone, 1 is fully procedural.
    pub weight: f32,
    /// Take each foot's resting place from the clip rather than the rest pose: the clip sets
    /// the stance and moves the feet, and the gait only keeps a planted foot from sliding and
    /// steps it when the clip (or the body's motion) carries it too far. For characters whose
    /// clips already move their feet.
    pub follow_clip: bool,
    phase: f32,
    speed: f32,
    last: Option<Vec3>,
    body_offset: f32,
}

impl Component for Gait {}

impl Gait {
    pub fn new(pattern: Pattern) -> Self {
        Self {
            legs: Vec::new(),
            patterns: vec![pattern],
            body: None,
            bob: 0.25,
            step_time: 0.3,
            tolerance: 0.25,
            weight: 1.0,
            follow_clip: false,
            phase: 0.0,
            speed: 0.0,
            last: None,
            body_offset: 0.0,
        }
    }

    pub fn with_pattern(mut self, pattern: Pattern) -> Self {
        self.patterns.push(pattern);
        self.patterns.sort_by(|a, b| a.from_speed.total_cmp(&b.from_speed));
        self
    }

    /// Adds a leg if it was found.
    pub fn with_leg(mut self, leg: Option<Leg>) -> Self {
        self.legs.extend(leg);
        self
    }

    pub fn with_body(mut self, skeleton: &Skeleton, name: &str) -> Self {
        self.body = skeleton.find(name);
        self
    }

    /// Four legs, Rocketbox- or 3ds Max biped-style names under a prefix (`"horse "`): hind
    /// legs `L Thigh`/`L Calf`/`L Foot`, fore legs `L UpperArm`/`L Forearm`/`L Hand`, feet
    /// ending at the named tips. Legs are added left hind, left fore, right hind, right fore.
    pub fn quadruped(skeleton: &Skeleton, prefix: &str, hind_tip: &str, fore_tip: &str, walk: Pattern) -> Self {
        let n = |s: &str, side: &str| format!("{prefix}{side} {s}");
        let hind = |side: &str| Leg::new(skeleton, &n("Thigh", side), &n("Calf", side), &n("Foot", side), &n(hind_tip, side)).map(|l| l.with_curl(0.5));
        let fore = |side: &str| Leg::new(skeleton, &n("UpperArm", side), &n("Forearm", side), &n("Hand", side), &n(fore_tip, side)).map(|l| l.with_curl(1.1));
        Gait::new(walk)
            .with_leg(hind("L"))
            .with_leg(fore("L"))
            .with_leg(hind("R"))
            .with_leg(fore("R"))
            .with_body(skeleton, &format!("{prefix}Pelvis"))
    }

    /// Two legs (`L Thigh`/`L Calf`/`L Foot` to the named tip), left then right.
    pub fn biped(skeleton: &Skeleton, prefix: &str, tip: &str, walk: Pattern) -> Self {
        let n = |s: &str, side: &str| format!("{prefix}{side} {s}");
        let leg = |side: &str| Leg::new(skeleton, &n("Thigh", side), &n("Calf", side), &n("Foot", side), &n(tip, side)).map(|l| l.with_curl(0.4));
        Gait::new(walk).with_leg(leg("L")).with_leg(leg("R")).with_body(skeleton, &format!("{prefix}Pelvis"))
    }

    /// How fast the entity was last measured to move over the ground.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Forgets where the feet were, after a teleport.
    pub fn reset(&mut self) {
        self.last = None;
        self.speed = 0.0;
        for leg in &mut self.legs {
            leg.ready = false;
        }
    }

    fn pattern(&self) -> &Pattern {
        self.patterns.iter().rev().find(|p| self.speed >= p.from_speed).unwrap_or(&self.patterns[0])
    }
}

fn ground(physics: Option<&PhysicsWorld>, p: Vec3, level: f32) -> f32 {
    physics
        // Only fixed scenery is ground: not the walker's own collider, nor anyone else's.
        .and_then(|w| w.raycast_where(Vec3::new(p.x, level + 1.0, p.z), Vec3::NEG_Y, 3.0, |_, moving| !moving))
        .filter(|hit| hit.normal.y > 0.3)
        .map_or(level, |hit| hit.point.y)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[allow(clippy::type_complexity)]
pub(super) fn walk(
    time: Res<Time>,
    physics: Option<Res<PhysicsWorld>>,
    mut walkers: Query<(&Transform, Option<&GlobalTransform>, Option<&Parent>, &mut Gait, &mut Animator)>,
) {
    let dt = time.delta_secs().clamp(1e-4, 0.1);
    let physics = physics.as_deref();
    for (transform, global, parent, mut gait, mut animator) in &mut walkers {
        let gait = &mut *gait;
        if gait.legs.is_empty() {
            continue;
        }
        if gait.weight <= 0.0 {
            // Off: forget the feet, so they start afresh from wherever the clip has them.
            gait.reset();
            continue;
        }
        // Where it is in the world: a horse between the shafts goes where its cart goes.
        let model = match (parent, global) {
            (Some(_), Some(global)) => global.0,
            _ => transform.matrix(),
        };
        let inverse = model.inverse();
        let root = model.w_axis.truncate();
        // How fast, and which way, the entity moves.
        let velocity = gait.last.map_or(Vec3::ZERO, |last| (root - last) / dt);
        gait.last = Some(root);
        let flat = Vec3::new(velocity.x, 0.0, velocity.z);
        let measured = if flat.length() > 20.0 { 0.0 } else { flat.length() };
        gait.speed += (measured - gait.speed) * (1.0 - (-dt * 10.0).exp());
        let pattern = gait.pattern().clone();
        let moving = gait.speed > 0.05;
        // Slower, the stride shortens rather than the steps growing slow and long: the cycle
        // never drops below the pattern's cadence.
        let cadence = (gait.speed / pattern.stride.max(0.05)).max(pattern.cadence);
        let stride = gait.speed / cadence;
        if moving {
            gait.phase = (gait.phase + cadence * dt).fract();
        }
        let heading = flat.normalize_or(Vec3::new(model.z_axis.x, 0.0, model.z_axis.z).normalize_or(Vec3::Z));
        let swing_ahead = heading * stride * pattern.duty * 0.5;
        let swinging = gait.legs.iter().filter(|l| !l.planted).count();
        let mut lift_budget = (gait.legs.len() / 2).max(1).saturating_sub(swinging);
        let mut ground_sum = 0.0;
        let mut dip: f32 = 0.0;
        // How far the body must come down for every leg to reach its foot (negative: how far it
        // could rise), from where it was.
        let mut reach_drop = f32::NEG_INFINITY;
        let clip = animator.globals();
        let start = gait.body_offset;

        let count = gait.legs.len();
        for (i, leg) in gait.legs.iter_mut().enumerate() {
            // Its resting place on the ground now, and where it would land from a swing.
            // Standing where the clip stands it (foot locking), or where the rest pose does.
            let rest = model.transform_point3(if gait.follow_clip { clip[leg.tip].w_axis.truncate() } else { leg.rest });
            let lift_above = rest.y - root.y;
            let rest_ground = Vec3::new(rest.x, ground(physics, rest, root.y) + lift_above, rest.z);
            if !leg.ready {
                leg.foot = rest_ground;
                leg.from = rest_ground;
                leg.planted = true;
                leg.ready = true;
            }
            let landing = if moving { rest_ground + swing_ahead } else { rest_ground };
            let landing = Vec3::new(landing.x, ground(physics, landing, root.y) + lift_above, landing.z);
            let phase = (gait.phase + pattern.phases.get(i).copied().unwrap_or(i as f32 / count as f32)).fract();
            // A foot left far behind (the walker was shoved, or teleported) can't be reached:
            // put it back under the body rather than stretch the leg after it.
            if leg.planted && Vec2::new(leg.foot.x - rest_ground.x, leg.foot.z - rest_ground.z).length() > pattern.stride.min(stride.max(0.3) * 1.5).max(0.2) {
                leg.foot = landing;
            }
            if moving {
                let in_swing = phase >= pattern.duty;
                if in_swing && leg.planted {
                    leg.planted = false;
                    leg.from = leg.foot;
                }
                if !in_swing && !leg.planted {
                    leg.planted = true;
                    leg.foot = landing;
                }
                if in_swing {
                    leg.swing = ((phase - pattern.duty) / (1.0 - pattern.duty).max(1e-3)).clamp(0.0, 1.0);
                }
            } else if leg.planted {
                // Standing: step back under the body when turned or pushed out of place.
                let off = Vec3::new(leg.foot.x - rest_ground.x, 0.0, leg.foot.z - rest_ground.z).length();
                if off > pattern.stride * gait.tolerance && lift_budget > 0 {
                    lift_budget -= 1;
                    leg.planted = false;
                    leg.from = leg.foot;
                    leg.swing = 0.0;
                }
            } else {
                leg.swing += dt / gait.step_time.max(0.05);
                if leg.swing >= 1.0 {
                    leg.planted = true;
                    leg.foot = landing;
                }
            }
            let mut curl = 0.0;
            if !leg.planted {
                let s = leg.swing;
                let arc = (s * std::f32::consts::PI).sin();
                let height = if moving { pattern.lift } else { pattern.lift * 0.6 };
                leg.foot = leg.from.lerp(landing, smooth(s)) + Vec3::Y * height * arc;
                curl = leg.curl * arc;
            } else {
                // Weight settling onto a newly planted foot dips the body.
                dip = dip.max(1.0 - (phase / pattern.duty.max(1e-3) * 4.0).min(1.0));
            }
            ground_sum += rest_ground.y - lift_above;
            leg.bend = curl;
            {
                let hip = model.transform_point3(clip[leg.upper].w_axis.truncate()) + Vec3::Y * start;
                let point = |n: usize| model.transform_point3(clip[n].w_axis.truncate());
                let length = point(leg.upper).distance(point(leg.lower)) + point(leg.lower).distance(point(leg.end));
                let ankle = leg.foot + (point(leg.end) - point(leg.tip));
                let across = Vec2::new(ankle.x - hip.x, ankle.z - hip.z).length();
                // As straight as the clip holds it, if straighter than a comfortable stride.
                let reach = (length * 0.97).max(point(leg.upper).distance(point(leg.end)));
                if across < reach {
                    // Crouch a little for a long reach, never a lot: past that the foot steps.
                    reach_drop = reach_drop.max(((hip.y - ankle.y) - (reach * reach - across * across).sqrt()).min(length * 0.06));
                }
            }
        }

        // The body follows the ground beneath the feet, and dips as weight comes onto a foot.
        let mean_ground = ground_sum / count as f32;
        let settle = (mean_ground - root.y) - if moving { pattern.lift * gait.bob * dip } else { 0.0 };
        gait.body_offset += (settle - gait.body_offset) * (1.0 - (-dt * 12.0).exp());
        // Coming down for a long reach can't wait for smoothing, or the foot would slip.
        if reach_drop.is_finite() {
            gait.body_offset = gait.body_offset.min(start - reach_drop);
        }
        if let Some(body) = gait.body {
            let offset = inverse.transform_vector3(Vec3::Y * gait.body_offset * gait.weight);
            animator.shift(body, offset);
        }

        // Bend each leg to its foot.
        let sideways = Vec3::X;
        for leg in &gait.legs {
            let curl = leg.bend;
            let g = animator.globals();
            let (end, tip) = (g[leg.end].w_axis.truncate(), g[leg.tip].w_axis.truncate());
            let bend = Quat::from_axis_angle(sideways, curl * gait.weight);
            let foot_model = inverse.transform_point3(leg.foot);
            let clip_tip = tip;
            let want_tip = clip_tip.lerp(foot_model, gait.weight);
            let want_end = want_tip - bend * (tip - end);
            let knee = g[leg.lower].w_axis.truncate();
            animator.reach(leg.upper, leg.lower, leg.end, want_end, knee + (want_end - end) * 0.5 + leg.pole);
            if curl != 0.0 {
                animator.turn(leg.end, bend);
            }
        }
        animator.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use glam::Mat4;

    use super::*;
    use crate::ecs::{Schedule, World};

    /// A pelvis a metre up with two straight legs (hip, knee, ankle, toe) either side.
    fn walker() -> Arc<Skeleton> {
        let mut names = vec!["Pelvis".to_string()];
        let mut parents = vec![None];
        let mut rest = vec![(Vec3::Y, Quat::IDENTITY, Vec3::ONE)];
        for side in ["L", "R"] {
            let x = if side == "L" { 0.15 } else { -0.15 };
            let base = names.len();
            for (k, (name, t)) in [
                ("Thigh", Vec3::new(x, 0.0, 0.0)),
                ("Calf", Vec3::new(0.0, -0.45, 0.02)),
                ("Foot", Vec3::new(0.0, -0.45, -0.02)),
                ("Toe0", Vec3::new(0.0, -0.08, 0.1)),
            ]
            .into_iter()
            .enumerate()
            {
                names.push(format!("{side} {name}"));
                parents.push(Some(if k == 0 { 0 } else { base + k - 1 }));
                rest.push((t, Quat::IDENTITY, Vec3::ONE));
            }
        }
        let n = names.len();
        Arc::new(Skeleton { names, parents, rest, order: (0..n).collect(), joints: (0..n).collect(), inverse_bind: vec![Mat4::IDENTITY; n] })
    }

    #[test]
    fn planted_feet_stay_put_while_the_body_walks_on() {
        let skeleton = walker();
        let mut world = World::new();
        world.insert_resource(Time::default());
        let gait = Gait::biped(&skeleton, "", "Toe0", Pattern::new(0.0, 1.2, 0.6, &BIPED, 0.1));
        assert_eq!(gait.legs.len(), 2);
        let entity = world.spawn((Transform::IDENTITY, gait, Animator::new(skeleton.clone(), Vec::new())));
        let mut schedule = Schedule::default();
        schedule.add_systems((|mut a: Query<&mut Animator>| {
            for mut a in &mut a {
                a.update(1.0 / 60.0);
            }
        }, walk));
        schedule.initialize(&mut world);
        let frame = |world: &mut World, schedule: &mut Schedule| {
            world.resource_mut::<Time>().advance_by(Duration::from_secs_f32(1.0 / 60.0));
            schedule.run(world);
        };
        // Stand still: the feet settle on the ground under the hips.
        for _ in 0..10 {
            frame(&mut world, &mut schedule);
        }
        let toe = |world: &World, leg: usize| {
            let a = world.get::<Animator>(entity).unwrap();
            let t = world.get::<Transform>(entity).unwrap().matrix();
            let tip = world.get::<Gait>(entity).unwrap().legs[leg].tip;
            t.transform_point3(a.globals()[tip].w_axis.truncate())
        };
        // The toe stands as high above the ground as it does in the rest pose.
        assert!((toe(&world, 0).y - 0.02).abs() < 0.01, "left toe at {}", toe(&world, 0));

        // Walk forward at 1.2 m/s for three seconds, watching the feet.
        let mut planted_drift: f32 = 0.0;
        let mut swings = 0;
        let mut last = [None, None];
        let mut highest: f32 = 0.0;
        for _ in 0..180 {
            world.get_mut::<Transform>(entity).unwrap().translation.z += 1.2 / 60.0;
            frame(&mut world, &mut schedule);
            for (leg, last) in last.iter_mut().enumerate() {
                let planted = world.get::<Gait>(entity).unwrap().legs[leg].is_planted();
                let p = toe(&world, leg);
                highest = highest.max(p.y);
                match (planted, *last) {
                    (true, Some(Some(q))) => planted_drift = planted_drift.max(p.distance(q)),
                    (false, Some(Some(_))) => swings += 1,
                    _ => {}
                }
                *last = Some(planted.then_some(p));
            }
        }
        assert!(planted_drift < 0.01, "a planted foot slid {planted_drift} m");
        assert!(swings >= 4, "only {swings} steps in 3 s");
        assert!(highest > 0.05 && highest < 0.2, "feet lifted {highest} m");
        // And it kept up: the feet are near the body, not left behind.
        let body = world.get::<Transform>(entity).unwrap().translation;
        for leg in 0..2 {
            assert!((toe(&world, leg).z - body.z).abs() < 0.8, "foot {leg} at {} with the body at {body}", toe(&world, leg));
        }
    }
}
