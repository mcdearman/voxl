//! Measures what reading several components of each entity costs, against the best a table
//! could do: the same data in one plain array.
//!
//! ```sh
//! cargo run --release --example query_bench
//! ```
//!
//! Three worlds hold the same entities. In the first every entity was spawned with all its
//! components at once. In the second the components were added in a different order for
//! each kind, so an entity sits at different places in each component's storage. In the
//! third only one entity in eight has the rarest component, and the query asks for it.

use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use mira::ecs::{Component, World};
use mira::prelude::Vec3;

#[derive(Component, Clone, Copy)]
struct Position(Vec3);

#[derive(Component, Clone, Copy)]
struct Velocity(Vec3);

#[derive(Component, Clone, Copy)]
struct Drag(f32);

#[derive(Component, Clone, Copy)]
struct Marked;

const COUNT: usize = 200_000;
const ROUNDS: u32 = 40;

fn parts(i: usize) -> (Position, Velocity, Drag) {
    let f = i as f32;
    (
        Position(Vec3::new(f, 0.0, -f)),
        Velocity(Vec3::new(1.0, f * 0.001, 0.5)),
        Drag(0.01 + (i % 7) as f32 * 0.001),
    )
}

fn step(position: &mut Vec3, velocity: Vec3, drag: f32) {
    *position += velocity * (1.0 - drag) * 0.016;
}

fn together() -> World {
    let mut world = World::new();
    for i in 0..COUNT {
        world.spawn(parts(i));
    }
    world
}

/// The same entities, with each kind of component added in an order of its own.
fn scattered() -> World {
    let mut world = World::new();
    let entities: Vec<_> = (0..COUNT).map(|_| world.spawn(())).collect();
    // Strides that share no factor with the count visit every entity once.
    for (stride, kind) in [(1usize, 0), (7919, 1), (104_729, 2)] {
        for n in 0..COUNT {
            let i = n * stride % COUNT;
            let (position, velocity, drag) = parts(i);
            match kind {
                0 => world.insert(entities[i], position),
                1 => world.insert(entities[i], velocity),
                _ => world.insert(entities[i], drag),
            };
        }
    }
    world
}

fn sparse() -> World {
    let mut world = together();
    let entities: Vec<_> = world.query::<mira::ecs::Entity>().iter().collect();
    for entity in entities.into_iter().step_by(8) {
        world.insert(entity, Marked);
    }
    world
}

fn time(mut run: impl FnMut()) -> Duration {
    run();
    let started = Instant::now();
    for _ in 0..ROUNDS {
        run();
    }
    started.elapsed() / ROUNDS
}

fn report(what: &str, took: Duration, entities: usize, against: Duration) {
    println!(
        "{what:<44} {:>7.2} ms  {:>5.1} ns an entity  {:>5.1}x the array",
        took.as_secs_f64() * 1e3,
        took.as_secs_f64() * 1e9 / entities as f64,
        took.as_secs_f64() / against.as_secs_f64(),
    );
}

fn main() {
    // What a table gives at its best: every entity's components side by side.
    let mut array: Vec<(Position, Velocity, Drag)> = (0..COUNT).map(parts).collect();
    let array_time = time(|| {
        for (position, velocity, drag) in array.iter_mut() {
            step(&mut position.0, velocity.0, drag.0);
        }
        black_box(&array);
    });
    report("a plain array", array_time, COUNT, array_time);

    let moving = |world: &mut World| {
        let mut query = world.query::<(&mut Position, &Velocity, &Drag)>();
        for (mut position, velocity, drag) in &mut query {
            step(&mut position.0, velocity.0, drag.0);
        }
    };
    let mut world = together();
    let took = time(|| moving(&mut world));
    report(
        "three components, spawned together",
        took,
        COUNT,
        array_time,
    );

    let mut world = scattered();
    let took = time(|| moving(&mut world));
    report(
        "three components, added in other orders",
        took,
        COUNT,
        array_time,
    );

    let mut world = sparse();
    let took = time(|| {
        let mut query =
            world.query_filtered::<(&mut Position, &Velocity, &Drag), mira::ecs::With<Marked>>();
        for (mut position, velocity, drag) in &mut query {
            step(&mut position.0, velocity.0, drag.0);
        }
    });
    report(
        "the same, for the one in eight marked",
        took,
        COUNT / 8,
        array_time / 8,
    );

    let mut world = together();
    let took = time(|| {
        let mut query = world.query::<&mut Position>();
        for mut position in &mut query {
            step(&mut position.0, Vec3::X, 0.0);
        }
    });
    report("one component", took, COUNT, array_time);
}
