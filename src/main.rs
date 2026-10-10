//! mira, the engine app: a window, built with Neo, in which a game is shown, looked into and
//! changed while it runs.
//!
//! ```text
//! cargo run                         # the app, on a small scene to start from
//! cargo run -- chase scoreboard     # the app, on a game made of plugins
//! cargo run -- level.json           # the app, on a scene saved from it (Save writes there)
//! ```
//!
//! An argument is a plugin built into Cargo's output directory (`chase`), or a path to a
//! shared library; rebuild one while the app is open and it is reloaded. The samples are
//! examples: `cargo run --example voxel_world`, `--example sacred_sites`, and so on.

use std::path::PathBuf;

use mira::{
    plugin::cargo_library_path,
    prelude::*,
    reflect::{Scene, TypeRegistry},
};

fn main() -> anyhow::Result<()> {
    // A scene file to open (and to save to) is told from a plugin by how it ends.
    let (scenes, plugins): (Vec<String>, Vec<String>) = std::env::args()
        .skip(1)
        .partition(|argument| argument.ends_with(".json"));
    let scene = PathBuf::from(scenes.first().map_or("scene.json", String::as_str));
    let opening = !scenes.is_empty() && scene.exists();

    let mut game = App::new();
    game.add_plugins(DefaultPlugins).add_plugins(PhysicsPlugin);
    if opening {
        game.insert_resource(Opening(scene.clone()))
            .add_systems(Stage::Startup, open_scene);
    } else if plugins.is_empty() {
        // Nothing asked for: something to look at and take apart.
        game.add_systems(Stage::Startup, starter_scene);
    }
    for plugin in &plugins {
        let path = if plugin.contains(['/', '\\', '.']) {
            PathBuf::from(plugin)
        } else {
            cargo_library_path(plugin)
        };
        game.load_native_plugin(&path)?;
    }
    mira::editor::app(game)
        .with_scene(scene)
        .run()
        .map_err(|err| anyhow::anyhow!("{err}"))
}

/// The scene file the app was asked to open.
struct Opening(PathBuf);

/// Puts what the scene file holds into the world.
fn open_scene(world: &mut World) {
    let Some(Opening(path)) = world.remove_resource::<Opening>() else {
        return;
    };
    match Scene::load(&path) {
        Ok(scene) => {
            world.resource_scope(|world, registry: &mut TypeRegistry| {
                scene.spawn(world, registry);
            });
        }
        Err(why) => log::error!("can't open {}: {why}", path.display()),
    }
}

/// A floor, a few shapes, a sun and a camera.
fn starter_scene(
    mut commands: Commands,
    mut server: ResMut<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let cube = server.cube(&mut meshes, 1.0);
    let ball = server.sphere(&mut meshes, 0.6);
    let floor = server.plane(&mut meshes, 40.0);
    let colour = |r, g, b| Material {
        color: Color::rgb(r, g, b),
        roughness: 0.7,
        ..Default::default()
    };
    commands.spawn((
        Transform::IDENTITY,
        Mesh3d(floor),
        colour(0.32, 0.36, 0.34),
        Name::new("Floor"),
    ));
    commands.spawn((
        Transform::from_xyz(-2.0, 0.5, 0.0),
        Mesh3d(cube),
        colour(0.85, 0.3, 0.25),
        Name::new("Red cube"),
    ));
    let tall = Transform::from_xyz(0.0, 1.0, -1.5).with_scale(Vec3::new(1.0, 2.0, 1.0));
    let tall = commands
        .spawn((tall, Mesh3d(cube), colour(0.9, 0.75, 0.3)))
        .id();
    // One thing on top of another, to show a parent and its child.
    commands.spawn((
        Transform::from_xyz(0.0, 0.8, 0.0).with_scale(Vec3::new(1.0, 0.5, 1.0)),
        Mesh3d(ball),
        colour(0.95, 0.95, 0.95),
        Parent(tall),
        Name::new("Cap"),
    ));
    commands.spawn((
        Transform::from_xyz(2.2, 0.6, 0.4),
        Mesh3d(ball),
        colour(0.25, 0.45, 0.9),
        Name::new("Blue ball"),
    ));
    commands.spawn((
        Transform::from_xyz(6.0, 4.5, 8.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
        Camera::default(),
        Name::new("Camera"),
    ));
    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.5, -1.0, -0.35), Vec3::Y),
        DirectionalLight::default(),
        Name::new("Sun"),
    ));
}
