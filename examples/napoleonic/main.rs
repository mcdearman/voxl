//! France, summer 1813. A line battalion drills volleys on a ridge, a foot battery fires at the
//! butts, a column marches through the village, and the Emperor watches from a knoll.
//!
//! ```text
//! cargo run --release --example napoleonic
//! ```
//!
//! Click to capture the mouse. WASD walks, Shift runs, Tab switches to flying (Space and Ctrl
//! go up and down), F2 saves a screenshot, Esc releases the mouse and again quits.
//!
//! The sky, ground and building materials are Poly Haven scans and the trees EZ-Tree models;
//! see `res/napoleonic/CREDITS.md`.

mod army;
mod artillery;
mod buildings;
mod grass;
mod materials;
mod scene;
mod smoke;
mod terrain;
mod vegetation;
mod world;

use mira::{glam::Vec2, prelude::*, render::Screenshot};

fn main() -> anyhow::Result<()> {
    let sky = std::fs::read(materials::asset("sky/sky_4k.hdr"))?;
    // Turn the photographed sky so its sun stands in the south-west, behind the viewer's left
    // shoulder at the start: the light rakes across the scene and models every shape.
    let environment = Environment::from_hdr_with_sun(&sky, Vec3::new(-0.55, 0.0, 0.56))?;
    App::new()
        .insert_resource(WindowSettings {
            title: "mira — France, 1813".into(),
            ..Default::default()
        })
        .insert_resource(environment)
        .insert_resource(ShadowSettings {
            resolution: 4096,
            max_distance: 260.0,
            first_split: 9.0,
            ..Default::default()
        })
        .insert_resource(Fog {
            density: 0.00032,
            height_falloff: 0.004,
            base_height: 0.0,
            start: 0.0,
        })
        .insert_resource(PostProcess {
            exposure: 1.0,
            bloom: 0.035,
            vignette: 0.35,
            saturation: 1.12,
            contrast: 1.2,
            // Summer light: shadows a touch cool, highlights warm, a little grain.
            shadow_tint: Color::rgb(0.97, 0.99, 1.03),
            highlight_tint: Color::rgb(1.03, 1.0, 0.95),
            temperature: 0.08,
            grain: 0.12,
            ..Default::default()
        })
        // Summer haze, with shafts of sun through the trees and the powder smoke.
        .insert_resource(VolumetricLight {
            enabled: true,
            density: 0.003,
            anisotropy: 0.6,
            height_falloff: 0.015,
            max_distance: 200.0,
            ..Default::default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(world::plugin)
        .add_plugins(grass::plugin)
        .add_plugins(smoke::plugin)
        .add_systems(
            Stage::Startup,
            (
                setup,
                scene::setup,
                world::init,
                army::setup,
                artillery::setup,
                smoke::init,
                scene::light_fires,
            ),
        )
        .add_systems(
            Stage::Update,
            (
                grab_cursor,
                move_player,
                show_stats,
                take_screenshot,
                scene::spin,
                army::march,
                army::drill,
                army::show_poses,
                artillery::fire_guns,
                artillery::roll_wheels,
                artillery::work_guns,
                scene::campfires,
            ),
        )
        .run()
}

const EYE_HEIGHT: f32 = 1.68;
const WALK_SPEED: f32 = 2.4;
const RUN_SPEED: f32 = 6.5;
const FLY_SPEED: f32 = 25.0;
/// Keep the player where the detailed terrain is.
const BOUNDS: f32 = terrain::DETAIL_EXTENT - 40.0;

struct Player {
    yaw: f32,
    pitch: f32,
    flying: bool,
    /// Steps walked, for the bob of a head in motion.
    walked: f32,
}
impl Component for Player {}

impl Player {
    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }
}

fn setup(mut commands: Commands, environment: Res<Environment>) {
    commands.spawn(environment.sun_light());

    let (start, look, flying) = start_view();
    let player = Player {
        yaw: (-look.x).atan2(-look.z),
        pitch: look.y.asin(),
        flying,
        walked: 0.0,
    };
    commands.spawn((
        Transform::from_translation(start).with_rotation(player.rotation()),
        Camera {
            fov_y: 50f32.to_radians(),
            near: 0.1,
            active: true,
        },
        player,
    ));
}

/// Where the player starts, which way they look, and whether they're flying.
/// `MIRA_VIEW=x,height,z,yaw_degrees,pitch_degrees` starts in the air somewhere else (height
/// above the ground), which is handy
/// for scripted screenshots.
fn start_view() -> (Vec3, Vec3, bool) {
    if let Ok(view) = std::env::var("MIRA_VIEW") {
        let v: Vec<f32> = view.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let [x, y, z, yaw, pitch] = v[..] {
            let rotation =
                Quat::from_euler(EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
            return (Vec3::new(x, terrain::walk_height(x, z) + y, z), rotation * Vec3::NEG_Z, true);
        }
        log::warn!("MIRA_VIEW should be x,y,z,yaw,pitch");
    }
    // Among the guns of the battery, looking east toward the butts; the battalion is off to
    // the left and the Emperor's knoll behind.
    let at = Vec2::new(318.0, 108.0);
    let position = scene::ground(at) + Vec3::Y * EYE_HEIGHT;
    (position, Vec3::new(1.0, -0.05, -0.28).normalize(), false)
}

fn grab_cursor(
    window: Res<Window>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut focus: EventReader<WindowFocused>,
    mut exit: EventWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if window.cursor_grabbed() {
            window.set_cursor_grabbed(false);
        } else {
            exit.send(AppExit);
        }
    }
    if buttons.just_pressed(MouseButton::Left) && !window.cursor_grabbed() {
        window.set_cursor_grabbed(true);
    }
    if focus.read().any(|f| !f.0) && window.cursor_grabbed() {
        window.set_cursor_grabbed(false);
    }
}

fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<Mouse>,
    window: Res<Window>,
    mut players: Query<(&mut Transform, &mut Player)>,
) {
    let dt = time.delta_secs();
    // Scripted screenshots hold the view still, whatever the mouse does.
    if std::env::var("MIRA_SCREENSHOT").is_ok() {
        return;
    }
    for (mut transform, mut player) in &mut players {
        if window.cursor_grabbed() && mouse.delta != Vec2::ZERO {
            player.yaw -= mouse.delta.x * 0.002;
            player.pitch = (player.pitch - mouse.delta.y * 0.002).clamp(-1.5, 1.5);
        }
        if keys.just_pressed(KeyCode::Tab) {
            player.flying = !player.flying;
        }
        transform.rotation = player.rotation();

        // Walking moves along the ground whichever way you look; flying goes where you look.
        let (forward, right) = if player.flying {
            (transform.forward(), transform.right())
        } else {
            let yaw = Quat::from_rotation_y(player.yaw);
            (yaw * Vec3::NEG_Z, yaw * Vec3::X)
        };
        let mut direction: Vec3 = [
            (KeyCode::KeyW, forward),
            (KeyCode::KeyS, -forward),
            (KeyCode::KeyD, right),
            (KeyCode::KeyA, -right),
        ]
        .iter()
        .filter(|(key, _)| keys.pressed(*key))
        .map(|(_, d)| *d)
        .sum();
        if player.flying {
            if keys.pressed(KeyCode::Space) {
                direction += Vec3::Y;
            }
            if keys.pressed(KeyCode::ControlLeft) {
                direction -= Vec3::Y;
            }
        }
        let fast = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let speed = match (player.flying, fast) {
            (true, false) => FLY_SPEED,
            (true, true) => FLY_SPEED * 4.0,
            (false, false) => WALK_SPEED,
            (false, true) => RUN_SPEED,
        };
        let step = direction.normalize_or_zero() * speed * dt;
        let mut p = transform.translation + step;
        p.x = p.x.clamp(-BOUNDS, BOUNDS);
        p.z = p.z.clamp(-BOUNDS, BOUNDS);
        let floor = terrain::walk_height(p.x, p.z) + EYE_HEIGHT;
        if player.flying {
            p.y = p.y.clamp(floor, 1500.0);
        } else {
            // One dip of the head per footfall, deeper and longer-strided when running.
            let step_length = if fast { 1.3 } else { 0.75 };
            player.walked += step.length() / step_length;
            let moving = if step.length() > 0.0 { 1.0 } else { 0.0 };
            let rise = if fast { 0.09 } else { 0.04 };
            p.y = floor + ((player.walked * std::f32::consts::PI).cos().abs() * 2.0 - 1.0) * rise * 0.5 * moving;
        }
        transform.translation = p;
    }
}

#[derive(Default)]
struct FpsCounter {
    frames: u32,
    elapsed: f32,
}

fn show_stats(
    time: Res<Time>,
    window: Res<Window>,
    mut counter: Local<FpsCounter>,
    players: Query<&Player>,
) {
    counter.frames += 1;
    counter.elapsed += time.delta_secs();
    if counter.elapsed >= 0.5 {
        let mode = players
            .get_single()
            .map_or("", |p| if p.flying { "flying" } else { "walking" });
        log::debug!("{:.0} fps", counter.frames as f32 / counter.elapsed);
        window.set_title(&format!(
            "mira — France, summer 1813 — {:.0} fps — {mode} (Tab to switch)",
            counter.frames as f32 / counter.elapsed,
        ));
        *counter = FpsCounter::default();
    }
}

/// F2 saves a screenshot. `MIRA_SCREENSHOT=<path>` saves one after a few seconds and quits;
/// `MIRA_SCREENSHOT_DELAY` sets how many.
fn take_screenshot(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut screenshot: ResMut<Screenshot>,
    mut exit: EventWriter<AppExit>,
    mut automatic: Local<Option<bool>>,
) {
    if keys.just_pressed(KeyCode::F2) {
        screenshot.request(format!("screenshot-{}.png", time.frame_count()));
    }
    if let Ok(path) = std::env::var("MIRA_SCREENSHOT") {
        let delay = std::env::var("MIRA_SCREENSHOT_DELAY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6.0);
        match *automatic {
            None if time.elapsed_secs() > delay => {
                screenshot.request(path);
                *automatic = Some(true);
            }
            Some(true) if screenshot.path.is_none() => exit.send(AppExit),
            _ => {}
        }
    }
}
