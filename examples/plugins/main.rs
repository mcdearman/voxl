//! Native plugins and hot reload.
//!
//! The host (this file) draws a grid of cubes and knows nothing about how they move. Three
//! plugins animate them: `plugins/swirl` in Haskell, `plugins/wave` in Rust and
//! `plugins/pulse` in C. Build the ones you have toolchains for, run this, then edit a plugin
//! and rebuild it while the app is running:
//!
//! ```text
//! plugins/swirl/build.sh && cargo build -p wave && plugins/pulse/build.sh
//! cargo run --example plugins
//! # in another terminal, after changing a constant in plugins/swirl/Swirl.hs:
//! plugins/swirl/build.sh
//! ```

use mira::{plugin::cargo_library_path, prelude::*, render::Screenshot};

/// Where a cube sits on the grid. Exported to plugins as `demo.Cell`, so its layout is part
/// of the contract with them: `#[repr(C)]`, two floats.
#[repr(C)]
#[derive(Clone, Copy)]
struct Cell {
    x: f32,
    z: f32,
}
impl Component for Cell {}

fn main() -> anyhow::Result<()> {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins)
        .insert_resource(WindowSettings {
            title: "mira plugins: edit a plugin in plugins/ and rebuild it".into(),
            ..Default::default()
        })
        .add_systems(Stage::Startup, setup)
        .add_systems(Stage::Update, (quit, screenshot));

    // Plugins find components by name, so export before loading.
    app.world.export_component::<Cell>("demo.Cell");
    for name in ["wave", "pulse", "swirl"] {
        let path = cargo_library_path(name);
        if !path.exists() {
            log::warn!(
                "{} isn't built; see the top of examples/plugins/main.rs",
                path.display()
            );
        } else if let Err(err) = app.load_native_plugin(&path) {
            log::error!("{err:#}");
        }
    }
    app.run()
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let cube = meshes.add(Mesh::cube(1.0));
    for x in -8..=8 {
        for z in -8..=8 {
            let (x, z) = (x as f32, z as f32);
            commands.spawn((
                Transform::from_xyz(x * 1.3, 0.0, z * 1.3),
                Mesh3d(cube),
                Material::color(Color::srgb(
                    0.25 + (x + 8.0) / 24.0,
                    0.55,
                    0.9 - (z + 8.0) / 24.0,
                )),
                Cell { x, z },
            ));
        }
    }
    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.5, -1.0, -0.3), Vec3::Y),
        DirectionalLight {
            intensity: 2.0,
            ..Default::default()
        },
    ));
    commands.insert_resource(AmbientLight {
        intensity: 0.4,
        ..Default::default()
    });
    commands.spawn((
        Transform::from_xyz(14.0, 12.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        Camera::default(),
    ));
}

fn quit(keys: Res<ButtonInput<KeyCode>>, mut exit: EventWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.send(AppExit);
    }
}

/// `MIRA_SCREENSHOT=<path>` saves a frame a few seconds in and quits, for checking the
/// example from a script.
fn screenshot(
    time: Res<Time>,
    mut screenshot: ResMut<Screenshot>,
    mut exit: EventWriter<AppExit>,
    mut requested: Local<bool>,
) {
    let Ok(path) = std::env::var("MIRA_SCREENSHOT") else {
        return;
    };
    if !*requested && time.elapsed_secs() > 3.0 {
        screenshot.request(path);
        *requested = true;
    } else if *requested && screenshot.path.is_none() {
        exit.send(AppExit);
    }
}
