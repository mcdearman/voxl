//! Carts driving round the square behind walking horses: each follows the route from waypoint
//! to waypoint, turning gradually as a horse does, slows and stops for anyone in its way, and
//! rolls its wheels by how far it has gone. The crowd sees the carts as moving obstacles.

use std::f32::consts::{PI, TAU};

use mira::{glam::Vec2, prelude::*};

use crate::{crowd::Person, square::ROUTE};

/// Where the carts and their horses are this frame, as circles, for the crowd to walk round.
#[derive(Default)]
pub struct Traffic(pub Vec<(Vec2, f32)>);

pub struct Cart {
    waypoint: usize,
    position: Vec2,
    heading: f32,
    speed: f32,
    /// How fast the horse's walk clip covers the ground, and how far ahead of the cart's middle
    /// the horse stands.
    pace: f32,
    reach: f32,
    horse: Entity,
    /// Metres rolled, for turning the wheels.
    pub travelled: f32,
}
impl Component for Cart {}

impl Cart {
    pub fn new(start: usize, horse: Entity, pace: f32, reach: f32) -> Self {
        let (x, z) = ROUTE[start];
        let (nx, nz) = ROUTE[(start + 1) % ROUTE.len()];
        Self {
            waypoint: (start + 1) % ROUTE.len(),
            position: Vec2::new(x, z),
            heading: (nx - x).atan2(nz - z),
            speed: 0.0,
            pace,
            reach,
            horse,
            travelled: 0.0,
        }
    }
}

/// A wheel of a cart, turned by how far the cart has rolled.
pub struct Wheel {
    pub cart: Entity,
    pub radius: f32,
}
impl Component for Wheel {}

pub fn drive(
    time: Res<Time>,
    mut traffic: ResMut<Traffic>,
    mut carts: Query<(Entity, &mut Cart, &mut Transform)>,
    people: Query<&Person>,
    mut animators: Query<&mut Animator>,
) {
    let dt = time.delta_secs().min(0.1);
    let others: Vec<(Entity, Vec2)> = carts.iter_mut().map(|(e, c, _)| (e, c.position)).collect();
    let walkers: Vec<Vec2> = people.iter().map(|p| p.position).collect();
    traffic.0.clear();
    for (entity, mut cart, mut transform) in &mut carts {
        let (tx, tz) = ROUTE[cart.waypoint];
        let target = Vec2::new(tx, tz);
        if cart.position.distance(target) < 3.0 {
            cart.waypoint = (cart.waypoint + 1) % ROUTE.len();
        }
        let forward = Vec2::new(cart.heading.sin(), cart.heading.cos());
        // Anyone just ahead of the horse, or another cart close in front, and it pulls up.
        let nose = cart.position + forward * cart.reach;
        let blocked = walkers.iter().any(|p| {
            let to = *p - nose;
            to.length() < 3.0 && to.normalize_or_zero().dot(forward) > 0.3
        }) || others.iter().any(|(e, p)| {
            let to = *p - cart.position;
            *e != entity && to.length() < 9.0 && to.normalize_or_zero().dot(forward) > 0.7
        });
        let wanted = if blocked { 0.0 } else { cart.pace };
        cart.speed += (wanted - cart.speed) * (1.0 - (-dt * 1.5).exp());
        // Steer toward the waypoint, a horse's turn at a time.
        let to = target - cart.position;
        let desired = to.x.atan2(to.y);
        let turn = ((desired - cart.heading + PI).rem_euclid(TAU) - PI).clamp(-0.35 * dt, 0.35 * dt);
        cart.heading += turn * (cart.speed / cart.pace.max(0.1));
        let step = Vec2::new(cart.heading.sin(), cart.heading.cos()) * cart.speed * dt;
        cart.position += step;
        cart.travelled += step.length();
        transform.translation = Vec3::new(cart.position.x, 0.0, cart.position.y);
        transform.rotation = Quat::from_rotation_y(cart.heading);

        let facing = Vec2::new(cart.heading.sin(), cart.heading.cos());
        traffic.0.push((cart.position, 1.8));
        traffic.0.push((cart.position + facing * cart.reach, 1.4));
        if let Some(mut animator) = animators.get_mut(cart.horse) {
            if cart.speed > 0.15 {
                animator.speed = cart.speed / cart.pace.max(0.1);
                let clip = crate::animals::walk_clip(&animator);
                animator.play(clip, 0.4, 0.0);
            } else {
                animator.speed = 1.0;
                animator.play("idle", 0.6, 0.0);
            }
        }
    }
}

pub fn spin_wheels(carts: Query<&Cart>, mut wheels: Query<(&Wheel, &mut Transform)>) {
    for (wheel, mut transform) in &mut wheels {
        if let Some(cart) = carts.get(wheel.cart) {
            transform.rotation = Quat::from_rotation_x(cart.travelled / wheel.radius);
        }
    }
}
