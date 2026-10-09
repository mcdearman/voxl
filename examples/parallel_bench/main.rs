//! Measures what running systems at the same moment buys: eight systems that each do real
//! work on a component of their own, run as one batch and then one at a time.
//!
//! ```sh
//! cargo run --release --example parallel_bench
//! MIRA_THREADS=4 cargo run --release --example parallel_bench
//! ```

use std::time::{Duration, Instant};

use mira::ecs::{Component, Query, Schedule, World};

macro_rules! bodies {
    ($($component:ident $system:ident),*) => {
        $(
            #[derive(Component)]
            struct $component(f32, f32);

            fn $system(mut query: Query<&mut $component>) {
                for mut body in &mut query {
                    // Something with a cost: a few steps of a damped spring.
                    for _ in 0..8 {
                        body.1 += (-body.0 * 40.0 - body.1 * 0.5) * 0.001;
                        body.0 += body.1 * 0.001;
                    }
                }
            }
        )*

        fn world(count: usize) -> World {
            let mut world = World::new();
            for i in 0..count {
                $(world.spawn($component(i as f32 * 0.001, 0.0));)*
            }
            world
        }

        fn schedule(parallel: bool) -> Schedule {
            let mut schedule = Schedule::default();
            schedule.add_systems(($($system,)*));
            schedule.set_parallel(parallel);
            schedule
        }
    };
}

bodies!(A a, B b, C c, D d, E e, F f, G g, H h);

fn time(parallel: bool, world: &mut World, frames: u32) -> Duration {
    let mut schedule = schedule(parallel);
    schedule.run(world);
    let started = Instant::now();
    for _ in 0..frames {
        schedule.run(world);
    }
    started.elapsed() / frames
}

fn main() {
    let mut world = world(100_000);
    let frames = 30;
    let turns = time(false, &mut world, frames);
    let together = time(true, &mut world, frames);
    println!("8 systems, 100,000 entities each");
    println!("  one at a time: {turns:.2?} a frame");
    println!("  together:      {together:.2?} a frame");
    println!(
        "  {:.1} times faster",
        turns.as_secs_f64() / together.as_secs_f64()
    );
}
