//! The animals' minds. Hens and the rooster scratch and peck about their patch and scatter from
//! anyone who walks too close; geese do the same by the quay; pigs root about slowly; dogs roam
//! the whole square and now and then trot along after someone; the coach horses stand, heads
//! down, flicking their tails.
//!
//! Their legs are the engine's `Gait`: feet planted on the cobbles while the body passes over
//! them, lifted and swung to the next footfall in the order each animal's gait sets (a
//! four-beat walk, a trot for a hurrying dog, a strut or a scurry for the birds), at whatever
//! speed they go. The walk clips still move the rest of them: the nodding head, swaying back
//! and swinging tail.

use std::{
    f32::consts::{PI, TAU},
    sync::Arc,
};

use voxl::{
    glam::Vec2,
    prelude::*,
    render::gait::{Gait, Pattern, BIPED, TROT, WALK},
};

use crate::{crowd::Person, square, traffic::Traffic};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Bird,
    Pig,
    Dog,
    /// Stands where it is put.
    Tethered,
}

impl Kind {
    pub fn of(model: &str) -> Self {
        match model {
            m if m.starts_with("hen") || m == "rooster" || m == "goose" => Kind::Bird,
            "pig" => Kind::Pig,
            "beagle" | "shepherd" => Kind::Dog,
            _ => Kind::Tethered,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mind {
    Wander { to: Vec2 },
    Idle { until: f32 },
    Follow { who: Entity, until: f32 },
    Flee { from: Vec2, until: f32 },
}

pub struct Beast {
    pub kind: Kind,
    home: Vec2,
    range: f32,
    pub position: Vec2,
    heading: f32,
    mind: Mind,
    /// How fast its walk clip covers the ground.
    clip_pace: f32,
    pace: f32,
    rng: u32,
}
impl Component for Beast {}

impl Beast {
    pub fn new(kind: Kind, home: Vec2, range: f32, seed: u32, clip_pace: f32) -> Self {
        let mut beast = Self {
            kind,
            home,
            range,
            position: home,
            heading: 0.0,
            mind: Mind::Idle { until: 0.0 },
            clip_pace,
            pace: clip_pace,
            rng: seed.wrapping_mul(2654435761).max(1),
        };
        beast.heading = beast.random() * TAU;
        beast.pace = clip_pace * (0.9 + beast.random() * 0.2);
        beast.mind = Mind::Idle { until: beast.random() * 4.0 };
        beast
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }

    fn somewhere(&mut self) -> Vec2 {
        for _ in 0..20 {
            let a = self.random() * TAU;
            let r = self.random().sqrt() * self.range;
            let p = self.home + Vec2::new(a.cos(), a.sin()) * r;
            let inside = p.x > square::WALK_MIN.x + 0.5 && p.x < square::WALK_MAX.x - 0.5 && p.y > square::WALK_MIN.y + 0.5 && p.y < square::WALK_MAX.y - 0.5;
            if inside && square::obstacles().iter().all(|(c, r)| p.distance(*c) > r + 0.5) {
                return p;
            }
        }
        self.home
    }
}

pub fn think(
    time: Res<Time>,
    traffic: Option<Res<Traffic>>,
    mut beasts: Query<(&mut Beast, &mut Transform, &mut Animator)>,
    people: Query<(Entity, &Person)>,
) {
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    let crowd: Vec<(Entity, Vec2)> = people.iter().map(|(e, p)| (e, p.position)).collect();
    let mut obstacles = square::obstacles();
    if let Some(traffic) = &traffic {
        obstacles.extend_from_slice(&traffic.0);
    }
    for (mut beast, mut transform, mut animator) in &mut beasts {
        let here = beast.position;
        let nearest = crowd
            .iter()
            .map(|(e, p)| (*e, *p, p.distance(here)))
            .min_by(|a, b| a.2.total_cmp(&b.2));
        // Birds scatter from anyone coming too close, and from carts.
        if beast.kind == Kind::Bird && !matches!(beast.mind, Mind::Flee { .. }) {
            let threat = nearest.filter(|n| n.2 < 1.4).map(|n| n.1).or_else(|| obstacles.iter().find(|(c, r)| c.distance(here) < r + 1.0).map(|o| o.0));
            if let Some(from) = threat {
                beast.mind = Mind::Flee { from, until: now + 0.9 + beast.random() * 0.5 };
            }
        }
        let mut target: Option<Vec2> = None;
        let mut speed = beast.pace;
        match beast.mind {
            Mind::Wander { to } => {
                if here.distance(to) < 0.4 {
                    beast.mind = Mind::Idle { until: now + idle_time(&mut beast) };
                } else {
                    target = Some(to);
                }
            }
            Mind::Idle { until } => {
                if beast.kind != Kind::Tethered && now > until {
                    let roll = beast.random();
                    let company = nearest.filter(|n| n.2 < 10.0);
                    beast.mind = match (beast.kind, company) {
                        (Kind::Dog, Some((who, _, _))) if roll < 0.35 => Mind::Follow { who, until: now + 10.0 + beast.random() * 15.0 },
                        _ => Mind::Wander { to: beast.somewhere() },
                    };
                }
            }
            Mind::Follow { who, until } => {
                match crowd.iter().find(|(e, _)| *e == who) {
                    Some((_, p)) if now < until => {
                        // Trot at the person's heel.
                        if here.distance(*p) > 1.6 {
                            target = Some(*p);
                            speed = beast.pace * 1.4;
                        }
                    }
                    _ => beast.mind = Mind::Idle { until: now + 3.0 },
                }
            }
            Mind::Flee { from, until } => {
                if now > until {
                    beast.mind = Mind::Idle { until: now + 2.0 + beast.random() * 3.0 };
                } else {
                    target = Some(here + (here - from).normalize_or(Vec2::X) * 2.0);
                    speed = beast.pace * 2.4;
                }
            }
        }

        let mut moving = false;
        if let Some(to) = target {
            let mut want = (to - here).normalize_or_zero();
            for (center, radius) in &obstacles {
                let away = here - *center;
                let gap: f32 = away.length() - radius;
                if gap < 1.0 {
                    let out = away.normalize_or_zero();
                    want += out * ((1.0 - gap) * 1.5).clamp(0.0, 2.0);
                }
            }
            if want.length() > 1e-3 {
                let desired = want.x.atan2(want.y);
                let off = (desired - beast.heading + PI).rem_euclid(TAU) - PI;
                beast.heading += off.clamp(-4.0 * dt, 4.0 * dt);
                let step = Vec2::new(beast.heading.sin(), beast.heading.cos()) * speed * (1.0 - (off.abs() / PI) * 0.7) * dt;
                let next = here + step;
                if next.x > square::WALK_MIN.x && next.x < square::WALK_MAX.x && next.y > square::WALK_MIN.y && next.y < square::WALK_MAX.y {
                    beast.position = next;
                }
                moving = true;
            }
        } else if let Mind::Follow { who, .. } = beast.mind {
            // Waiting at heel: face the person.
            if let Some((_, p)) = crowd.iter().find(|(e, _)| *e == who) {
                let to = *p - here;
                beast.heading += ((to.x.atan2(to.y) - beast.heading + PI).rem_euclid(TAU) - PI).clamp(-2.0 * dt, 2.0 * dt);
            }
        }
        if moving {
            animator.speed = speed / beast.clip_pace.max(0.05);
            let clip = walk_clip(&animator);
            animator.play(clip, 0.25, 0.0);
        } else {
            animator.speed = 1.0;
            let offset = beast.random() * 4.0;
            animator.play("idle", 0.4, offset);
        }
        transform.translation = Vec3::new(beast.position.x, 0.0, beast.position.y);
        transform.rotation = Quat::from_rotation_y(beast.heading);
    }
}

fn idle_time(beast: &mut Beast) -> f32 {
    match beast.kind {
        Kind::Bird => 2.0 + beast.random() * 5.0,
        Kind::Pig => 4.0 + beast.random() * 8.0,
        Kind::Dog => 1.5 + beast.random() * 5.0,
        Kind::Tethered => 1e9,
    }
}

/// The clip to walk with: the walk without its legs when the gait moves them.
pub fn walk_clip(animator: &Animator) -> &'static str {
    if animator.clip("walk_body").is_some() { "walk_body" } else { "walk" }
}

/// What each rig calls its bones: the animal's name, then the bone's.
pub fn prefix(model: &str) -> String {
    match model {
        m if m.starts_with("horse") => "horse ".to_string(),
        m if m.starts_with("hen") => "chicken ".to_string(),
        "shepherd" => "dog ".to_string(),
        m => format!("{m} "),
    }
}

/// Gives an animal procedural legs: a `Gait` suited to it, with strides as long as its walk
/// clip's (so the head still nods in time), and that clip with the legs taken out.
pub fn give_legs(world: &mut World, root: Entity, model: &str, pace: f32) {
    let Some(animator) = world.get::<Animator>(root) else { return };
    let skeleton = animator.skeleton.clone();
    let Some(walk) = animator.clip("walk").map(|i| animator.clips[i].clone()) else { return };
    let stride = pace * walk.duration;
    let p = prefix(model);
    let gait = if p == "chicken " || p == "rooster " || p == "goose " {
        // A strut, and a scurry when frightened.
        Gait::biped(&skeleton, &p, "Toe0", Pattern::new(0.0, stride, 0.6, &BIPED, 0.035))
            .with_pattern(Pattern::new(pace * 1.6, stride * 1.6, 0.42, &BIPED, 0.05))
    } else {
        let (hind, fore, lift) = match p.as_str() {
            "horse " => ("Toe01", "Finger01", 0.13),
            "pig " => ("Toe0", "Finger0", 0.05),
            _ => ("Toe0", "Finger0", 0.07),
        };
        let gait = Gait::quadruped(&skeleton, &p, hind, fore, Pattern::new(0.0, stride, 0.62, &WALK, lift));
        // Dogs break into a trot to keep up with someone.
        if Kind::of(model) == Kind::Dog { gait.with_pattern(Pattern::new(pace * 1.25, stride * 1.35, 0.45, &TROT, lift * 1.3)) } else { gait }
    };
    if gait.legs.is_empty() {
        log::warn!("{model}: no legs found for its gait");
        return;
    }
    let legs: Vec<usize> = gait.legs.iter().map(|l| l.upper).collect();
    let body = walk.masked("walk_body", &skeleton, &legs);
    if let Some(animator) = world.get_mut::<Animator>(root) {
        animator.clips.push(Arc::new(body));
    }
    world.insert(root, gait);
}

/// A body for the physics, to shove what it walks into: roughly its trunk, off the ground.
pub fn collider(kind: Kind, model: &str) -> Collider {
    match kind {
        Kind::Bird => Collider::sphere(0.15).at(Vec3::Y * 0.22),
        Kind::Pig => Collider::cuboid(Vec3::new(0.24, 0.26, 0.5)).at(Vec3::Y * 0.4),
        Kind::Dog => Collider::cuboid(Vec3::new(0.14, 0.22, 0.38)).at(Vec3::Y * 0.38),
        Kind::Tethered if model.starts_with("horse") => Collider::cuboid(Vec3::new(0.32, 0.45, 1.05)).at(Vec3::Y * 1.2),
        Kind::Tethered => Collider::sphere(0.4).at(Vec3::Y * 0.5),
    }
}

impl Beast {
    /// Turns it to face a way, as a yaw about the vertical.
    pub fn face(&mut self, yaw: f32) {
        self.heading = yaw;
    }
}
