//! Paris in 1810: the Place du Châtelet on a bright afternoon, after Étienne Bouhot's
//! painting. The new Fontaine du Palmier raises its gilded Victory over a square full of
//! townsfolk, water-carriers, market women, a Guard sentry or two, carts and a coach; beyond
//! the Pont au Change stand the clock tower of the Palais and the towers of the Conciergerie.
//!
//! ```text
//! cargo run --release --example paris
//! ```
//!
//! Best on a GPU with hardware ray tracing (Vulkan), which gives traced sun shadows, true
//! reflections and light bounced about the square. Click to capture the mouse; WASD walks,
//! Shift runs, Tab flies, F2 saves a screenshot, Esc releases the mouse and again quits.

mod buildings;
mod animals;
mod collide;
mod crowd;
mod decals;
mod materials;
mod smoke;
mod square;
mod traffic;
mod water;

use std::{collections::HashMap, f32::consts::PI};

use mira::{
    glam::Vec2,
    prelude::*,
    render::{GltfScene, Image, ProbeGrid, Screenshot},
};

fn main() -> anyhow::Result<()> {
    let sky = std::fs::read(materials::asset("sky/sky_4k.hdr"))?;
    // A low afternoon sun ahead and to the left, down toward the river: it rakes along the
    // ochre block on the right, so every sill, moulding and stone throws a shadow across the
    // face, leaves the old houses on the left in shade, and sends the shadows of the crowd
    // back across the square toward the eye. The sky photo's sun stands higher; its disc is
    // cut out of the image, so the light alone is lowered, and warmed as a low sun is.
    let mut environment = Environment::from_hdr_with_sun(&sky, Vec3::new(-0.3, 0.0, -0.95))?;
    let across = Vec3::new(environment.sun_direction.x, 0.0, environment.sun_direction.z).normalize();
    let elevation = 22f32.to_radians();
    environment.sun_direction = across * elevation.cos() + Vec3::Y * elevation.sin();
    environment.sun_illuminance *= Vec3::new(1.0, 0.88, 0.72);
    App::new()
        .insert_resource(WindowSettings {
            title: "mira — Paris, 1810".into(),
            ..Default::default()
        })
        .insert_resource(environment)
        .insert_resource(ShadowSettings {
            resolution: 4096,
            max_distance: 160.0,
            first_split: 8.0,
            ..Default::default()
        })
        .insert_resource(Fog {
            density: 0.0012,
            height_falloff: 0.015,
            base_height: 0.0,
            start: 0.0,
        })
        .insert_resource(PostProcess {
            exposure: 1.0,
            bloom: 0.05,
            vignette: 0.3,
            saturation: 1.08,
            contrast: 1.3,
            // A warm afternoon: shadows a touch cool, highlights golden, a little grain.
            shadow_tint: Color::rgb(0.96, 0.98, 1.04),
            highlight_tint: Color::rgb(1.04, 1.0, 0.93),
            temperature: 0.12,
            grain: 0.15,
            ..Default::default()
        })
        // Haze in the air, lit by the sun where the buildings don't shade it.
        .insert_resource(VolumetricLight {
            enabled: true,
            density: 0.004,
            anisotropy: 0.65,
            height_falloff: 0.03,
            ..Default::default()
        })
        .insert_resource(RayTracingSettings {
            enabled: true,
            max_distance: 500.0,
            probes: Some(ProbeGrid::new(Vec3::new(-32.0, -0.5, -110.0), Vec3::new(46.0, 27.0, 50.0), 1.5)),
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugin)
        .add_plugins(smoke::plugin)
        .add_systems(Stage::Startup, (setup, build, smoke::init))
        .add_systems(Stage::Update, (grab_cursor, move_player, show_stats, take_screenshot, collide::separate, (water::stir, kick_ball), crowd::think, animals::think, traffic::drive, traffic::spin_wheels, chimney_smoke, fountain_spray))
        .run()
}

const EYE_HEIGHT: f32 = 1.66;

struct Player {
    yaw: f32,
    pitch: f32,
    flying: bool,
    /// Where the eye would be without the bob of walking.
    steady: Vec3,
    /// How far through the stride, in steps (each step is half a turn of the bob).
    stride: f32,
    /// How hard the walk bobs, easing in and out as walking starts and stops; how much of it
    /// is running.
    bob: f32,
    running: f32,
    speed: f32,
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
        steady: start,
        stride: 0.0,
        bob: 0.0,
        running: 0.0,
        speed: 0.0,
    };
    commands.spawn((
        Transform::from_translation(start).with_rotation(player.rotation()),
        Camera {
            fov_y: 50f32.to_radians(),
            near: 0.05,
            active: true,
            ..Default::default()
        },
        player,
    ));
}

/// `MIRA_VIEW=x,height,z,yaw_degrees,pitch_degrees` starts somewhere else (flying), for
/// scripted screenshots.
fn start_view() -> (Vec3, Vec3, bool) {
    if let Ok(view) = std::env::var("MIRA_VIEW") {
        let v: Vec<f32> = view.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let [x, y, z, yaw, pitch] = v[..] {
            let rotation = Quat::from_euler(EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
            return (Vec3::new(x, y, z), rotation * Vec3::NEG_Z, true);
        }
    }
    // Where the painter stood: back from the fountain, looking across the square toward the
    // river with the column a little right of centre.
    let at = Vec3::new(-6.0, EYE_HEIGHT, 30.0);
    let target = square::FOUNTAIN + Vec3::new(-3.0, 7.5, 0.0);
    (at, (target - at).normalize(), false)
}

fn load(world: &mut World, name: &str) -> Option<GltfScene> {
    let scene = world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
        GltfScene::load(materials::asset(&format!("models/{name}.glb")), meshes, world.resource_mut::<Assets<Image>>())
    });
    match scene {
        Ok(s) => Some(s),
        Err(err) => {
            log::error!("couldn't load {name}: {err:#}");
            None
        }
    }
}

/// Spawns a model's parts under one root, its feet at `at`, facing `facing`.
fn place(world: &mut World, parts: &[(Handle<Mesh>, Material, Mat4)], at: Vec3, facing: Vec2, scale: f32) {
    // The models were exported facing +Z; turn them to face where they look.
    let yaw = (-facing.x).atan2(-facing.y) + PI;
    let root = world.spawn(Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)).with_scale(Vec3::splat(scale)));
    for (mesh, material, local) in parts {
        let (scale, rotation, translation) = local.to_scale_rotation_translation();
        world.spawn((Transform { translation, rotation, scale }, Mesh3d(*mesh), *material, Parent(root)));
    }
}

fn build(world: &mut World) {
    let started = std::time::Instant::now();
    let palette = world.resource_scope(|_, images: &mut Assets<Image>| materials::load_all(images));
    let palette = match palette {
        Ok(p) => p,
        Err(err) => {
            log::error!("couldn't load the textures: {err:#}");
            return;
        }
    };
    let square = square::build(&palette);
    world.insert_resource(Chimneys(square.chimneys.clone()));
    water::spawn(world, square::basin_water());
    for (at, collider) in square::colliders() {
        world.spawn((Transform::from_translation(at), collider));
    }
    // A leather ball the children kick about, and anyone may stumble into.
    let ball = world.resource_mut::<Assets<Mesh>>().add(Mesh::uv_sphere(BALL_RADIUS, 20, 14));
    let leather = Material { color: Color::rgb(0.32, 0.18, 0.09), roughness: 0.55, ..Default::default() };
    world.spawn((
        Transform::from_xyz(3.0, BALL_RADIUS, 4.0),
        Mesh3d(ball),
        leather,
        RigidBody::dynamic().with_density(90.0),
        Collider::sphere(BALL_RADIUS).with_restitution(0.55).with_friction(0.8),
        Ball { kicked: 0.0 },
        Interpolate::default(),
    ));
    for (material, mesh) in square.parts {
        if mesh.indices.is_empty() {
            continue;
        }
        let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
        world.spawn((Transform::IDENTITY, Mesh3d(handle), material));
    }
    log::info!("the square built in {:.1?}", started.elapsed());

    // The townsfolk, animated and minded. Each model is loaded once; no two people wear
    // quite the same shade.
    let mut models: HashMap<&str, (GltfScene, f32, f32, f32)> = HashMap::new();
    for (i, placement) in square.people.iter().enumerate() {
        let model = placement.model.trim_end_matches("_walk");
        let walking = placement.model.ends_with("_walk");
        if !models.contains_key(model) {
            let Some(mut scene) = load(world, model) else { continue };
            for part in &mut scene.parts {
                dress(&part.material_name, &mut part.material);
            }
            // The walks are recorded moving over the ground: take that out, so they walk on the
            // spot as the crowd moves them, and keep how fast they went, to match strides to
            // speed.
            let (mut walk, mut stroll, mut run) = (1.3, 1.1, 2.5);
            if let Some(skeleton) = scene.skeleton.clone() {
                for clip in &mut scene.clips {
                    let name = clip.name.clone();
                    if name.starts_with("walk") || name.starts_with("stroll") || name.starts_with("run") {
                        let speed = std::sync::Arc::make_mut(clip).remove_root_motion(&skeleton);
                        if speed > 0.1 {
                            if name.starts_with("walk") {
                                walk = speed
                            } else if name.starts_with("stroll") {
                                stroll = speed
                            } else {
                                run = speed
                            }
                        }
                    }
                }
            }
            log::info!("{model}: walk clip {walk:.2} m/s, stroll {stroll:.2} m/s, run {run:.2} m/s");
            models.insert(model, (scene, walk, stroll, run));
        }
        let (scene, walk_pace, stroll_pace, run_pace) = &models[model];
        let shade = |k: f32| 0.9 + 0.2 * buildings::hash(i as f32 * 7.3, k);
        let yaw = placement.facing.x.atan2(placement.facing.y);
        let transform = Transform::from_translation(placement.at).with_rotation(Quat::from_rotation_y(yaw));
        let root = scene.spawn_animated(world, transform, |part| {
            let mut m = part.material;
            if m.subsurface == 0.0 {
                m.color = Color::rgb(m.color.r * shade(1.0), m.color.g * shade(2.0), m.color.b * shade(3.0));
            }
            m
        });
        let role = match model {
            "grenadier" => crowd::Role::Guard,
            "market_woman" => crowd::Role::Vendor { home: Vec2::new(placement.at.x, placement.at.z) },
            "water_carrier" => crowd::Role::Carrier,
            m if m.starts_with("boy") || m.starts_with("girl") => crowd::Role::Child,
            _ => crowd::Role::Stroller,
        };
        let stroll = matches!(model, "lady2" | "lady3" | "elder" | "market_woman" | "maid");
        let pace = if stroll { *stroll_pace } else { *walk_pace };
        let at = Vec2::new(placement.at.x, placement.at.z);
        let person = crowd::Person::new(role, at, placement.facing, i as u32 + 1, walking, stroll, pace, *run_pace);
        world.insert(root, person);
        // A body for the physics: moved by the crowd's minds, it shoves whatever it walks into.
        let height = if role == crowd::Role::Child { 1.25 } else { 1.72 };
        world.insert(root, RigidBody::animated());
        world.insert(root, Collider::capsule(height, 0.24).at(Vec3::Y * height * 0.5));
    }

    // The fountain's statues: limestone, and the Victory in gold.
    for statue in &square.statues {
        let Some(scene) = load(world, statue.model) else { continue };
        let gold = statue.model == "victory";
        let parts: Vec<_> = scene
            .parts
            .into_iter()
            .map(|p| {
                let material = if gold {
                    Material { color: Color::rgb(1.0, 0.74, 0.34), roughness: 0.28, metallic: 1.0, normal_texture: p.material.normal_texture, ..Default::default() }
                } else {
                    Material { roughness: 0.75, weathering: 0.6, normal_texture: p.material.normal_texture, ..palette.plaster }
                };
                (p.mesh, material, p.transform)
            })
            .collect();
        place(world, &parts, statue.at, statue.facing, statue.scale);
    }

    // Trees along the quay, their leaves dappling the paving with shade.
    if let Some(tree) = load(world, "trees/ash") {
        let parts: Vec<_> = tree
            .parts
            .iter()
            .map(|p| {
                let mut m = p.material;
                if m.alpha_cutoff.is_some() {
                    m.color = Color::rgb(m.color.r * 0.72, m.color.g * 0.78, m.color.b * 0.55);
                    m.translucency = 0.45;
                    m.roughness = m.roughness.max(0.6);
                    m.double_sided = true;
                } else {
                    m.roughness = 1.0;
                    m.metallic = 0.0;
                }
                (p.mesh, m, p.transform)
            })
            .collect();
        for (at, yaw, scale) in &square.trees {
            place(world, &parts, *at, Vec2::new(yaw.sin(), yaw.cos()), *scale);
        }
    }

    // Animals: loaded once each, with how fast each walk covers the ground.
    let mut beasts: HashMap<&str, (GltfScene, f32)> = HashMap::new();
    let mut animal = |world: &mut World, model: &'static str| -> Option<(GltfScene, f32)> {
        if !beasts.contains_key(model) {
            let scene = load(world, &format!("animals/{model}"))?;
            // Each rig names its bones after its animal.
            let prefix = animals::prefix(model);
            let feet = if prefix == "chicken " || prefix == "rooster " || prefix == "goose " { ["L Foot", "R Foot"] } else { ["L Toe0", "R Toe0"] };
            let feet = feet.map(|f| format!("{prefix}{f}"));
            let pace = scene
                .skeleton
                .clone()
                .map(|s| Animator::new(s, scene.clips.clone()))
                .and_then(|a| a.ground_speed("walk", [&feet[0], &feet[1]]))
                .filter(|p| *p > 0.05 && *p < 5.0)
                .unwrap_or(0.8);
            // Measuring by the lowest foot is fooled by four legs out of step; a horse's walk,
            // a stride of about 1.4 m each 1.15 s cycle, covers about 1.2 m a second.
            let pace = if model.starts_with("horse") { 1.2 } else { pace };
            log::info!("{model}: walk clip {pace:.2} m/s");
            beasts.insert(model, (scene, pace));
        }
        beasts.get(model).map(|(s, p)| (s.clone(), *p))
    };
    let animal_material = |part: &mira::render::GltfPart| {
        let mut m = part.material;
        m.metallic = if part.material_name.contains("brass") { 1.0 } else { 0.0 };
        m.roughness = m.roughness.max(0.55);
        m
    };

    // The coach horses stand waiting.
    for (i, (coat, at, facing)) in square.horses.iter().enumerate() {
        let Some((scene, pace)) = animal(world, coat) else { continue };
        let yaw = facing.x.atan2(facing.y);
        let root = scene.spawn_animated(world, Transform::from_translation(*at).with_rotation(Quat::from_rotation_y(yaw)).with_scale(Vec3::splat(0.95)), animal_material);
        let mut beast = animals::Beast::new(animals::Kind::Tethered, Vec2::new(at.x, at.z), 0.0, 900 + i as u32, pace);
        beast.face(yaw);
        world.insert(root, beast);
        animals::give_legs(world, root, coat, pace);
        world.insert(root, RigidBody::animated());
        world.insert(root, animals::collider(animals::Kind::Tethered, coat));
    }
    // Hens, geese, pigs and dogs about the square.
    for (i, (model, home, range)) in square.animals.iter().enumerate() {
        let Some((scene, pace)) = animal(world, model) else { continue };
        let root = scene.spawn_animated(world, Transform::from_translation(Vec3::new(home.x, 0.0, home.y)), animal_material);
        world.insert(root, animals::Beast::new(animals::Kind::of(model), *home, *range, 500 + i as u32, pace));
        animals::give_legs(world, root, model, pace);
        world.insert(root, RigidBody::animated());
        world.insert(root, animals::collider(animals::Kind::of(model), model));
    }
    // Carts driving round, their horses walking and their wheels turning.
    world.insert_resource(traffic::Traffic::default());
    let mut wheel_meshes: HashMap<u32, Vec<(Handle<Mesh>, Material)>> = HashMap::new();
    for vehicle in square.vehicles {
        let root = world.spawn(Transform::IDENTITY);
        for (material, mesh) in vehicle.parts {
            let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
            world.spawn((Transform::IDENTITY, Mesh3d(handle), material, Parent(root)));
        }
        for (center, radius) in vehicle.wheels {
            let key = (radius * 1000.0) as u32;
            if let std::collections::hash_map::Entry::Vacant(slot) = wheel_meshes.entry(key) {
                let parts = square::wheel_parts(radius)
                    .into_iter()
                    .map(|(m, mesh)| (world.resource_mut::<Assets<Mesh>>().add(mesh), m))
                    .collect();
                slot.insert(parts);
            }
            let wheel = world.spawn((Transform::from_translation(center), traffic::Wheel { cart: root, radius }, Parent(root)));
            for (mesh, material) in &wheel_meshes[&key] {
                world.spawn((Transform::IDENTITY, Mesh3d(*mesh), *material, Parent(wheel)));
            }
        }
        let (offset, coat) = vehicle.horse;
        let Some((scene, pace)) = animal(world, coat) else { continue };
        let horse = scene.spawn_animated(world, Transform::from_translation(offset).with_scale(Vec3::splat(0.95)), animal_material);
        world.insert(horse, Parent(root));
        animals::give_legs(world, horse, coat, pace);
        world.insert(root, traffic::Cart::new(vehicle.start, horse, pace * 0.95, offset.z));
    }
    log::info!("{} people placed in {:.1?}", square.people.len(), started.elapsed());
}

const BALL_RADIUS: f32 = 0.11;

/// The children's ball.
struct Ball {
    /// When it was last kicked.
    kicked: f32,
}
impl Component for Ball {}

/// A child who runs up to the ball kicks it on, away from them and a little up.
fn kick_ball(time: Res<Time>, mut balls: Query<(&Transform, &mut RigidBody, &mut Ball)>, people: Query<&crowd::Person>) {
    let now = time.elapsed_secs();
    for (transform, mut body, mut ball) in &mut balls {
        if now - ball.kicked < 1.2 {
            continue;
        }
        let at = Vec2::new(transform.translation.x, transform.translation.z);
        let kicker = people.iter().filter(|p| p.role == crowd::Role::Child).find(|p| p.position.distance(at) < 0.45);
        if let Some(child) = kicker {
            let away = (at - child.position).normalize_or(Vec2::X);
            let strength = 1.0 + ((now * 7.3).sin() * 0.5 + 0.5) * 0.8;
            body.apply_impulse(Vec3::new(away.x, 0.35, away.y) * strength);
            ball.kicked = now;
        }
    }
}

/// Chimney pots with a fire lit below.
struct Chimneys(Vec<Vec3>);

/// Puffs of woodsmoke from the lit chimneys, a few a second.
fn chimney_smoke(time: Res<Time>, chimneys: Option<Res<Chimneys>>, mut smoke: ResMut<smoke::Smoke>, mut clock: Local<f32>) {
    let Some(chimneys) = chimneys else { return };
    *clock += time.delta_secs();
    while *clock > 0.35 {
        *clock -= 0.35;
        for pot in &chimneys.0 {
            smoke.woodsmoke(*pot);
        }
    }
}

/// Spray where the fountain's jets fall into the basin.
fn fountain_spray(time: Res<Time>, mut smoke: ResMut<smoke::Smoke>, mut clock: Local<f32>) {
    *clock += time.delta_secs();
    let splashes = square::jet_splashes();
    while *clock > 0.04 {
        *clock -= 0.04;
        for at in &splashes {
            smoke.spray(*at);
        }
    }
}

/// Tunes the figures' materials: skin scatters light, cloth and felt are matt, metal shines.
fn dress(material_name: &str, m: &mut Material) {
    // glTF counts a material as metal unless it says otherwise; only the brass and steel are.
    if ["brass", "steel", "iron"].iter().any(|metal| material_name.contains(metal)) {
        return;
    }
    m.metallic = 0.0;
    if material_name.ends_with("_head") {
        // Faces: light scatters under skin, and it has a soft sheen.
        m.subsurface = 0.75;
        m.roughness = 0.5;
        m.normal_strength = 1.0;
    } else {
        m.roughness = m.roughness.max(0.6);
    }
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

#[allow(clippy::too_many_arguments)]
fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<Mouse>,
    window: Res<Window>,
    mut players: Query<(&mut Transform, &mut Player)>,
    traffic: Option<Res<traffic::Traffic>>,
    people: Query<&crowd::Person>,
    beasts: Query<&animals::Beast>,
) {
    // Scripted screenshots hold the view still, whatever the mouse does.
    if std::env::var("MIRA_SCREENSHOT").is_ok() {
        return;
    }
    let dt = time.delta_secs();
    for (mut transform, mut player) in &mut players {
        if window.cursor_grabbed() && mouse.delta != Vec2::ZERO {
            player.yaw -= mouse.delta.x * 0.002;
            player.pitch = (player.pitch - mouse.delta.y * 0.002).clamp(-1.5, 1.5);
        }
        if keys.just_pressed(KeyCode::Tab) {
            player.flying = !player.flying;
        }
        transform.rotation = player.rotation();
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
        // Walk briskly; hold Shift to run.
        let run = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let target = match (player.flying, run) {
            (true, false) => 6.0,
            (true, true) => 20.0,
            (false, false) => 2.4,
            (false, true) => 6.5,
        };
        let moving = direction.length() > 0.0;
        // Speed up and slow down over a moment, as a body does.
        let wanted = if moving { target } else { 0.0 };
        player.speed += (wanted - player.speed) * (1.0 - (-dt * 6.0).exp());
        let step = direction.normalize_or_zero() * player.speed * dt;
        let mut p = player.steady + step;
        if player.flying {
            p.x = p.x.clamp(-150.0, 150.0);
            p.z = p.z.clamp(-220.0, 120.0);
        } else {
            p.x = p.x.clamp(square::WALK_MIN.x, square::WALK_MAX.x);
            p.z = p.z.clamp(square::WALK_MIN.y, square::WALK_MAX.y);
            // Solid things and bodies push back: the fountain, bollards, stalls, carts, people
            // and animals.
            let mut solids = collide::solids(traffic.as_deref());
            solids.extend(people.iter().map(|q| (q.position, 0.3)));
            solids.extend(beasts.iter().map(|q| (q.position, 0.3)));
            let mut at = Vec2::new(p.x, p.z);
            collide::push_out(&mut at, 0.3, &solids);
            p.x = at.x;
            p.z = at.y;
        }
        if player.flying {
            p.y = p.y.clamp(0.2, 120.0);
            player.steady = p;
            player.bob = 0.0;
            transform.translation = p;
            continue;
        }
        p.y = EYE_HEIGHT;
        player.steady = p;
        // The bob of walking: the head dips at each footfall and rises over the planted foot,
        // and sways toward the foot bearing the weight; running is quicker, longer and harder.
        let ease = 1.0 - (-dt * 8.0).exp();
        player.running += ((if run && moving { 1.0 } else { 0.0 }) - player.running) * ease;
        player.bob += ((if moving { 1.0 } else { 0.0 }) - player.bob) * ease;
        let step_length = 0.75 + 0.55 * player.running;
        player.stride += step.length() / step_length;
        let phase = player.stride * PI;
        let rise = 0.04 + 0.05 * player.running;
        let sway = 0.025 + 0.02 * player.running;
        let lift = (phase.cos().abs() * 2.0 - 1.0) * rise * 0.5 * player.bob;
        let side = phase.sin() * sway * player.bob;
        let right = Quat::from_rotation_y(player.yaw) * Vec3::X;
        transform.translation = p + Vec3::Y * lift + right * side;
        let roll = -phase.sin() * (0.006 + 0.01 * player.running) * player.bob;
        transform.rotation = Quat::from_euler(EulerRot::YXZ, player.yaw, player.pitch, roll);
    }
}

#[derive(Default)]
struct FpsCounter {
    frames: u32,
    elapsed: f32,
}

fn show_stats(time: Res<Time>, window: Res<Window>, mut counter: Local<FpsCounter>) {
    counter.frames += 1;
    counter.elapsed += time.delta_secs();
    if counter.elapsed >= 0.5 {
        let fps = counter.frames as f32 / counter.elapsed;
        log::debug!("{fps:.0} fps");
        window.set_title(&format!("mira — Paris, 1810 — {fps:.0} fps"));
        *counter = FpsCounter::default();
    }
}

/// F2 saves a screenshot. `MIRA_SCREENSHOT=<path>` saves one after `MIRA_SCREENSHOT_DELAY`
/// seconds (default 6) and quits.
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
