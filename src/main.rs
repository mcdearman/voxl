use voxl::{prelude::*, render::Screenshot, voxel::ChunkStreaming};

fn main() -> anyhow::Result<()> {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(VoxelPlugin::default())
        .add_systems(Stage::Startup, setup)
        .add_systems(
            Stage::Update,
            (
                grab_cursor,
                fly_camera,
                spin,
                select_block,
                edit_blocks,
                shoot,
                expire,
                show_stats,
                take_screenshot,
            ),
        )
        .add_systems(Stage::FixedUpdate, bounce)
        .run()
}

const REACH: f32 = 8.0;

struct Spin {
    axis: Vec3,
    speed: f32,
}
impl Component for Spin {}

struct FlyCamera {
    speed: f32,
    sensitivity: f32,
    yaw: f32,
    pitch: f32,
}
impl Component for FlyCamera {}

impl FlyCamera {
    fn looking(direction: Vec3) -> Self {
        let d = direction.normalize();
        Self {
            speed: 12.0,
            sensitivity: 0.002,
            yaw: (-d.x).atan2(-d.z),
            pitch: d.y.asin(),
        }
    }

    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }
}

struct Ball {
    velocity: Vec3,
    radius: f32,
    restitution: f32,
}
impl Component for Ball {}

struct Lifetime(f32);
impl Component for Lifetime {}

struct DemoAssets {
    sphere: Handle<Mesh>,
}

/// The block placed by right-click. Number keys change it.
struct SelectedBlock(BlockId);

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, registry: Res<BlockRegistry>) {
    let cube = meshes.add(Mesh::cube(1.0));
    let sphere = meshes.add(Mesh::uv_sphere(0.5, 32, 16));
    commands.insert_resource(DemoAssets { sphere });
    commands.insert_resource(SelectedBlock(registry.find("stone").unwrap()));

    // Ordinary mesh entities share the scene with the terrain: a ring of spinning cubes
    // orbiting a pivot, with each cube's transform relative to it.
    let pivot = commands
        .spawn((
            Transform::from_xyz(28.0, 84.0, -28.0),
            Spin {
                axis: Vec3::Y,
                speed: 0.3,
            },
        ))
        .id();
    for i in 0..12 {
        let angle = i as f32 / 12.0 * std::f32::consts::TAU;
        commands.spawn((
            Transform::from_xyz(angle.cos() * 10.0, 0.0, angle.sin() * 10.0),
            Mesh3d(cube),
            Material::color(Color::srgb(
                0.5 + angle.cos() * 0.4,
                0.5,
                0.5 + angle.sin() * 0.4,
            )),
            Spin {
                axis: Vec3::new(angle.cos(), 1.0, angle.sin()).normalize(),
                speed: 1.5,
            },
            Parent(pivot),
        ));
    }

    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.5, -1.0, -0.3), Vec3::Y),
        DirectionalLight {
            intensity: 3.0,
            ..Default::default()
        },
    ));
    commands.insert_resource(AmbientLight {
        intensity: 0.45,
        ..Default::default()
    });

    let camera = FlyCamera::looking(Vec3::new(1.0, -0.35, -1.0));
    commands.spawn((
        Transform::from_xyz(0.0, 95.0, 0.0).with_rotation(camera.rotation()),
        Camera::default(),
        ChunkViewer,
        camera,
    ));
}

fn grab_cursor(
    window: Res<Window>,
    keys: Res<ButtonInput<KeyCode>>,
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
    if focus.read().any(|f| !f.0) && window.cursor_grabbed() {
        window.set_cursor_grabbed(false);
    }
}

fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<Mouse>,
    window: Res<Window>,
    mut cameras: Query<(&mut Transform, &mut FlyCamera)>,
) {
    for (mut transform, mut camera) in &mut cameras {
        if window.cursor_grabbed() && mouse.delta != Vec2::ZERO {
            let sensitivity = camera.sensitivity;
            camera.yaw -= mouse.delta.x * sensitivity;
            camera.pitch = (camera.pitch - mouse.delta.y * sensitivity).clamp(-1.54, 1.54);
            transform.rotation = camera.rotation();
        }

        let (forward, right) = (transform.forward(), transform.right());
        let bindings = [
            (KeyCode::KeyW, forward),
            (KeyCode::KeyS, -forward),
            (KeyCode::KeyD, right),
            (KeyCode::KeyA, -right),
            (KeyCode::Space, Vec3::Y),
            (KeyCode::ShiftLeft, Vec3::NEG_Y),
        ];
        let direction: Vec3 = bindings
            .iter()
            .filter(|(key, _)| keys.pressed(*key))
            .map(|(_, dir)| *dir)
            .sum();
        if direction != Vec3::ZERO {
            let boost = if keys.pressed(KeyCode::ControlLeft) {
                5.0
            } else {
                1.0
            };
            transform.translation +=
                direction.normalize() * camera.speed * boost * time.delta_secs();
        }
    }
}

fn spin(time: Res<Time>, mut query: Query<(&mut Transform, &Spin)>) {
    for (mut transform, spin) in &mut query {
        transform.rotation *= Quat::from_axis_angle(spin.axis, spin.speed * time.delta_secs());
    }
}

fn select_block(
    keys: Res<ButtonInput<KeyCode>>,
    registry: Res<BlockRegistry>,
    mut selected: ResMut<SelectedBlock>,
) {
    let choices = [
        (KeyCode::Digit1, "stone"),
        (KeyCode::Digit2, "dirt"),
        (KeyCode::Digit3, "grass"),
        (KeyCode::Digit4, "sand"),
    ];
    for (key, name) in choices {
        if keys.just_pressed(key) {
            selected.0 = registry.find(name).unwrap();
        }
    }
}

/// Left click breaks the block under the crosshair, right click places one against it.
fn edit_blocks(
    window: Res<Window>,
    buttons: Res<ButtonInput<MouseButton>>,
    registry: Res<BlockRegistry>,
    selected: Res<SelectedBlock>,
    mut voxels: ResMut<VoxelWorld>,
    camera: Query<&Transform, With<FlyCamera>>,
) {
    if !window.cursor_grabbed() {
        if buttons.just_pressed(MouseButton::Left) {
            window.set_cursor_grabbed(true);
        }
        return;
    }
    let (breaking, placing) = (
        buttons.just_pressed(MouseButton::Left),
        buttons.just_pressed(MouseButton::Right),
    );
    if !breaking && !placing {
        return;
    }
    let Some(camera) = camera.get_single() else {
        return;
    };
    let Some(hit) = voxels.raycast(&registry, camera.translation, camera.forward(), REACH) else {
        return;
    };
    if breaking {
        voxels.set_block(hit.block, BlockId::AIR);
    } else if hit.normal != IVec3::ZERO {
        voxels.set_block(hit.adjacent(), selected.0);
    }
}

fn shoot(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    assets: Res<DemoAssets>,
    camera: Query<&Transform, With<FlyCamera>>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    let Some(camera) = camera.get_single() else {
        return;
    };
    commands.spawn((
        Transform::from_translation(camera.translation + camera.forward())
            .with_scale(Vec3::splat(0.6)),
        Mesh3d(assets.sphere),
        Material::color(Color::hex(0xffc53d)),
        Ball {
            velocity: camera.forward() * 30.0,
            radius: 0.3,
            restitution: 0.6,
        },
        // Balls move at the fixed rate; this blends between steps so they look smooth at
        // any frame rate.
        Interpolate::default(),
        Lifetime(15.0),
    ));
}

fn expire(mut commands: Commands, time: Res<Time>, mut query: Query<(Entity, &mut Lifetime)>) {
    for (entity, mut lifetime) in &mut query {
        lifetime.0 -= time.delta_secs();
        if lifetime.0 <= 0.0 {
            commands.despawn(entity);
        }
    }
}

/// Fixed-rate physics against the voxel grid. Each axis moves on its own: if the leading edge
/// of the ball would end up inside a solid block, that axis bounces instead.
fn bounce(
    fixed: Res<FixedTime>,
    voxels: Res<VoxelWorld>,
    registry: Res<BlockRegistry>,
    mut balls: Query<(&mut Transform, &mut Ball)>,
) {
    let dt = fixed.timestep_secs();
    for (mut transform, mut ball) in &mut balls {
        ball.velocity.y -= 20.0 * dt;
        for axis in 0..3 {
            let step = ball.velocity[axis] * dt;
            let mut probe = transform.translation;
            probe[axis] += step + ball.radius * step.signum();
            if voxels.is_solid(&registry, probe.floor().as_ivec3()) {
                let restitution = ball.restitution;
                ball.velocity[axis] *= -restitution;
                if axis == 1 {
                    // Rolling friction, and come to rest instead of bouncing forever.
                    ball.velocity.x *= 0.9;
                    ball.velocity.z *= 0.9;
                    if ball.velocity.y.abs() < 1.0 {
                        ball.velocity.y = 0.0;
                    }
                }
            } else {
                transform.translation[axis] += step;
            }
        }
    }
}

/// F2 saves a screenshot. Setting `VOXL_SCREENSHOT=<path>` instead saves one a few seconds
/// after launch and quits, which is handy for checking rendering from a script.
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
    if let Ok(path) = std::env::var("VOXL_SCREENSHOT") {
        match *automatic {
            None if time.elapsed_secs() > 6.0 => {
                screenshot.request(path);
                *automatic = Some(true);
            }
            Some(true) if screenshot.path.is_none() => exit.send(AppExit),
            _ => {}
        }
    }
}

#[derive(Default)]
struct FpsCounter {
    frames: u32,
    elapsed: f32,
}

#[allow(clippy::too_many_arguments)]
fn show_stats(
    time: Res<Time>,
    window: Res<Window>,
    voxels: Res<VoxelWorld>,
    streaming: Res<ChunkStreaming>,
    registry: Res<BlockRegistry>,
    selected: Res<SelectedBlock>,
    mut counter: Local<FpsCounter>,
    entities: Query<Entity>,
) {
    counter.frames += 1;
    counter.elapsed += time.delta_secs();
    if counter.elapsed >= 0.5 {
        let stats = format!(
            "voxl — {:.0} fps — {} chunks ({} in flight) — {} entities — placing {} (1-4) — click: break/place, F: ball",
            counter.frames as f32 / counter.elapsed,
            voxels.chunk_count(),
            streaming.in_flight(),
            entities.count(),
            registry.get(selected.0).map_or("?", |b| b.name.as_str()),
        );
        // Run with RUST_LOG=voxl=debug to get these in the terminal too.
        log::debug!("{stats}");
        window.set_title(&stats);
        *counter = FpsCounter::default();
    }
}
