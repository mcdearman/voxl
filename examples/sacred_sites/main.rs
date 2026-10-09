//! The sacred sites of Age of Empires 4, as signals, with no window: a game to watch from
//! outside while it runs.
//!
//! ```sh
//! cargo run --example sacred_sites              # the game; listens on 127.0.0.1:7878
//! cargo run --bin voxl-debug -- watch           # in another terminal: the signal graph, live
//! cargo run --bin voxl-debug -- signal_set name=win_after value=20
//! cargo run --bin voxl-debug -- signal_force name=blue.contesting value=false
//! ```
//!
//! Red holds both sites, and wins when its clock reaches `win_after`. The clock stops while a
//! blue unit stands on any site red holds. Two blue scouts wander on and off the sites; no
//! code ever sets the clock or a paused flag. See `docs/SIGNALS.md`.

use std::time::Duration;

use voxl::{prelude::*, time::TimePlugin, transform::TransformPlugin};

#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug)]
#[reflect(name = "sites.Team")]
enum Team {
    Red,
    Blue,
}

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(name = "sites.Site")]
struct Site {
    holder: Option<Team>,
}

/// Walks between the sites' neighbourhood and the middle of the map.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(name = "sites.Scout")]
struct Scout {
    /// Seconds for one trip out and back.
    period: f32,
    /// Where it turns round.
    reach: f32,
}

const SITE_RADIUS: f32 = 2.0;

fn blue_contesting(units: Query<(&Team, &Transform)>, sites: Query<(&Site, &Transform)>) -> bool {
    units.iter().any(|(team, at)| {
        *team == Team::Blue
            && sites.iter().any(|(site, centre)| {
                site.holder == Some(Team::Red)
                    && at.translation.distance(centre.translation) < SITE_RADIUS
            })
    })
}

fn red_holds_all(sites: Query<&Site>) -> bool {
    sites.iter().all(|site| site.holder == Some(Team::Red))
}

fn wander(time: Res<Time>, mut scouts: Query<(&Scout, &mut Transform)>) {
    for (scout, mut at) in &mut scouts {
        let phase = time.elapsed_secs() / scout.period * std::f32::consts::TAU;
        at.translation.x = scout.reach * phase.sin();
    }
}

fn announce(signals: Res<Signals>) {
    println!(
        "red wins, {:.0} seconds on the clock",
        signals.number("red.clock")
    );
}

fn main() -> anyhow::Result<()> {
    let mut app = App::new();
    app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
    app.register_type::<Team>()
        .register_type::<Site>()
        .register_type::<Scout>();

    for x in [-10.0, 10.0] {
        app.world.spawn((
            Site {
                holder: Some(Team::Red),
            },
            Transform::from_xyz(x, 0.0, 0.0),
        ));
    }
    // One scout visits the east site, the other both.
    app.world.spawn((
        Team::Blue,
        Scout {
            period: 16.0,
            reach: 10.5,
        },
        Transform::IDENTITY,
    ));
    app.world.spawn((
        Team::Blue,
        Scout {
            period: 23.0,
            reach: -9.0,
        },
        Transform::IDENTITY,
    ));
    app.world
        .spawn((Team::Red, Transform::from_xyz(-10.0, 0.0, 0.0)));

    app.add_signal("red.holds_all", red_holds_all)
        .add_signal("blue.contesting", blue_contesting);
    let signals = app.world.resource_mut::<Signals>();
    signals.define("blue.calm", Op::Not, ["blue.contesting"]);
    signals.define("red.clock.running", Op::And, ["red.holds_all", "blue.calm"]);
    signals.define("red.lost_a_site", Op::Not, ["red.holds_all"]);
    signals.define(
        "red.clock",
        Op::Timer,
        ["red.clock.running", "red.lost_a_site"],
    );
    signals.set("win_after", 60.0);
    signals.define(
        "red.wins",
        Op::Compare(Compare::GreaterOrEqual),
        ["red.clock", "win_after"],
    );

    app.add_systems(
        Stage::Update,
        (wander, announce.run_if(signal_became_true("red.wins"))),
    );

    let address = std::env::var("VOXL_DEBUG").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
    let address = app.listen_for_debugger(&address)?;
    println!("the sacred sites are running; watch them with:");
    println!("  cargo run --bin voxl-debug -- --at {address} watch");
    loop {
        app.update();
        std::thread::sleep(Duration::from_millis(16));
    }
}
