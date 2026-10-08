//! A host for games written as plugins: it opens a window, sets up a camera and a sun, and
//! loads the plugins named on the command line. Everything else is up to them.
//!
//! ```text
//! plugins/chase/build.sh
//! cargo run --example host -- chase
//! ```
//!
//! An argument is a plugin built into Cargo's output directory (`chase`), or a path to a
//! shared library. Rebuild a plugin while this is running and it is reloaded.

use std::path::PathBuf;

use voxl::{plugin::cargo_library_path, prelude::*, render::Screenshot};

fn main() -> anyhow::Result<()> {
    let plugins: Vec<String> = std::env::args().skip(1).collect();
    if plugins.is_empty() {
        anyhow::bail!("name at least one plugin, e.g. `cargo run --example host -- chase`");
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins)
        .insert_resource(WindowSettings {
            title: format!("voxl: {}", plugins.join(", ")),
            ..Default::default()
        })
        .add_systems(Stage::Startup, setup)
        .add_systems(Stage::Update, (quit, screenshot));

    for plugin in &plugins {
        let path = if plugin.contains(['/', '\\', '.']) {
            PathBuf::from(plugin)
        } else {
            cargo_library_path(plugin)
        };
        app.load_native_plugin(&path)?;
    }
    app.run()
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.4, -1.0, -0.5), Vec3::Y),
        DirectionalLight {
            intensity: 2.5,
            ..Default::default()
        },
    ));
    commands.insert_resource(AmbientLight {
        intensity: 0.5,
        ..Default::default()
    });
    commands.spawn((
        Transform::from_xyz(0.0, 17.0, 15.0).looking_at(Vec3::new(0.0, 0.0, 1.0), Vec3::Y),
        Camera::default(),
    ));
}

fn quit(keys: Res<ButtonInput<KeyCode>>, mut exit: EventWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.send(AppExit);
    }
}

/// `VOXL_SCREENSHOT=<path>` saves a frame a few seconds in and quits, for checking a plugin
/// from a script.
fn screenshot(
    time: Res<Time>,
    mut screenshot: ResMut<Screenshot>,
    mut exit: EventWriter<AppExit>,
    mut requested: Local<bool>,
) {
    let Ok(path) = std::env::var("VOXL_SCREENSHOT") else {
        return;
    };
    if !*requested && time.elapsed_secs() > 3.0 {
        screenshot.request(path);
        *requested = true;
    } else if *requested && screenshot.path.is_none() {
        exit.send(AppExit);
    }
}
