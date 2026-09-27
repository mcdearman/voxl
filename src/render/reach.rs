//! Limbs reaching for things: a hand onto a wheel, a handle, a rammer staff; a foot onto a
//! step. Each `Limb` is a two-bone chain bent by inverse kinematics so its end meets a target
//! in the world, blended in and out over the clip underneath, so a keyed or captured action
//! lands its hands where the prop actually is.

use glam::Vec3;

use super::{Animator, Skeleton};
use crate::{
    ecs::{Component, Query, Res},
    time::Time,
    transform::{GlobalTransform, Parent, Transform},
};

#[derive(Clone, Debug)]
pub struct Limb {
    pub upper: usize,
    pub lower: usize,
    pub end: usize,
    /// Where the end should be, in the world; `None` leaves it to the clip.
    pub target: Option<Vec3>,
    /// How far toward the target, 0 to 1.
    pub weight: f32,
    /// Which way (in the world) the middle joint should bend, added to where the clip puts it:
    /// an elbow out and down, say. Zero keeps the clip's bend.
    pub bend: Vec3,
    blend: f32,
    last: Vec3,
}

impl Limb {
    /// A limb by node names; `None` if any is missing.
    pub fn new(skeleton: &Skeleton, upper: &str, lower: &str, end: &str) -> Option<Self> {
        Some(Self {
            upper: skeleton.find(upper)?,
            lower: skeleton.find(lower)?,
            end: skeleton.find(end)?,
            target: None,
            weight: 1.0,
            bend: Vec3::ZERO,
            blend: 0.0,
            last: Vec3::ZERO,
        })
    }

    /// Reaches for `target` (world space), or lets go with `None`.
    pub fn reach(&mut self, target: Option<Vec3>) {
        self.target = target;
    }
}

/// Limbs that reach for targets. Changes of target and weight are eased over `ease` seconds.
#[derive(Clone, Debug, Default)]
pub struct Reach {
    pub limbs: Vec<Limb>,
    pub ease: f32,
}

impl Component for Reach {}

impl Reach {
    pub fn new() -> Self {
        Self { limbs: Vec::new(), ease: 0.25 }
    }

    pub fn with_limb(mut self, limb: Option<Limb>) -> Self {
        self.limbs.extend(limb);
        self
    }

    /// Both arms of a 3ds Max biped- or Rocketbox-style rig (`L UpperArm`, `L Forearm`,
    /// `L Hand` under a prefix such as `"Bip01 "`), left then right.
    pub fn arms(skeleton: &Skeleton, prefix: &str) -> Self {
        let arm = |side: &str| Limb::new(skeleton, &format!("{prefix}{side} UpperArm"), &format!("{prefix}{side} Forearm"), &format!("{prefix}{side} Hand"));
        Self::new().with_limb(arm("L")).with_limb(arm("R"))
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn reach(time: Res<Time>, mut reachers: Query<(&Transform, Option<&GlobalTransform>, Option<&Parent>, &mut Reach, &mut Animator)>) {
    let dt = time.delta_secs().clamp(0.0, 0.1);
    for (transform, global, parent, mut reach, mut animator) in &mut reachers {
        let reach = &mut *reach;
        let model = match (parent, global) {
            (Some(_), Some(global)) => global.0,
            _ => transform.matrix(),
        };
        let inverse = model.inverse();
        let rate = 1.0 - (-dt / reach.ease.max(1e-3) * 3.0).exp();
        let mut changed = false;
        for limb in &mut reach.limbs {
            let goal = if limb.target.is_some() { limb.weight.clamp(0.0, 1.0) } else { 0.0 };
            limb.blend += (goal - limb.blend) * rate;
            if let Some(t) = limb.target {
                // Follow a moving target closely, but ease onto a new one.
                limb.last = if limb.blend < 0.02 { t } else { limb.last.lerp(t, rate.max(0.5)) };
            }
            if limb.blend < 1e-3 {
                continue;
            }
            let g = animator.globals();
            let end = g[limb.end].w_axis.truncate();
            let elbow = g[limb.lower].w_axis.truncate();
            let want = end.lerp(inverse.transform_point3(limb.last), limb.blend);
            let pole = elbow + (want - end) * 0.5 + inverse.transform_vector3(limb.bend);
            animator.reach(limb.upper, limb.lower, limb.end, want, pole);
            changed = true;
        }
        if changed {
            animator.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use glam::{Mat4, Quat};

    use super::*;
    use crate::ecs::{Schedule, World};

    #[test]
    fn a_hand_eases_onto_its_target_and_lets_go() {
        // A shoulder 1.5 m up with an arm hanging down; the figure stands at x = 2.
        let skeleton = Arc::new(Skeleton {
            names: vec!["root".into(), "Bip01 R UpperArm".into(), "Bip01 R Forearm".into(), "Bip01 R Hand".into()],
            parents: vec![None, Some(0), Some(1), Some(2)],
            rest: vec![
                (Vec3::Y * 1.5, Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(-0.2, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(0.0, -0.3, -0.01), Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(0.0, -0.28, 0.01), Quat::IDENTITY, Vec3::ONE),
            ],
            order: vec![0, 1, 2, 3],
            joints: vec![0, 1, 2, 3],
            inverse_bind: vec![Mat4::IDENTITY; 4],
        });
        let mut world = World::new();
        world.insert_resource(Time::default());
        let mut reach = Reach::arms(&skeleton, "Bip01 ");
        assert_eq!(reach.limbs.len(), 1);
        let target = Vec3::new(1.6, 1.3, -0.35);
        reach.limbs[0].reach(Some(target));
        let e = world.spawn((Transform::from_xyz(2.0, 0.0, 0.0), reach, Animator::new(skeleton.clone(), Vec::new())));
        let mut schedule = Schedule::default();
        schedule.add_systems((|mut a: Query<&mut Animator>| for mut a in &mut a { a.update(1.0 / 60.0) }, super::reach));
        schedule.initialize(&mut world);
        let hand = |world: &World| world.get::<Transform>(e).unwrap().matrix().transform_point3(world.get::<Animator>(e).unwrap().globals()[3].w_axis.truncate());
        let mut run = |world: &mut World, frames: usize| {
            for _ in 0..frames {
                world.resource_mut::<Time>().advance_by(Duration::from_secs_f32(1.0 / 60.0));
                schedule.run(world);
            }
        };
        run(&mut world, 2);
        assert!(hand(&world).distance(target) > 0.1, "should ease in, not snap");
        run(&mut world, 60);
        assert!(hand(&world).distance(target) < 0.01, "hand at {}", hand(&world));
        world.get_mut::<Reach>(e).unwrap().limbs[0].reach(None);
        run(&mut world, 60);
        assert!(hand(&world).distance(Vec3::new(1.8, 0.92, 0.0)) < 0.02, "should hang again, at {}", hand(&world));
    }
}
