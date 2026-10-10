//! Skeletal animation: skeletons and clips loaded from glTF, and an `Animator` that plays and
//! cross-fades clips, working out each frame the matrices that pose a skinned mesh.
//!
//! The meshes themselves are posed on the GPU (see `skin.rs`); this side only samples clips
//! and walks the joint hierarchy, a few hundred matrices a figure.

use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3};

use super::Mesh;
use crate::{assets::Handle, ecs::Component};

/// A joint hierarchy, as the nodes of a glTF file, and the joints a skin binds to.
#[derive(Debug)]
pub struct Skeleton {
    pub names: Vec<String>,
    pub parents: Vec<Option<usize>>,
    /// Each node's own translation, rotation and scale when nothing animates it.
    pub rest: Vec<(Vec3, Quat, Vec3)>,
    /// Nodes, parents before their children.
    pub order: Vec<usize>,
    /// The node of each joint the skin refers to.
    pub joints: Vec<usize>,
    /// From the mesh's bind space into each joint's space.
    pub inverse_bind: Vec<Mat4>,
}

impl Skeleton {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Every node's transform in model space for a local pose.
    pub fn globals(&self, pose: &[(Vec3, Quat, Vec3)]) -> Vec<Mat4> {
        let mut globals = vec![Mat4::IDENTITY; self.names.len()];
        for &node in &self.order {
            let (t, r, s) = pose[node];
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            globals[node] = match self.parents[node] {
                Some(parent) => globals[parent] * local,
                None => local,
            };
        }
        globals
    }

    /// The skinning matrices for a pose: joint space to model space, from bind space.
    pub fn palette(&self, pose: &[(Vec3, Quat, Vec3)], out: &mut Vec<Mat4>) {
        let globals = self.globals(pose);
        out.clear();
        out.extend(self.joints.iter().zip(&self.inverse_bind).map(|(&j, ib)| globals[j] * *ib));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Property {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Debug)]
pub(crate) struct Channel {
    pub node: usize,
    pub property: Property,
    pub times: Vec<f32>,
    /// xyz for translation and scale, xyzw for rotation.
    pub values: Vec<[f32; 4]>,
    pub step: bool,
}

/// One animation: keyframed joint motion, looping over `duration` seconds.
#[derive(Clone, Debug)]
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub(crate) channels: Vec<Channel>,
}

impl AnimationClip {
    /// Takes the steady travel out of a clip that moves over the ground (a walk recorded with
    /// its root motion), so it plays on the spot, and returns how fast it travelled in model
    /// units a second. The travel is found on whichever node's translation drifts furthest.
    pub fn remove_root_motion(&mut self, skeleton: &Skeleton) -> f32 {
        let drift = |c: &Channel| {
            let (a, b) = (c.values[0], c.values[c.values.len() - 1]);
            Vec3::new(b[0] - a[0], b[1] - a[1], b[2] - a[2])
        };
        let Some(index) = (0..self.channels.len())
            .filter(|&i| self.channels[i].property == Property::Translation && self.channels[i].values.len() > 1)
            .max_by(|&a, &b| drift(&self.channels[a]).length().total_cmp(&drift(&self.channels[b]).length()))
        else {
            return 0.0;
        };
        let node = self.channels[index].node;
        // Measure the travel in model space, where the ground is.
        let at = |clip: &AnimationClip, t: f32| {
            let mut pose = skeleton.rest.clone();
            clip.sample(t, &mut pose);
            skeleton.globals(&pose)[node].w_axis.truncate()
        };
        let end = self.duration * 0.9999;
        let travel = at(self, end) - at(self, 0.0);
        let speed = Vec3::new(travel.x, 0.0, travel.z).length() / self.duration.max(1e-3);
        let channel = &mut self.channels[index];
        let d = drift(channel);
        let (t0, t1) = (channel.times[0], *channel.times.last().unwrap());
        for (t, v) in channel.times.iter().zip(channel.values.iter_mut()) {
            let f = (t - t0) / (t1 - t0).max(1e-6);
            v[0] -= d.x * f;
            v[1] -= d.y * f;
            v[2] -= d.z * f;
        }
        speed
    }

    /// A copy that leaves alone the nodes under (and including) `roots`, named `name`: a walk
    /// with its legs taken out, say, for procedural legs to replace.
    pub fn masked(&self, name: &str, skeleton: &Skeleton, roots: &[usize]) -> AnimationClip {
        let under = |mut node: usize| loop {
            if roots.contains(&node) {
                return true;
            }
            match skeleton.parents.get(node).copied().flatten() {
                Some(parent) => node = parent,
                None => return false,
            }
        };
        AnimationClip {
            name: name.to_string(),
            duration: self.duration,
            channels: self.channels.iter().filter(|c| !under(c.node)).cloned().collect(),
        }
    }

    /// Sets the pose's nodes this clip animates to their values at `time` (looped).
    pub fn sample(&self, time: f32, pose: &mut [(Vec3, Quat, Vec3)]) {
        let t = if self.duration > 0.0 { time.rem_euclid(self.duration) } else { 0.0 };
        for c in &self.channels {
            let Some(target) = pose.get_mut(c.node) else { continue };
            let i = c.times.partition_point(|&k| k <= t).clamp(1, c.times.len().max(1)) - 1;
            let j = (i + 1).min(c.times.len() - 1);
            let f = if j == i || c.step { 0.0 } else { ((t - c.times[i]) / (c.times[j] - c.times[i])).clamp(0.0, 1.0) };
            let (a, b) = (c.values[i], c.values[j]);
            match c.property {
                Property::Translation => target.0 = Vec3::new(a[0], a[1], a[2]).lerp(Vec3::new(b[0], b[1], b[2]), f),
                Property::Scale => target.2 = Vec3::new(a[0], a[1], a[2]).lerp(Vec3::new(b[0], b[1], b[2]), f),
                Property::Rotation => {
                    let qa = Quat::from_array(a);
                    let qb = Quat::from_array(b);
                    target.1 = qa.slerp(qb, f).normalize();
                }
            }
        }
    }
}

/// Up to four joints moving each vertex of a skinned mesh, and how much each does.
#[derive(Debug)]
pub struct SkinWeights {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// Shared between an `Animator` and the meshes it poses.
pub type Palette = Arc<Mutex<Vec<Mat4>>>;

/// A mesh posed by a skeleton. The entity's `Mesh3d` must be its own copy of `source`: the GPU
/// writes the posed vertices into it every frame.
pub struct Skinned {
    pub source: Handle<Mesh>,
    pub weights: Arc<SkinWeights>,
    pub palette: Palette,
}
impl Component for Skinned {}

/// Plays clips on a skeleton and keeps its meshes' palette up to date. `play` cross-fades from
/// the current clip to another.
pub struct Animator {
    pub skeleton: Arc<Skeleton>,
    pub clips: Vec<Arc<AnimationClip>>,
    /// How fast clips play: 1 is as authored.
    pub speed: f32,
    pub palette: Palette,
    current: usize,
    time: f32,
    previous: Option<(usize, f32)>,
    fade: f32,
    fade_length: f32,
    /// The pose the clips gave this frame, each node's local translation, rotation and scale;
    /// inverse kinematics and procedural motion adjust it before it is skinned.
    pub pose: Vec<(Vec3, Quat, Vec3)>,
}
impl Component for Animator {}

impl Animator {
    pub fn new(skeleton: Arc<Skeleton>, clips: Vec<Arc<AnimationClip>>) -> Self {
        let palette = Arc::new(Mutex::new(vec![Mat4::IDENTITY; skeleton.joints.len()]));
        Self {
            skeleton,
            clips,
            speed: 1.0,
            palette,
            current: 0,
            time: 0.0,
            previous: None,
            fade: 0.0,
            fade_length: 0.0,
            pose: Vec::new(),
        }
    }

    /// The clip called `name` (glTF exporters often add a suffix, such as the armature's name,
    /// after an underscore).
    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips
            .iter()
            .position(|c| c.name == name)
            .or_else(|| self.clips.iter().position(|c| c.name.strip_prefix(name).is_some_and(|rest| rest.starts_with('_'))))
    }

    /// The name of the clip playing, as passed to `play`.
    pub fn playing(&self) -> &str {
        let name = &self.clips[self.current].name;
        name.split('_').next().unwrap_or(name)
    }

    /// Switches to another clip, blending over `fade` seconds; starts it at `offset` seconds.
    /// Does nothing if it is already playing.
    pub fn play(&mut self, name: &str, fade: f32, offset: f32) {
        let Some(clip) = self.clip(name) else {
            return;
        };
        if clip == self.current && !self.clips.is_empty() {
            return;
        }
        self.previous = Some((self.current, self.time));
        self.current = clip;
        self.time = offset;
        self.fade = 0.0;
        self.fade_length = fade.max(1e-3);
    }

    /// How far into the playing clip the animator is, in seconds.
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Jumps the playing clip to `time` seconds, as when an animation must keep in step with
    /// something else.
    pub fn seek(&mut self, time: f32) {
        self.time = time;
        self.previous = None;
    }

    /// Advances the clips and recomputes the palette.
    pub fn update(&mut self, dt: f32) {
        if self.clips.is_empty() {
            // Nothing to play: the rest pose, for procedural motion to start from each frame.
            self.pose.clone_from(&self.skeleton.rest);
            return;
        }
        let step = dt * self.speed;
        self.time += step;
        let mut pose = self.skeleton.rest.clone();
        self.clips[self.current].sample(self.time, &mut pose);
        if let Some((clip, time)) = self.previous {
            self.fade += dt;
            let w = (self.fade / self.fade_length).min(1.0);
            if w >= 1.0 {
                self.previous = None;
            } else {
                let t = time + step;
                self.previous = Some((clip, t));
                let mut old = self.skeleton.rest.clone();
                self.clips[clip].sample(t, &mut old);
                let w = w * w * (3.0 - 2.0 * w);
                for (p, o) in pose.iter_mut().zip(&old) {
                    p.0 = o.0.lerp(p.0, w);
                    p.1 = o.1.slerp(p.1, w);
                    p.2 = o.2.lerp(p.2, w);
                }
            }
        }
        self.pose = pose;
        self.rebuild();
    }

    /// Recomputes the skinning palette from `pose`, after changing it.
    pub fn rebuild(&mut self) {
        self.ensure_pose();
        let mut palette = self.palette.lock().unwrap_or_else(|e| e.into_inner());
        self.skeleton.palette(&self.pose, &mut palette);
    }

    fn ensure_pose(&mut self) {
        if self.pose.len() != self.skeleton.rest.len() {
            self.pose = self.skeleton.rest.clone();
        }
    }

    /// Every node's transform in model space for the current pose.
    pub fn globals(&self) -> Vec<Mat4> {
        if self.pose.len() != self.skeleton.rest.len() {
            return self.skeleton.globals(&self.skeleton.rest);
        }
        self.skeleton.globals(&self.pose)
    }

    /// Two-bone inverse kinematics: bends the chain `upper` → `lower` → `end` (nodes, each the
    /// parent of the next) so that `end` reaches `target` (model space), the middle joint bending
    /// toward `pole` (a model-space point). `end` keeps its orientation in model space. Call
    /// `rebuild` afterwards.
    pub fn reach(&mut self, upper: usize, lower: usize, end: usize, target: Vec3, pole: Vec3) {
        self.ensure_pose();
        let g = self.globals();
        let (a, b, c) = (g[upper].w_axis.truncate(), g[lower].w_axis.truncate(), g[end].w_axis.truncate());
        let (l1, l2) = ((b - a).length(), (c - b).length());
        if l1 < 1e-6 || l2 < 1e-6 {
            return;
        }
        let to = target - a;
        let dist = to.length().clamp((l1 - l2).abs() + 1e-4, (l1 + l2) * 0.9995);
        let dir = to.normalize_or(Vec3::NEG_Y);
        // The middle joint: on the circle where the two lengths meet, turned toward the pole.
        let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
        let bend = (pole - a) - dir * (pole - a).dot(dir);
        let bend = bend.normalize_or((b - a - dir * (b - a).dot(dir)).normalize_or(dir.any_orthonormal_vector()));
        let knee = a + dir * l1 * cos_a + bend * l1 * (1.0 - cos_a * cos_a).sqrt();
        let reach = a + dir * dist;
        // Turn the upper bone to the knee, then the lower bone to the target.
        let q1 = Quat::from_rotation_arc((b - a).normalize(), (knee - a).normalize());
        let end_global = g[end];
        let set_global = |me: &mut Self, node: usize, global: Mat4, parent_global: Mat4| {
            let local = parent_global.inverse() * global;
            let (_, r, _) = local.to_scale_rotation_translation();
            me.pose[node].1 = r.normalize();
        };
        let parent_of = |n: usize| self.skeleton.parents[n];
        let upper_parent = parent_of(upper).map_or(Mat4::IDENTITY, |p| g[p]);
        let upper_new = Mat4::from_translation(a) * Mat4::from_quat(q1) * Mat4::from_translation(-a) * g[upper];
        set_global(self, upper, upper_new, upper_parent);
        let lower_moved = Mat4::from_translation(a) * Mat4::from_quat(q1) * Mat4::from_translation(-a) * g[lower];
        let c_moved = lower_moved.transform_point3(g[lower].inverse().transform_point3(c));
        let q2 = Quat::from_rotation_arc((c_moved - knee).normalize_or(dir), (reach - knee).normalize_or(dir));
        let lower_new = Mat4::from_translation(knee) * Mat4::from_quat(q2) * Mat4::from_translation(-knee) * lower_moved;
        set_global(self, lower, lower_new, upper_new);
        // The end keeps its model-space orientation, at its new place.
        let end_now = lower_new * (g[lower].inverse() * end_global);
        let (scale, _, translation) = end_now.to_scale_rotation_translation();
        let (_, rotation, _) = end_global.to_scale_rotation_translation();
        let end_new = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        set_global(self, end, end_new, lower_new);
    }

    /// Turns a node by `rotation` in model space, about its own origin (everything under it
    /// turning with it).
    pub fn turn(&mut self, node: usize, rotation: Quat) {
        self.ensure_pose();
        let g = self.globals();
        let parent = self.skeleton.parents[node].map_or(Quat::IDENTITY, |p| g[p].to_scale_rotation_translation().1);
        let r = &mut self.pose[node].1;
        *r = (parent.inverse() * rotation * parent * *r).normalize();
    }

    /// Moves a node by `offset` in model space (keeping everything under it attached).
    pub fn shift(&mut self, node: usize, offset: Vec3) {
        self.ensure_pose();
        let g = self.globals();
        let parent = self.skeleton.parents[node].map_or(Mat4::IDENTITY, |p| g[p]);
        let local_offset = parent.inverse().transform_vector3(offset);
        self.pose[node].0 += local_offset;
    }

    /// How fast a walk clip moves over the ground, in model units a second, from how fast its
    /// planted foot slides back (the clip walks on the spot). `feet` names the two foot joints.
    pub fn ground_speed(&self, name: &str, feet: [&str; 2]) -> Option<f32> {
        let clip = &self.clips[self.clip(name)?];
        let feet = [self.skeleton.find(feet[0])?, self.skeleton.find(feet[1])?];
        let samples = 120;
        let dt = clip.duration / samples as f32;
        let mut previous: Option<[Vec3; 2]> = None;
        let (mut total, mut count) = (0.0, 0);
        for k in 0..=samples {
            let mut pose = self.skeleton.rest.clone();
            clip.sample(k as f32 * dt, &mut pose);
            let globals = self.skeleton.globals(&pose);
            let at = feet.map(|f| globals[f].w_axis.truncate());
            if let Some(before) = previous {
                // The lower foot is the one on the ground.
                let planted = if at[0].y < at[1].y { 0 } else { 1 };
                let slide = -(at[planted].z - before[planted].z) / dt;
                if slide > 0.0 {
                    total += slide;
                    count += 1;
                }
            }
            previous = Some(at);
        }
        (count > 0).then(|| total / count as f32)
    }
}

/// Which clip an animated model plays, as plain data: what a scene file, an editor or an
/// agent can read and set, where an [`Animator`] holds the skeleton and clips themselves.
/// Put it beside a [`Model`](crate::asset_server::Model); the model's animator follows it.
///
/// Change `clip` and the animator cross-fades to it over `fade` seconds. `time` is kept up
/// with how far into the clip the animator is, so a saved scene comes back mid-stride.
#[derive(Clone, Debug, PartialEq, crate::reflect::Reflect)]
#[reflect(name = "mira.Playing", default)]
pub struct Playing {
    /// The clip's name, as in the model file; empty for whichever the file has first.
    pub clip: String,
    /// Seconds into the clip.
    pub time: f32,
    /// How fast it plays: 1 as authored, 0 to hold a pose.
    pub speed: f32,
    /// Seconds a change of clip is blended over.
    pub fade: f32,
    /// Whether the animator has taken up `clip` and `time` since this was made or loaded.
    #[reflect(skip)]
    started: bool,
}

impl Component for Playing {}

impl Default for Playing {
    fn default() -> Self {
        Self { clip: String::new(), time: 0.0, speed: 1.0, fade: 0.25, started: false }
    }
}

impl Playing {
    pub fn new(clip: impl Into<String>) -> Self {
        Self { clip: clip.into(), ..Default::default() }
    }

    /// Starting this many seconds into the clip.
    pub fn at(mut self, time: f32) -> Self {
        self.time = time;
        self
    }

    /// Playing this fast: 1 as authored.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }
}

/// Has each animator play what its entity's [`Playing`] says, and keeps the time there.
pub(crate) fn follow_playing(mut animated: crate::ecs::Query<(&mut Playing, &mut Animator)>) {
    for (mut playing, mut animator) in &mut animated {
        if animator.clips.is_empty() {
            continue;
        }
        animator.speed = playing.speed;
        let named = !playing.clip.is_empty();
        if !playing.started {
            // Fresh from a file, or the model has only now arrived: straight to the moment.
            if named {
                animator.play(&playing.clip, 0.0, playing.time);
            }
            animator.seek(playing.time);
            playing.started = true;
        } else if named && animator.clip(&playing.clip).is_some_and(|clip| clip != animator.current) {
            let fade = playing.fade;
            animator.play(&playing.clip, fade, 0.0);
        }
        playing.time = animator.time();
    }
}

impl Playing {
    /// Has the animator take up the clip and time again: for when its model was replaced.
    pub(crate) fn restart(&mut self) {
        self.started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight leg hanging down: hip at 1 m, knee at 0.5 m, foot at the ground.
    fn leg() -> Animator {
        let skeleton = Skeleton {
            names: vec!["hip".into(), "knee".into(), "foot".into(), "toe".into()],
            parents: vec![None, Some(0), Some(1), Some(2)],
            rest: vec![
                (Vec3::Y, Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(0.0, -0.5, 0.01), Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(0.0, -0.5, -0.01), Quat::IDENTITY, Vec3::ONE),
                (Vec3::new(0.0, 0.0, 0.1), Quat::IDENTITY, Vec3::ONE),
            ],
            order: vec![0, 1, 2, 3],
            joints: vec![0, 1, 2, 3],
            inverse_bind: vec![Mat4::IDENTITY; 4],
        };
        let mut animator = Animator::new(Arc::new(skeleton), Vec::new());
        animator.rebuild();
        animator
    }

    #[test]
    fn reach_puts_the_end_on_target() {
        let mut a = leg();
        let target = Vec3::new(0.0, 0.3, 0.3);
        a.reach(0, 1, 2, target, Vec3::new(0.0, 0.6, 1.0));
        let g = a.globals();
        let end = g[2].w_axis.truncate();
        assert!(end.distance(target) < 1e-3, "end at {end}");
        // The knee bent toward the pole, and the bones kept their lengths.
        let knee = g[1].w_axis.truncate();
        assert!(knee.z > 0.2, "knee at {knee}");
        assert!((knee.distance(Vec3::Y) - Vec3::new(0.0, 0.5, 0.01).length()).abs() < 1e-3);
        // The foot kept its angle: the toe still points straight ahead of it.
        let toe = g[3].w_axis.truncate() - end;
        assert!(toe.normalize().dot(Vec3::Z) > 0.999, "toe along {toe}");
    }

    #[test]
    fn reach_out_of_range_stretches_toward_the_target() {
        let mut a = leg();
        a.reach(0, 1, 2, Vec3::new(3.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 1.0));
        let end = a.globals()[2].w_axis.truncate();
        assert!((end - Vec3::Y).normalize().dot(Vec3::X) > 0.99, "end at {end}");
    }
}
