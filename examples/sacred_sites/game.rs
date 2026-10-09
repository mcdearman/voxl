//! The sacred-site game itself: its components, its rules as signals, and how it is drawn.
//! Shared by the example beside it and by the one that shows the signal graph in the game.

use mira::{prelude::*, time::TimePlugin, transform::TransformPlugin};

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

/// The bar that shows red's clock.
#[derive(Component)]
struct ClockBar;

const BAR_LENGTH: f32 = 24.0;

/// Gives the game's entities something to be seen by, and sets the camera above them.
fn dress(
    mut commands: Commands,
    mut server: ResMut<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    sites: Query<Entity, With<Site>>,
    units: Query<(Entity, &Team)>,
) {
    let cube = server.cube(&mut meshes, 1.0);
    let ball = server.sphere(&mut meshes, 0.6);
    let ground = server.plane(&mut meshes, 60.0);
    let colour = |r, g, b| Material {
        color: Color::rgb(r, g, b),
        roughness: 0.8,
        ..Default::default()
    };
    commands.spawn((
        Transform::from_xyz(0.0, -0.05, 0.0),
        Mesh3d(ground),
        colour(0.25, 0.4, 0.22),
    ));
    for site in &sites {
        // A site is a flat slab; `show` scales and colours it.
        commands
            .entity(site)
            .insert((Mesh3d(cube), colour(0.7, 0.1, 0.1)));
    }
    for (unit, team) in &units {
        let material = match team {
            Team::Red => colour(0.85, 0.15, 0.15),
            Team::Blue => colour(0.15, 0.3, 0.9),
        };
        commands.entity(unit).insert((Mesh3d(ball), material));
    }
    commands.spawn((
        Transform::from_xyz(-BAR_LENGTH * 0.5, 0.3, 9.0),
        Mesh3d(cube),
        colour(0.9, 0.85, 0.3),
        ClockBar,
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 30.0, 26.0).looking_at(Vec3::ZERO, Vec3::Y),
        Camera::orthographic(26.0),
    ));
    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
        DirectionalLight::default(),
    ));
}

/// Draws the state of the rules: nothing here is kept, it is all read from the signals.
fn show(
    signals: Res<Signals>,
    window: Option<Res<Window>>,
    mut sites: Query<(&Site, &mut Transform, &mut Material), Without<ClockBar>>,
    mut bar: Query<&mut Transform, With<ClockBar>>,
) {
    let contested = signals.is_true("blue.contesting");
    for (site, mut at, mut material) in &mut sites {
        at.scale = Vec3::new(SITE_RADIUS * 2.0, 0.1, SITE_RADIUS * 2.0);
        material.color = match (site.holder, contested) {
            (Some(Team::Red), true) => Color::rgb(0.9, 0.6, 0.1),
            (Some(Team::Red), false) => Color::rgb(0.7, 0.1, 0.1),
            (Some(Team::Blue), _) => Color::rgb(0.1, 0.2, 0.7),
            (None, _) => Color::rgb(0.5, 0.5, 0.5),
        };
    }
    let (clock, win_after) = (signals.number("red.clock"), signals.number("win_after"));
    let filled = if win_after > 0.0 {
        (clock / win_after).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    for mut at in &mut bar {
        at.scale = Vec3::new((BAR_LENGTH * filled).max(0.01), 0.4, 0.6);
        at.translation.x = -BAR_LENGTH * 0.5 + BAR_LENGTH * filled * 0.5;
    }
    if let Some(window) = window {
        let state = if signals.is_true("red.wins") {
            "red wins"
        } else if contested {
            "clock stopped: a scout is on a site"
        } else {
            "clock running"
        };
        window.set_title(&format!(
            "sacred sites: {clock:.0} of {win_after:.0} s, {state}"
        ));
    }
}

/// Builds the game: with a window and a view from above, or with neither.
pub fn build(headless: bool) -> anyhow::Result<App> {
    let mut app = App::new();
    if headless {
        app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
    } else {
        app.add_plugins(DefaultPlugins)
            .insert_resource(WindowSettings {
                title: "sacred sites".to_owned(),
                ..Default::default()
            })
            .add_systems(Stage::Startup, dress)
            .add_systems(Stage::Update, show);
    }
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
    // A lost site sets the clock back; so does asking for a new round.
    signals.set("restart", false);
    signals.define("red.clock.reset", Op::Or, ["red.lost_a_site", "restart"]);
    signals.define(
        "red.clock",
        Op::Timer,
        ["red.clock.running", "red.clock.reset"],
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

    // With a window the engine is already listening if `MIRA_DEBUG` was set; this game
    // listens either way.
    let address = match app.debugger_address() {
        Some(address) => address,
        None => {
            let address =
                std::env::var("MIRA_DEBUG").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
            app.listen_for_debugger(&address)?
        }
    };
    println!("the sacred sites are running; watch them with:");
    println!("  cargo run --bin mira-debug -- --at {address} watch");
    Ok(app)
}
