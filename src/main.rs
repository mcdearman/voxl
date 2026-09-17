use voxl::prelude::*;

fn main() -> anyhow::Result<()> {
    App::new()
        .add_plugins(DefaultPlugins)
        .insert_resource(WindowSettings {
            title: "voxl".into(),
            ..Default::default()
        })
        .add_systems(Stage::Startup, setup)
        .add_systems(
            Stage::Update,
            (
                grab_cursor,
                fly_camera,
                spin,
                shoot,
                expire,
                announce_spinners,
                show_fps,
            ),
        )
        .add_systems(Stage::FixedUpdate, bounce)
        .run()
}

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
            speed: 8.0,
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

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let cube = meshes.add(Mesh::cube(1.0));
    let sphere = meshes.add(Mesh::uv_sphere(0.5, 32, 16));
    let plane = meshes.add(Mesh::plane(1.0));
    commands.insert_resource(DemoAssets { sphere });

    commands.spawn((
        Transform::IDENTITY.with_scale(Vec3::new(60.0, 1.0, 60.0)),
        Mesh3d(plane),
        Material::color(Color::hex(0x5b7f4a)),
    ));

    for x in -5i32..=5 {
        for z in -5..=5 {
            let (fx, fz) = (x as f32, z as f32);
            let height = 0.6 + ((fx * 0.7).sin() + (fz * 0.5).cos()).abs();
            commands.spawn((
                Transform::from_xyz(fx * 2.0, height, fz * 2.0).with_scale(Vec3::splat(0.8)),
                Mesh3d(cube),
                Material::color(Color::srgb((fx + 5.0) / 10.0, 0.45, (fz + 5.0) / 10.0)),
                Spin {
                    axis: Vec3::new(fx, 4.0, fz).normalize(),
                    speed: 0.5 + (x + z).rem_euclid(3) as f32 * 0.4,
                },
            ));
        }
    }

    // A spinning pivot with a child: the child orbits because its transform is relative.
    let pivot = commands
        .spawn((
            Transform::from_xyz(0.0, 5.0, 0.0),
            Spin {
                axis: Vec3::Y,
                speed: 1.2,
            },
        ))
        .id();
    commands.spawn((
        Transform::from_xyz(4.0, 0.0, 0.0),
        Mesh3d(sphere),
        Material::color(Color::hex(0xe8e8f0)),
        Parent(pivot),
    ));

    commands.spawn((
        Transform::from_xyz(-6.0, 6.0, -6.0),
        Mesh3d(sphere),
        Material::color(Color::hex(0xd9534f)),
        Ball {
            velocity: Vec3::ZERO,
            radius: 0.5,
            restitution: 1.0,
        },
    ));

    commands.spawn((
        Transform::IDENTITY.looking_at(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
        DirectionalLight {
            intensity: 2.5,
            ..Default::default()
        },
    ));

    let position = Vec3::new(14.0, 8.0, 18.0);
    let camera = FlyCamera::looking(-position);
    commands.spawn((
        Transform::from_translation(position).with_rotation(camera.rotation()),
        Camera::default(),
        camera,
    ));
}

fn grab_cursor(
    window: Res<Window>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: EventReader<WindowFocused>,
    mut exit: EventWriter<AppExit>,
) {
    if buttons.just_pressed(MouseButton::Left) && !window.cursor_grabbed() {
        window.set_cursor_grabbed(true);
    }
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
                4.0
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

fn shoot(
    mut commands: Commands,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Res<Window>,
    assets: Res<DemoAssets>,
    camera: Query<&Transform, With<FlyCamera>>,
) {
    let fire = keys.just_pressed(KeyCode::KeyF)
        || (window.cursor_grabbed() && buttons.just_pressed(MouseButton::Right));
    let Some(camera) = camera.get_single().filter(|_| fire) else {
        return;
    };
    commands.spawn((
        Transform::from_translation(camera.translation + camera.forward())
            .with_scale(Vec3::splat(0.4)),
        Mesh3d(assets.sphere),
        Material::color(Color::hex(0xffc53d)),
        Ball {
            velocity: camera.forward() * 25.0,
            radius: 0.2,
            restitution: 0.7,
        },
        Lifetime(6.0),
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

/// Simple physics in the fixed-rate stage.
fn bounce(fixed: Res<FixedTime>, mut balls: Query<(&mut Transform, &mut Ball)>) {
    let dt = fixed.timestep_secs();
    for (mut transform, mut ball) in &mut balls {
        ball.velocity.y -= 9.81 * dt;
        transform.translation += ball.velocity * dt;
        if transform.translation.y < ball.radius && ball.velocity.y < 0.0 {
            transform.translation.y = ball.radius;
            let restitution = ball.restitution;
            ball.velocity.y *= -restitution;
            ball.velocity.x *= 0.98;
            ball.velocity.z *= 0.98;
        }
    }
}

/// `Added<T>` only matches components added since this system last ran.
fn announce_spinners(query: Query<Entity, Added<Spin>>) {
    let count = query.count();
    if count > 0 {
        log::info!("{count} new spinning entities");
    }
}

#[derive(Default)]
struct FpsCounter {
    frames: u32,
    elapsed: f32,
}

fn show_fps(
    time: Res<Time>,
    window: Res<Window>,
    mut counter: Local<FpsCounter>,
    entities: Query<Entity>,
) {
    counter.frames += 1;
    counter.elapsed += time.delta_secs();
    if counter.elapsed >= 0.5 {
        window.set_title(&format!(
            "voxl — {:.0} fps — {} entities — click to look, F to shoot",
            counter.frames as f32 / counter.elapsed,
            entities.count(),
        ));
        *counter = FpsCounter::default();
    }
}
