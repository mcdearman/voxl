//! The scene the app opens on when it is given nothing else: a small level to walk about in
//! first person, with a gun that fires balls at things that fall over.
//!
//! It is an ordinary game made of the engine's parts, there to be taken apart: a
//! `CharacterController` steered by a [`Walker`], a camera under it, a [`Gun`] under the
//! camera, blocks with colliders and bodies, a sun, and the settings for sky, haze and the
//! finished picture. Everything in it is registered, so it saves to a scene and comes back.

use mira::prelude::*;

/// Steers its entity's `CharacterController` from the keyboard and turns the camera below
/// it with the mouse: W A S D to walk, Shift to run, Space to jump, the arrow keys to turn
/// where the mouse is not held.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(name = "starter.Walker", default)]
pub struct Walker {
    /// Metres a second, walking and running.
    pub speed: f32,
    pub run_speed: f32,
    /// Radians turned for each count the mouse moves.
    pub look_speed: f32,
    /// Which way it faces, about the vertical, and how far it looks up or down.
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for Walker {
    fn default() -> Self {
        Self { speed: 5.0, run_speed: 8.5, look_speed: 0.0022, yaw: 0.0, pitch: 0.0 }
    }
}

/// Fires a ball from the end of its barrel when the left button is pressed, or F.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(name = "starter.Gun", default)]
pub struct Gun {
    /// How fast a ball leaves, in metres a second, and how heavy it is.
    pub speed: f32,
    pub mass: f32,
    /// Seconds between shots while the button is held.
    pub every: f32,
    /// Seconds until it can fire again, and how far it has kicked back.
    pub wait: f32,
    pub kick: f32,
}

impl Default for Gun {
    fn default() -> Self {
        Self { speed: 32.0, mass: 2.0, every: 0.16, wait: 0.0, kick: 0.0 }
    }
}

/// A ball that was fired, and how long it has left.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(name = "starter.Shot", default)]
pub struct Shot {
    pub left: f32,
}

/// Where the gun sits in front of the camera, and where its barrel ends.
const GUN_AT: Vec3 = Vec3::new(0.24, -0.2, -0.5);
const MUZZLE: Vec3 = Vec3::new(0.24, -0.16, -1.1);
const BALL: f32 = 0.11;

/// The walker, the gun and the shots: add it to play the starter scene or one saved from it.
pub struct StarterPlugin;

impl Plugin for StarterPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Walker>()
            .register_type::<Gun>()
            .register_type::<Shot>()
            .add_systems(Stage::Update, (walk, fire, expire));
    }
}

/// The direction a walker facing `yaw` goes for the keys held: ahead, back and to the sides.
pub fn heading(yaw: f32, ahead: f32, right: f32) -> Vec3 {
    let turn = Quat::from_rotation_y(yaw);
    (turn * Vec3::new(right, 0.0, -ahead)).normalize_or_zero()
}

fn walk(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<Mouse>,
    mut walkers: Query<(Entity, &mut Walker, &mut CharacterController)>,
    mut cameras: Query<(&Parent, &mut Transform), With<Camera>>,
) {
    let held = |key| keys.pressed(key) as i32 as f32;
    for (entity, mut walker, mut body) in &mut walkers {
        // The mouse where it is held in the picture; the arrow keys anywhere.
        let turn = held(KeyCode::ArrowLeft) - held(KeyCode::ArrowRight);
        let tilt = held(KeyCode::ArrowUp) - held(KeyCode::ArrowDown);
        let keyed = 1.8 * time.delta_secs();
        walker.yaw += turn * keyed - mouse.delta.x * walker.look_speed;
        walker.pitch = (walker.pitch + tilt * keyed - mouse.delta.y * walker.look_speed).clamp(-1.5, 1.5);

        let ahead = held(KeyCode::KeyW) - held(KeyCode::KeyS);
        let right = held(KeyCode::KeyD) - held(KeyCode::KeyA);
        let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let speed = if running { walker.run_speed } else { walker.speed };
        body.desired_velocity = heading(walker.yaw, ahead, right) * speed;
        if keys.just_pressed(KeyCode::Space) {
            body.jump = true;
        }
        // The body is a capsule and never turns; the camera under it does all the looking.
        let looking = Quat::from_rotation_y(walker.yaw) * Quat::from_rotation_x(walker.pitch);
        for (parent, mut at) in &mut cameras {
            if parent.0 == entity {
                at.rotation = looking;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fire(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
    mut server: ResMut<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut guns: Query<(&mut Gun, &Parent, &mut Transform)>,
    eyes: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    let pulled = buttons.pressed(MouseButton::Left) || keys.pressed(KeyCode::KeyF);
    for (mut gun, eye, mut held) in &mut guns {
        gun.wait = (gun.wait - dt).max(0.0);
        // The kick dies away, and the gun comes forward again.
        gun.kick *= (-14.0 * dt).exp();
        held.translation = GUN_AT + Vec3::new(0.0, gun.kick * 0.25, gun.kick);
        held.rotation = Quat::from_rotation_x(gun.kick * 1.6);
        if !pulled || gun.wait > 0.0 || dt == 0.0 {
            continue;
        }
        let Some(eye) = eyes.get(eye.0) else { continue };
        gun.wait = gun.every;
        gun.kick = 0.07;
        let along = eye.forward();
        let ball = server.sphere(&mut meshes, BALL);
        commands.spawn((
            Transform::from_translation(eye.0.transform_point3(MUZZLE)),
            Mesh3d(ball),
            Material {
                color: Color::rgb(1.0, 0.45, 0.1),
                emissive: Color::rgb(1.6, 0.5, 0.05),
                roughness: 0.35,
                ..Default::default()
            },
            Collider::sphere(BALL).with_restitution(0.5),
            RigidBody::dynamic().with_mass(gun.mass).with_velocity(along * gun.speed),
            Shot { left: 8.0 },
            Name::new("Shot"),
        ));
    }
}

fn expire(time: Res<Time>, mut commands: Commands, mut shots: Query<(Entity, &mut Shot)>) {
    for (entity, mut shot) in &mut shots {
        shot.left -= time.delta_secs();
        if shot.left <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Builds the level: the ground, walls and blocks to walk among, crates to knock over, the
/// player with camera and gun, the sun, and how the air and the picture are set.
pub fn scene(mut commands: Commands, mut server: ResMut<AssetServer>, mut meshes: ResMut<Assets<Mesh>>) {
    let cube = server.cube(&mut meshes, 1.0);
    let ball = server.sphere(&mut meshes, 0.5);
    let ground = server.plane(&mut meshes, 120.0);
    let paint = |r: f32, g: f32, b: f32, roughness: f32| Material {
        color: Color::rgb(r, g, b),
        roughness,
        ..Default::default()
    };
    let floor = paint(0.33, 0.36, 0.4, 0.92);
    let wall = paint(0.78, 0.78, 0.76, 0.85);
    let stone = paint(0.55, 0.57, 0.6, 0.8);
    let blue = paint(0.1, 0.32, 0.85, 0.45);
    let orange = paint(0.95, 0.42, 0.08, 0.5);

    commands.spawn((Transform::IDENTITY, Mesh3d(ground), floor, Collider::ground(), Name::new("Ground")));

    // Something that stands still: a box of this size, here.
    let mut block = |name: &str, at: Vec3, size: Vec3, material: Material| {
        commands.spawn((
            Transform::from_translation(at).with_scale(size),
            Mesh3d(cube),
            material,
            Collider::cuboid(size * 0.5),
            Name::new(name),
        ));
    };
    // A yard forty metres across, walled in.
    for (name, at, size) in [
        ("North wall", Vec3::new(0.0, 1.5, -20.0), Vec3::new(41.0, 3.0, 1.0)),
        ("South wall", Vec3::new(0.0, 1.5, 20.0), Vec3::new(41.0, 3.0, 1.0)),
        ("West wall", Vec3::new(-20.0, 1.5, 0.0), Vec3::new(1.0, 3.0, 39.0)),
        ("East wall", Vec3::new(20.0, 1.5, 0.0), Vec3::new(1.0, 3.0, 39.0)),
    ] {
        block(name, at, size, wall);
    }
    // A platform with steps up to it, pillars, and cover to walk round.
    block("Platform", Vec3::new(-9.0, 0.75, -9.0), Vec3::new(8.0, 1.5, 8.0), stone);
    for step in 0..5 {
        let rise = 0.25 * (step + 1) as f32;
        let at = Vec3::new(-9.0, rise * 0.5, -4.6 + 0.5 * (4 - step) as f32);
        block(&format!("Step {}", step + 1), at, Vec3::new(3.0, rise, 0.5), stone);
    }
    for (name, x, z) in [("Pillar", 8.0, -12.0), ("Second pillar", 13.0, -6.0), ("Third pillar", 6.0, 9.0)] {
        block(name, Vec3::new(x, 2.5, z), Vec3::new(1.2, 5.0, 1.2), wall);
    }
    block("Low wall", Vec3::new(2.0, 0.6, -3.0), Vec3::new(6.0, 1.2, 0.6), wall);
    block("Long block", Vec3::new(11.0, 1.0, 4.0), Vec3::new(2.0, 2.0, 7.0), stone);
    block("Arch top", Vec3::new(-8.0, 3.6, 8.0), Vec3::new(6.0, 0.8, 1.4), wall);
    block("Arch left", Vec3::new(-10.4, 1.6, 8.0), Vec3::new(1.2, 3.2, 1.4), wall);
    block("Arch right", Vec3::new(-5.6, 1.6, 8.0), Vec3::new(1.2, 3.2, 1.4), wall);

    // Things to shoot at: a wall of crates, a pyramid on the platform, and a few balls.
    let mut loose = |name: String, at: Vec3, size: f32, round: bool, material: Material| {
        let shape = if round { Collider::sphere(size * 0.5) } else { Collider::cuboid(Vec3::splat(size * 0.5)) };
        commands.spawn((
            Transform::from_translation(at).with_scale(Vec3::splat(size)),
            Mesh3d(if round { ball } else { cube }),
            material,
            shape,
            RigidBody::dynamic(),
            Name::new(name),
        ));
    };
    for row in 0..4 {
        for column in 0..5 {
            let at = Vec3::new(-2.0 + column as f32 * 1.02, 0.5 + row as f32 * 1.0, -10.0);
            let material = if (row + column) % 4 == 3 { orange } else { blue };
            loose(format!("Crate {}", row * 5 + column + 1), at, 1.0, false, material);
        }
    }
    for layer in 0..3 {
        for place in 0..(3 - layer) {
            let x = -9.0 + (place as f32 - (2 - layer) as f32 * 0.5) * 0.82;
            let at = Vec3::new(x, 1.9 + layer as f32 * 0.8, -9.0);
            loose(format!("Box {}", layer * 3 + place + 1), at, 0.8, false, orange);
        }
    }
    for (index, (x, z)) in [(5.0, 2.0), (6.5, 0.5), (4.0, -0.5)].into_iter().enumerate() {
        loose(format!("Ball {}", index + 1), Vec3::new(x, 0.5, z), 1.0, true, blue);
    }

    // The player: a capsule that walks, the camera at its eyes, the gun in front of that.
    let walker = Walker::default();
    let player = commands
        .spawn((
            Transform::from_xyz(0.0, 0.9, 12.0),
            CharacterController::new(1.8, 0.35),
            Interpolate::default(),
            walker,
            Name::new("Player"),
        ))
        .id();
    let camera = commands
        .spawn((
            Transform::from_xyz(0.0, 0.7, 0.0),
            Camera { fov_y: 75f32.to_radians(), near: 0.05, ..Default::default() },
            Parent(player),
            Name::new("Camera"),
        ))
        .id();
    let metal = Material { metallic: 0.9, ..paint(0.09, 0.1, 0.12, 0.38) };
    let gun = commands
        .spawn((Transform::from_translation(GUN_AT), Gun::default(), Parent(camera), Name::new("Gun")))
        .id();
    for (name, at, size, material) in [
        ("Barrel", Vec3::new(0.0, 0.04, -0.28), Vec3::new(0.05, 0.05, 0.62), metal),
        ("Body", Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.075, 0.12, 0.34), metal),
        ("Grip", Vec3::new(0.0, -0.11, 0.1), Vec3::new(0.06, 0.16, 0.08), paint(0.2, 0.13, 0.08, 0.7)),
        ("Sight", Vec3::new(0.0, 0.085, -0.5), Vec3::new(0.012, 0.03, 0.02), orange),
    ] {
        commands.spawn((
            Transform::from_translation(at).with_scale(size),
            Mesh3d(cube),
            material,
            NotShadowCaster,
            Parent(gun),
            Name::new(name),
        ));
    }

    // A sun in the afternoon, a little warm, with the sky that goes with it.
    let toward_sun = Vec3::new(-0.55, 0.62, 0.45);
    commands.spawn((
        Transform::IDENTITY.looking_at(-toward_sun, Vec3::Y),
        DirectionalLight { color: Color::rgb(1.0, 0.95, 0.86), intensity: 3.6, shadows: true },
        Name::new("Sun"),
    ));
    // A thin haze that gathers low down and far off, and the sun's light caught in it.
    commands.insert_resource(Fog { density: 0.006, height_falloff: 0.06, base_height: 0.0, start: 6.0 });
    commands.insert_resource(VolumetricLight { enabled: true, density: 0.004, ..Default::default() });
    commands.insert_resource(PostProcess {
        exposure: 1.05,
        bloom: 0.07,
        contrast: 1.5,
        saturation: 1.25,
        vignette: 0.22,
        temperature: 0.12,
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::{time::TimePlugin, transform::TransformPlugin};

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .add_plugins(TransformPlugin)
            .add_plugins(PhysicsPlugin)
            .add_plugins(StarterPlugin)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Mouse>()
            .init_resource::<AssetServer>()
            .init_resource::<Assets<Mesh>>()
            .add_systems(Stage::Startup, scene);
        app
    }

    /// Runs the game for about this long, a sixtieth of a second at a time.
    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            std::thread::sleep(std::time::Duration::from_micros(16_700));
            app.update();
        }
    }

    fn player(app: &mut App) -> (Vec3, bool) {
        let found = app.world.query::<(&Transform, &CharacterController, &Walker)>();
        let (at, body, _) = found.iter().next().expect("there is a player");
        (at.translation, body.grounded)
    }

    #[test]
    fn a_walker_goes_the_way_it_faces() {
        assert!(heading(0.0, 1.0, 0.0).abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(heading(0.0, 0.0, 1.0).abs_diff_eq(Vec3::X, 1e-6));
        // Turned a quarter to the left, ahead is toward -x.
        assert!(heading(std::f32::consts::FRAC_PI_2, 1.0, 0.0).abs_diff_eq(Vec3::NEG_X, 1e-6));
        // Two keys at once are no faster than one.
        assert!((heading(0.3, 1.0, 1.0).length() - 1.0).abs() < 1e-6);
        assert_eq!(heading(0.0, 0.0, 0.0), Vec3::ZERO);
    }

    #[test]
    fn the_player_walks_jumps_and_lands() {
        let mut app = game();
        run(&mut app, 0.5);
        let (start, grounded) = player(&mut app);
        assert!(grounded && (start.y - 0.9).abs() < 0.05, "standing at {start}");

        app.world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
        run(&mut app, 1.0);
        let (walked, _) = player(&mut app);
        assert!(start.z - walked.z > 3.0, "walked from {start} to {walked}");
        app.world.resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::KeyW);

        app.world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Space);
        run(&mut app, 0.25);
        // Nothing here clears the keys at a frame's end as the window does, and a press
        // left standing would jump again on landing.
        let keys = app.world.resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Space);
        keys.clear();
        let (up, grounded) = player(&mut app);
        assert!(!grounded && up.y > walked.y + 0.3, "in the air at {up}");
        run(&mut app, 1.5);
        let (down, grounded) = player(&mut app);
        assert!(grounded && (down.y - 0.9).abs() < 0.05, "landed at {down}");
    }

    #[test]
    fn the_gun_fires_balls_that_fly_and_then_go() {
        let mut app = game();
        run(&mut app, 0.2);
        let shots = |app: &mut App| app.world.query::<(&Shot, &Transform)>().iter().count();
        assert_eq!(shots(&mut app), 0);
        app.world.resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
        run(&mut app, 0.5);
        app.world.resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
        // Held for half a second, at one every 0.16 s.
        let fired = shots(&mut app);
        assert!((3..=4).contains(&fired), "{fired} shots");
        // They leave ahead of the player, toward the crates.
        let furthest = app
            .world
            .query::<(&Shot, &Transform)>()
            .iter()
            .map(|(_, at)| at.translation.z)
            .fold(f32::MAX, f32::min);
        assert!(furthest < 5.0, "the first ball has got to z = {furthest}");
        // And are gone when their time is up.
        let all: Vec<Entity> = app.world.query::<(Entity, &Shot)>().iter().map(|(e, _)| e).collect();
        for entity in all {
            app.world.get_mut::<Shot>(entity).unwrap().left = 0.01;
        }
        run(&mut app, 0.1);
        assert_eq!(shots(&mut app), 0);
    }
}
