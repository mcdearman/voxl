//! The townsfolk's minds: where each goes, who stops to talk to whom, and how they walk there
//! without bumping into the fountain, the stalls, the carts or each other.
//!
//! Everyone is a small state machine. Strollers wander from place to place, pause to look
//! about, and fall into conversation with whoever is near; market women keep to their stalls;
//! water-carriers go back and forth between the fountain and the shops; the Guard holds its
//! post. Conversations are in pairs: they face each other and take turns to talk, with the
//! gestures of the talk clips, while the other listens.

use std::f32::consts::{PI, TAU};

use mira::{glam::Vec2, prelude::*};

use crate::{square, traffic::Traffic};

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    Stroller,
    Guard,
    /// Keeps near a stall.
    Vendor { home: Vec2 },
    /// Carries water from the fountain to the shops.
    Carrier,
    /// Runs about and plays tag with the other children.
    Child,
}

#[derive(Clone, Copy, PartialEq)]
enum Mind {
    Walk { to: Vec2 },
    /// Walking over to someone to talk.
    Approach { partner: Entity },
    Idle { until: f32 },
    Talk { partner: Entity, until: f32 },
    /// Running after another child to tag them.
    Chase { target: Entity, until: f32 },
}

pub struct Person {
    pub role: Role,
    mind: Mind,
    pub position: Vec2,
    /// The way they face, as an angle about the vertical.
    heading: f32,
    /// How fast they like to walk, and how fast their walk clip moves (to scale it by).
    pace: f32,
    clip_pace: f32,
    /// Ladies and older men stroll; others walk.
    walk_clip: &'static str,
    speaking: bool,
    next_turn: f32,
    /// Running rather than walking, and how fast; how fast the run clip covers the ground.
    running: bool,
    run_speed: f32,
    run_pace: f32,
    /// Picks which talk and idle clips this person uses.
    style: usize,
    rng: u32,
}
impl Component for Person {}

impl Person {
    #[allow(clippy::too_many_arguments)]
    pub fn new(role: Role, position: Vec2, facing: Vec2, seed: u32, walking: bool, stroll: bool, clip_pace: f32, run_pace: f32) -> Self {
        let mut person = Self {
            role,
            mind: Mind::Idle { until: 0.0 },
            position,
            heading: facing.x.atan2(facing.y),
            pace: 0.0,
            clip_pace,
            walk_clip: if stroll { "stroll" } else { "walk" },
            speaking: false,
            next_turn: 0.0,
            running: false,
            run_speed: 0.0,
            run_pace,
            style: seed as usize,
            rng: seed.wrapping_mul(2654435761).max(1),
        };
        // Near the pace the clip was recorded at, so strides stay natural.
        person.pace = clip_pace * (0.95 + person.random() * 0.15);
        person.run_speed = run_pace * (0.9 + person.random() * 0.2);
        person.mind = if walking {
            Mind::Walk { to: person.destination() }
        } else {
            Mind::Idle { until: 2.0 + person.random() * 10.0 }
        };
        person
    }

    fn random(&mut self) -> f32 {
        // xorshift32
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }

    fn facing(&self) -> Vec2 {
        Vec2::new(self.heading.sin(), self.heading.cos())
    }

    /// Somewhere to go next.
    fn destination(&mut self) -> Vec2 {
        match self.role {
            Role::Vendor { home } => home + Vec2::new(self.random() - 0.5, self.random() - 0.5) * 5.0,
            Role::Child => {
                // Somewhere near: children dart about rather than cross the square.
                for _ in 0..20 {
                    let a = self.random() * TAU;
                    let p = self.position + Vec2::new(a.cos(), a.sin()) * (3.0 + self.random() * 8.0);
                    let inside = p.x > square::WALK_MIN.x + 1.0 && p.x < square::WALK_MAX.x - 1.0 && p.y > square::WALK_MIN.y + 1.0 && p.y < square::WALK_MAX.y - 1.0;
                    if inside && square::obstacles().iter().all(|(c, r)| p.distance(*c) > r + 0.8) {
                        return p;
                    }
                }
                self.position
            }
            Role::Carrier => {
                if self.position.distance(square::fountain_center()) > 10.0 {
                    // Back to the basin to fill up, at a random point round its rim.
                    let a = self.random() * TAU;
                    square::fountain_center() + Vec2::new(a.cos(), a.sin()) * (square::FOUNTAIN_RADIUS + 0.6)
                } else {
                    // Off to a shop on one side or the other.
                    let right = self.random() < 0.5;
                    Vec2::new(if right { 28.4 } else { -18.4 }, -40.0 + self.random() * 75.0)
                }
            }
            _ => {
                let (lo, hi) = (square::WALK_MIN + Vec2::splat(1.5), square::WALK_MAX - Vec2::splat(1.5));
                loop {
                    let p = Vec2::new(lo.x + self.random() * (hi.x - lo.x), lo.y + self.random() * (hi.y - lo.y));
                    if square::obstacles().iter().all(|(c, r)| p.distance(*c) > r + 0.8) {
                        return p;
                    }
                }
            }
        }
    }
}

/// Everyone's position and what they're doing, taken before anyone moves this frame.
struct Glimpse {
    entity: Entity,
    position: Vec2,
    free: bool,
    role: Role,
}

/// Decides and moves: runs every frame.
pub fn think(time: Res<Time>, traffic: Option<Res<Traffic>>, mut people: Query<(Entity, &mut Person, &mut Transform, &mut Animator)>) {
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    let crowd: Vec<Glimpse> = people
        .iter_mut()
        .map(|(entity, p, ..)| Glimpse {
            entity,
            position: p.position,
            free: matches!(p.mind, Mind::Walk { .. } | Mind::Idle { .. }),
            role: p.role,
        })
        .collect();
    let mut obstacles = square::obstacles();
    if let Some(traffic) = &traffic {
        obstacles.extend_from_slice(&traffic.0);
    }
    // Conversations begun this frame: (who, with whom). Both sides must agree.
    let mut invitations: Vec<(Entity, Entity)> = Vec::new();
    // Children tagged this frame: (who, by whom); they turn and chase back.
    let mut tagged: Vec<(Entity, Entity)> = Vec::new();

    for (entity, mut person, mut transform, mut animator) in &mut people {
        let here = person.position;
        let where_is = |e: Entity| crowd.iter().find(|g| g.entity == e).map(|g| g.position);
        let mut target: Option<Vec2> = None;
        let mut face: Option<Vec2> = None;

        match person.mind {
            Mind::Walk { to } => {
                target = Some(to);
                if here.distance(to) < 0.8 {
                    decide(&mut person, entity, &crowd, now);
                }
            }
            Mind::Approach { partner } => match where_is(partner) {
                Some(p) if here.distance(p) > 1.4 => target = Some(p),
                Some(_) => {
                    let length = 12.0 + person.random() * 20.0;
                    person.mind = Mind::Talk { partner, until: now + length };
                    invitations.push((partner, entity));
                }
                None => person.mind = Mind::Idle { until: now + 2.0 },
            },
            Mind::Idle { until } => {
                if now > until {
                    decide(&mut person, entity, &crowd, now);
                }
            }
            Mind::Chase { target: quarry, until } => match where_is(quarry) {
                Some(p) if now < until => {
                    if here.distance(p) < 0.9 {
                        // Tag! Stop for breath; the one caught gives chase.
                        tagged.push((quarry, entity));
                        person.running = false;
                        person.mind = Mind::Idle { until: now + 1.5 + person.random() * 2.0 };
                    } else {
                        target = Some(p);
                    }
                }
                _ => {
                    person.running = false;
                    person.mind = Mind::Idle { until: now + 1.0 };
                }
            },
            Mind::Talk { partner, until } => {
                face = where_is(partner);
                if now > until {
                    person.mind = Mind::Walk { to: person.destination() };
                } else if now > person.next_turn {
                    // Take turns: talk a while, then listen a while.
                    person.speaking = !person.speaking;
                    person.next_turn = now + 2.5 + person.random() * 5.0;
                }
            }
        }

        // Walk toward the target, steering round obstacles and other people.
        let mut moving = false;
        if let Some(to) = target {
            let mut want = (to - here).normalize_or_zero();
            for (center, radius) in &obstacles {
                let away = here - *center;
                let gap: f32 = away.length() - radius;
                if gap < 2.0 {
                    // Push out, and slide round the obstacle's side rather than stopping.
                    let out = away.normalize_or_zero();
                    let side = Vec2::new(-out.y, out.x) * out.perp_dot(want).signum();
                    let urgency = ((2.0 - gap) / 2.0).clamp(0.0, 1.0);
                    want += (out * 1.2 + side * 0.8) * urgency;
                }
            }
            for other in &crowd {
                if other.entity == entity {
                    continue;
                }
                let away = here - other.position;
                let d = away.length();
                if d < 1.2 && d > 1e-3 {
                    want += away / d * (1.2 - d) * 1.5;
                }
            }
            if want.length() > 1e-3 {
                let desired = want.x.atan2(want.y);
                turn_toward(&mut person.heading, desired, 3.0 * dt);
                // Slow down while turning hard.
                let off = angle_between(person.heading, desired).abs();
                let pace = if person.running { person.run_speed } else { person.pace };
                let speed = pace * (1.0 - (off / PI).min(1.0) * 0.8);
                let step = person.facing() * speed * dt;
                let next = here + step;
                if next.x > square::WALK_MIN.x && next.x < square::WALK_MAX.x && next.y > square::WALK_MIN.y && next.y < square::WALK_MAX.y {
                    person.position = next;
                }
                moving = true;
            }
        } else if let Some(f) = face {
            let desired = (f - here).x.atan2((f - here).y);
            turn_toward(&mut person.heading, desired, 2.0 * dt);
        }

        // Show it: place the figure and pick the clip.
        let clip = if moving && person.running {
            "run"
        } else if moving {
            person.walk_clip
        } else if matches!(person.mind, Mind::Talk { .. }) && person.speaking {
            ["talk", "talk2", "talk3"][person.style % 3]
        } else if person.role == Role::Guard {
            "idle"
        } else {
            ["idle", "idle2"][person.style % 2]
        };
        animator.speed = match (moving, person.running) {
            (true, true) => person.run_speed / person.run_pace.max(0.1),
            (true, false) => person.pace / person.clip_pace.max(0.1),
            _ => 1.0,
        };
        let offset = person.random() * 3.0;
        animator.play(clip, 0.35, offset);
        transform.translation = Vec3::new(person.position.x, 0.0, person.position.y);
        transform.rotation = Quat::from_rotation_y(person.heading);
    }

    for (quarry, by) in tagged {
        if let Some((_, mut person, ..)) = people.get_mut(quarry) {
            if person.role == Role::Child {
                person.running = true;
                let until = now + 6.0 + person.random() * 4.0;
                person.mind = Mind::Chase { target: by, until };
            }
        }
    }

    // Whoever was asked to talk turns to face the one who asked.
    for (invited, by) in invitations {
        if let Some((_, mut person, ..)) = people.get_mut(invited) {
            if matches!(person.mind, Mind::Walk { .. } | Mind::Idle { .. }) {
                let length = 12.0 + person.random() * 20.0;
                person.mind = Mind::Talk { partner: by, until: now + length };
                person.speaking = true;
            }
        }
    }
}

/// What to do next, having arrived somewhere or stood about long enough.
fn decide(person: &mut Person, me: Entity, crowd: &[Glimpse], now: f32) {
    let roll = person.random();
    match person.role {
        Role::Guard => person.mind = Mind::Idle { until: now + 30.0 },
        Role::Carrier => {
            // A pause at each end: filling the buckets, or handing the water over.
            if matches!(person.mind, Mind::Walk { .. }) {
                person.mind = Mind::Idle { until: now + 4.0 + roll * 4.0 };
            } else {
                person.mind = Mind::Walk { to: person.destination() };
            }
        }
        Role::Child => {
            let other = crowd
                .iter()
                .filter(|g| g.entity != me && g.role == Role::Child && g.position.distance(person.position) < 15.0)
                .min_by(|a, b| a.position.distance(person.position).total_cmp(&b.position.distance(person.position)));
            if let (Some(other), true) = (other, roll < 0.4) {
                person.running = true;
                person.mind = Mind::Chase { target: other.entity, until: now + 5.0 + person.random() * 5.0 };
            } else if roll < 0.75 {
                person.running = person.random() < 0.6;
                person.mind = Mind::Walk { to: person.destination() };
            } else {
                person.running = false;
                person.mind = Mind::Idle { until: now + 1.0 + person.random() * 3.0 };
            }
        }
        Role::Vendor { .. } | Role::Stroller => {
            let reach = if matches!(person.role, Role::Vendor { .. }) { 4.0 } else { 8.0 };
            let company = crowd.iter().find(|g| {
                g.entity != me && g.free && g.role != Role::Guard && g.role != Role::Child && g.position.distance(person.position) < reach
            });
            if roll < 0.35 {
                if let Some(other) = company {
                    person.mind = Mind::Approach { partner: other.entity };
                    return;
                }
            }
            person.mind = if roll < 0.6 {
                Mind::Idle { until: now + 3.0 + person.random() * 8.0 }
            } else {
                Mind::Walk { to: person.destination() }
            };
        }
    }
}

fn angle_between(from: f32, to: f32) -> f32 {
    (to - from + PI).rem_euclid(TAU) - PI
}

fn turn_toward(heading: &mut f32, desired: f32, max: f32) {
    let d = angle_between(*heading, desired);
    *heading += d.clamp(-max, max);
}
