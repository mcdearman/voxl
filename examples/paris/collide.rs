//! Keeps bodies from passing through one another: people, children and animals are pushed
//! apart until they only touch, and out of anything solid (the fountain and its bollards, the
//! stalls, lamps and trees, and the carts as they go by).
//!
//! The crowd and the animals steer round each other, but steering is a wish, not a wall; this
//! runs first each frame and settles whatever overlaps remain.

use voxl::{glam::Vec2, prelude::*};

use crate::{
    animals::{Beast, Kind},
    crowd::{Person, Role},
    square,
    traffic::Traffic,
};

/// How much room a body takes up on the ground.
fn person_radius(person: &Person) -> f32 {
    if person.role == Role::Child { 0.22 } else { 0.3 }
}

fn beast_radius(beast: &Beast) -> f32 {
    match beast.kind {
        Kind::Bird => 0.14,
        Kind::Pig => 0.4,
        Kind::Dog => 0.28,
        Kind::Tethered => 1.0,
    }
}

/// Everything solid that doesn't move by itself, as circles, and the carts where they are now.
pub fn solids(traffic: Option<&Traffic>) -> Vec<(Vec2, f32)> {
    let mut solids = square::obstacles();
    solids.extend(square::bollards().into_iter().map(|p| (p, 0.3)));
    if let Some(traffic) = traffic {
        solids.extend_from_slice(&traffic.0);
    }
    solids
}

/// Pushes a point out of every solid it overlaps and keeps it inside the square.
pub fn push_out(p: &mut Vec2, radius: f32, solids: &[(Vec2, f32)]) {
    for (center, r) in solids {
        let away = *p - *center;
        let d = away.length();
        let min = r + radius;
        if d < min {
            *p = *center + away.normalize_or(Vec2::X) * min;
        }
    }
    p.x = p.x.clamp(square::WALK_MIN.x + radius, square::WALK_MAX.x - radius);
    p.y = p.y.clamp(square::WALK_MIN.y + radius, square::WALK_MAX.y - radius);
}

pub fn separate(traffic: Option<Res<Traffic>>, mut people: Query<&mut Person>, mut beasts: Query<&mut Beast>) {
    let solids = solids(traffic.as_deref());
    // (position, radius, how readily it gives way): the tethered horses don't budge.
    let mut bodies: Vec<(Vec2, f32, f32)> = people.iter_mut().map(|p| (p.position, person_radius(&p), 1.0)).collect();
    let people_count = bodies.len();
    bodies.extend(beasts.iter_mut().map(|b| (b.position, beast_radius(&b), if b.kind == Kind::Tethered { 0.0 } else { 1.0 })));
    for _ in 0..3 {
        for i in 0..bodies.len() {
            for j in i + 1..bodies.len() {
                let (a, b) = (bodies[i], bodies[j]);
                let away = a.0 - b.0;
                let d = away.length();
                let min = a.1 + b.1;
                if d < min {
                    let push = away.normalize_or(Vec2::X) * (min - d);
                    let total = (a.2 + b.2).max(1e-3);
                    bodies[i].0 += push * (a.2 / total);
                    bodies[j].0 -= push * (b.2 / total);
                }
            }
        }
        for body in bodies.iter_mut().filter(|b| b.2 > 0.0) {
            push_out(&mut body.0, body.1, &solids);
        }
    }
    for (person, body) in people.iter_mut().zip(&bodies[..people_count]) {
        let mut person = person;
        person.position = body.0;
    }
    for (beast, body) in beasts.iter_mut().zip(&bodies[people_count..]) {
        let mut beast = beast;
        beast.position = body.0;
    }
}
