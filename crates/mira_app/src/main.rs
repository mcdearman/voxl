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
    // The starter scene's walker and gun are there for scenes saved from it too.
    game.add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugin)
        .add_plugins(mira_app::starter::StarterPlugin);
    if opening {
        game.insert_resource(Opening(scene.clone()))
            .add_systems(Stage::Startup, open_scene);
    } else if plugins.is_empty() {
        // Nothing asked for: a level to walk about in, and take apart.
        game.add_systems(Stage::Startup, mira_app::starter::scene);
    }
    for plugin in &plugins {
        let path = if plugin.contains(['/', '\\', '.']) {
            PathBuf::from(plugin)
        } else {
            cargo_library_path(plugin)
        };
        game.load_native_plugin(&path)?;
    }
    mira_app::editor::app(game)
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
