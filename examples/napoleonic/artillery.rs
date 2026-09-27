//! The foot battery: six Gribeauval 12-pounders firing at the butts.

use std::f32::consts::{FRAC_PI_2, TAU};

use voxl::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::{GltfScene, Image},
};

use crate::{
    materials::asset,
    scene::{ground, yaw_toward, BATTERY, BUTTS, EAST},
    smoke::Smoke,
    terrain::hash,
};

pub struct Gun {
    home: Vec2,
    /// When the gun first fires; it fires again every `CYCLE` seconds after.
    phase: f32,
    last_cycle: f32,
    /// How far back it has rolled from its place.
    back: f32,
}
impl Component for Gun {}

/// Roundshot in flight: when and where each lands.
#[derive(Default)]
pub struct Shots(Vec<(f32, Vec3)>);

/// Builds meshes by appending shapes, with UVs projected in metres along each face's axis.
#[derive(Default)]
struct Parts(Mesh);

impl Parts {
    fn add(&mut self, shape: &Mesh, t: Mat4) {
        let start = self.0.vertices.len();
        self.0.append(shape, t);
        let normals = Mat3::from_mat4(t).inverse().transpose();
        for (v, src) in self.0.vertices[start..].iter_mut().zip(&shape.vertices) {
            let p = Vec3::from(v.position);
            let n = (normals * Vec3::from(src.normal)).abs();
            v.uv = if n.y >= n.x && n.y >= n.z {
                [p.x, p.z]
            } else if n.x >= n.z {
                [p.z, p.y]
            } else {
                [p.x, p.y]
            };
            // Campaign dirt: mud caked thick near the ground and splashed higher in blotches,
            // and a general grime over everything.
            let blotch = hash(21, p.x * 7.0 + p.z * 3.0, p.y * 9.0);
            let mud = (1.0 - (p.y / 0.7).clamp(0.0, 1.0)).powf(0.7) * 0.85 + if blotch > 0.8 { 0.35 } else { 0.0 };
            let grime = 0.72 + 0.2 * hash(23, p.x * 13.0, p.y * 11.0 + p.z * 5.0);
            let mud_color = Vec3::new(0.32, 0.25, 0.17);
            let c = Vec3::splat(grime).lerp(mud_color, mud.min(0.9));
            v.color = c.into();
        }
    }

    fn beam(&mut self, from: Vec3, to: Vec3, w: f32, d: f32) {
        let dir = to - from;
        let rot = Quat::from_rotation_arc(Vec3::Y, dir.normalize());
        self.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(w, dir.length(), d), rot, (from + to) * 0.5));
    }

    fn rod(&mut self, from: Vec3, to: Vec3, r: f32, segments: u32) {
        let dir = to - from;
        let rot = Quat::from_rotation_arc(Vec3::Y, dir.normalize());
        self.add(&Mesh::cylinder(1.0, 1.0, segments), Mat4::from_scale_rotation_translation(Vec3::new(r, dir.length(), r), rot, (from + to) * 0.5));
    }
}

/// A wheel on an axle along X: iron tyre, felloes, spokes and hub.
fn wheel(wood: &mut Parts, iron: &mut Parts, hub: Vec3, radius: f32) {
    let segments = 18;
    let at = |a: f32, r: f32| hub + Vec3::new(0.0, a.cos() * r, a.sin() * r);
    for i in 0..segments {
        let (a0, a1) = (i as f32 / segments as f32 * TAU, (i + 1) as f32 / segments as f32 * TAU);
        iron.beam(at(a0, radius - 0.02), at(a1, radius - 0.02), 0.1, 0.035);
        wood.beam(at(a0, radius - 0.08), at(a1, radius - 0.08), 0.09, 0.09);
    }
    for i in 0..12 {
        let a = i as f32 / 12.0 * TAU;
        wood.beam(hub + Vec3::X * (if i % 2 == 0 { 0.04 } else { -0.04 }), at(a, radius - 0.1), 0.045, 0.06);
    }
    wood.rod(hub - Vec3::X * 0.18, hub + Vec3::X * 0.18, 0.12, 14);
    iron.rod(hub - Vec3::X * 0.2, hub - Vec3::X * 0.12, 0.13, 14);
    iron.rod(hub + Vec3::X * 0.12, hub + Vec3::X * 0.2, 0.13, 14);
}

/// The gun on its carriage, muzzle toward -Z, as (painted wood, iron, bronze, sooty muzzle). The
/// wheels are made separately, to turn.
fn cannon() -> (Mesh, Mesh, Mesh, Mesh) {
    let (mut wood, mut iron, mut bronze, mut soot) = (Parts::default(), Parts::default(), Parts::default(), Parts::default());
    for side in [-1.0f32, 1.0] {
        // The cheeks: two heavy timbers running back and down to the trail.
        wood.beam(Vec3::new(side * 0.21, 1.02, -0.5), Vec3::new(side * 0.13, 0.14, 2.8), 0.11, 0.32);
        iron.beam(Vec3::new(side * 0.275, 1.0, -0.45), Vec3::new(side * 0.2, 0.2, 2.7), 0.012, 0.06);
    }
    iron.rod(Vec3::new(-0.95, 0.73, 0.0), Vec3::new(0.95, 0.73, 0.0), 0.05, 10);
    wood.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(0.34, 0.24, 0.34), Quat::IDENTITY, Vec3::new(0.0, 0.86, 0.0)));
    wood.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(0.26, 0.12, 0.5), Quat::from_rotation_x(-0.3), Vec3::new(0.0, 0.3, 2.35)));
    iron.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(0.3, 0.14, 0.12), Quat::IDENTITY, Vec3::new(0.0, 0.08, 2.82)));
    iron.rod(Vec3::new(0.0, 0.3, 2.6), Vec3::new(0.0, 0.38, 2.6), 0.07, 10);
    // The ammunition chest the cartridges come from, on the ground behind the trail.
    wood.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(0.9, 0.5, 0.55), Quat::IDENTITY, Vec3::new(-2.3, 0.25, 4.9)));
    iron.add(&Mesh::cube(1.0), Mat4::from_scale_rotation_translation(Vec3::new(0.94, 0.05, 0.59), Quat::IDENTITY, Vec3::new(-2.3, 0.5, 4.9)));
    // Elevating screw under the breech.
    iron.rod(Vec3::new(0.0, 0.75, 0.75), Vec3::new(0.0, 1.05, 0.75), 0.03, 8);

    // The barrel: breech toward the trail, muzzle forward, sitting on its trunnions. The last
    // half metre is blackened by the smoke of every shot.
    let axis = Quat::from_rotation_x(-FRAC_PI_2);
    let barrel_at = |z: f32| Vec3::new(0.0, 1.15, z);
    bronze.add(&Mesh::frustum(0.19, 0.14, 1.0, 24), Mat4::from_rotation_translation(axis, barrel_at(0.1)));
    bronze.add(&Mesh::frustum(0.14, 0.126, 0.95, 24), Mat4::from_rotation_translation(axis, barrel_at(-0.87)));
    soot.add(&Mesh::frustum(0.126, 0.12, 0.5, 24), Mat4::from_rotation_translation(axis, barrel_at(-1.6)));
    for (z, r) in [(0.6, 0.2), (-0.4, 0.155), (-0.62, 0.15)] {
        bronze.add(&Mesh::cylinder(r, 0.05, 24), Mat4::from_rotation_translation(axis, barrel_at(z)));
    }
    for (z, r) in [(-1.72, 0.14), (-1.84, 0.15)] {
        soot.add(&Mesh::cylinder(r, 0.05, 24), Mat4::from_rotation_translation(axis, barrel_at(z)));
    }
    bronze.add(&Mesh::uv_sphere(0.09, 12, 8), Mat4::from_translation(barrel_at(0.72)));
    bronze.rod(Vec3::new(-0.27, 1.15, -0.2), Vec3::new(0.27, 1.15, -0.2), 0.065, 12);
    // The two lifting handles, cast as dolphins.
    for side in [-1.0f32, 1.0] {
        for (z0, z1) in [(-0.35, -0.15), (-0.15, 0.05)] {
            let a = Vec3::new(side * 0.07, 1.3, z0);
            let b = Vec3::new(side * 0.07, 1.36, (z0 + z1) * 0.5);
            let c = Vec3::new(side * 0.07, 1.3, z1);
            bronze.rod(a, b, 0.018, 6);
            bronze.rod(b, c, 0.018, 6);
        }
    }
    (wood.0, iron.0, bronze.0, soot.0)
}

/// One wheel, about its hub at the origin: (wood, iron).
fn loose_wheel() -> (Mesh, Mesh) {
    let (mut wood, mut iron) = (Parts::default(), Parts::default());
    wheel(&mut wood, &mut iron, Vec3::ZERO, WHEEL_RADIUS);
    (wood.0, iron.0)
}

const WHEEL_RADIUS: f32 = 0.73;
/// A shot, the gun run back up, sponged, loaded, rammed, primed and fired again.
const CYCLE: f32 = 20.0;
/// How far the gun rolls back when it fires.
const RECOIL: f32 = 1.7;
/// Where the trail's end rests on the ground, about which the muzzle kicks up.
const TRAIL: Vec3 = Vec3::new(0.0, 0.0, 2.8);

/// How far back the gun is `t` seconds after it fired: a violent kick that the ground brakes,
/// a pause while the crew close on it, then run back up to its place.
fn recoil(t: f32) -> f32 {
    let smooth = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    if t < 0.45 {
        let s = 1.0 - t / 0.45;
        RECOIL * (1.0 - s * s * s)
    } else if t < 1.5 {
        RECOIL
    } else {
        RECOIL * (1.0 - smooth((t - 1.5) / 3.5))
    }
}

/// The man at each post: his name (the model is crew_<name>), where he stands and faces at his
/// post, and where he stands and faces on the wheels or trail when the gun is run up (none for
/// the man who brings up the cartridges), in gun space (x right, z toward the trail). Must match
/// `POSTS` in tools/soldiers.py.
struct Post {
    name: &'static str,
    spot: Vec2,
    facing: Vec2,
    push: Option<(Vec2, Vec2)>,
}

const POSTS: [Post; 5] = [
    Post { name: "sponge", spot: Vec2::new(1.05, -1.75), facing: Vec2::new(-1.0, 0.0), push: Some((Vec2::new(1.25, 0.15), Vec2::new(0.0, -1.0))) },
    Post { name: "load", spot: Vec2::new(-0.85, -1.8), facing: Vec2::new(1.0, 0.0), push: Some((Vec2::new(-1.25, 0.15), Vec2::new(0.0, -1.0))) },
    Post { name: "vent", spot: Vec2::new(0.6, 0.45), facing: Vec2::new(-1.0, 0.0), push: Some((Vec2::new(0.35, 2.35), Vec2::new(0.0, -1.0))) },
    Post { name: "fire", spot: Vec2::new(-1.15, 0.9), facing: Vec2::new(1.0, 0.0), push: Some((Vec2::new(-0.35, 2.35), Vec2::new(0.0, -1.0))) },
    // No. 5 waits by the ammunition chest behind the gun and brings each cartridge up to No. 2.
    Post { name: "carry", spot: Vec2::new(-2.3, 5.6), facing: Vec2::new(0.5, -1.0), push: None },
];

/// The muzzle and the vent, in gun space: what the crew's hands go to.
const MUZZLE: Vec3 = Vec3::new(0.0, 1.15, -1.95);
const VENT: Vec3 = Vec3::new(0.0, 1.34, 0.55);

/// Where a man's hands (left, right) go, in gun space, `t` seconds into a clip: on the wheel
/// rims or the trail as he runs the gun up, the thumb on the vent, into the muzzle with the
/// cartridge. `None` leaves a hand to the clip.
fn hands(post: usize, clip: &str, t: f32) -> [Option<Vec3>; 2] {
    match (post, clip) {
        // Nos. 1 and 2 lean on the top of the wheels, facing the muzzle's way (their right is
        // the gun's +x).
        (0 | 1, "push") => {
            let side = if post == 0 { 1.0 } else { -1.0 };
            let rim = Vec3::new(0.8 * side, 1.25, -0.25);
            [Some(rim + Vec3::new(-0.09, 0.0, 0.06)), Some(rim + Vec3::new(0.09, 0.04, -0.08))]
        }
        // Nos. 3 and 4 bend to the trail.
        (2 | 3, "push") => {
            let grip = Vec3::new(if post == 2 { 0.12 } else { -0.12 }, 0.62, 2.2);
            [Some(grip - Vec3::X * 0.1), Some(grip + Vec3::X * 0.1)]
        }
        // No. 3 faces the gun from its right: his left hand is toward the trail.
        (2, "thumb") => [Some(VENT + Vec3::new(0.0, 0.04, 0.0)), None],
        (2, "prime") if (0.25..1.5).contains(&t) => [Some(VENT + Vec3::new(0.02, 0.05, 0.03)), Some(VENT + Vec3::new(0.05, 0.1, -0.06))],
        (2, "prime") => [Some(VENT + Vec3::new(0.0, 0.04, 0.0)), None],
        // No. 2 puts the cartridge into the muzzle with both hands (facing +x, his right is
        // toward the trail).
        (1, "load") if (1.15..1.95).contains(&t) => {
            let mouth = MUZZLE + Vec3::Y * 0.08;
            [Some(mouth - Vec3::Z * 0.07), Some(mouth + Vec3::Z * 0.07)]
        }
        _ => [None, None],
    }
}

/// Where No. 5 hands the cartridge to the loader.
const HANDOVER: (Vec2, Vec2) = (Vec2::new(-1.55, -1.1), Vec2::new(0.6, -0.8));

/// Where a man is during part of the drill.
#[derive(Clone, Copy)]
enum Place {
    Post,
    /// On the wheels or trail, moving with the gun.
    Push,
    Handover,
}

/// What a man does over a stretch of the cycle.
#[derive(Clone, Copy)]
enum Act {
    /// Stands at a place, playing a clip (looping it if the stretch is longer).
    At(Place, &'static str),
    /// Walks or runs from one place to another.
    Go(Place, Place),
}

/// Each man's part in the 20-second drill, as (start, end, what he does). The gun fires at 0.
fn drill(post: usize) -> Vec<(f32, f32, Act)> {
    use Act::*;
    use Place::*;
    let idle = if post.is_multiple_of(2) { "idle" } else { "idle2" };
    let idle = if post == 0 { "idle_staff" } else { idle };
    let mut run_up = vec![(0.0, 0.7, At(Post, idle)), (0.7, 1.5, Go(Post, Push)), (1.5, 5.0, At(Push, "push")), (5.0, 5.9, Go(Push, Post))];
    let work = match post {
        0 => vec![(5.9, 6.2, At(Post, idle)), (6.2, 8.2, At(Post, "sponge")), (8.2, 10.1, At(Post, idle)), (10.1, 12.9, At(Post, "ram")), (12.9, CYCLE, At(Post, idle))],
        1 => vec![(5.9, 7.9, At(Post, idle)), (7.9, 10.0, At(Post, "load")), (10.0, CYCLE, At(Post, idle))],
        2 => vec![(5.9, 13.0, At(Post, "thumb")), (13.0, 14.9, At(Post, "prime")), (14.9, CYCLE, At(Post, idle))],
        3 => vec![(5.9, 17.0, At(Post, idle)), (17.0, 18.4, At(Post, "blow")), (18.4, 18.9, At(Post, idle)), (18.9, CYCLE, At(Post, "fire"))],
        _ => {
            run_up = vec![];
            vec![(0.0, 5.3, At(Post, idle)), (5.3, 7.3, Go(Post, Handover)), (7.3, 8.1, At(Handover, "hand")), (8.1, 10.4, Go(Handover, Post)), (10.4, CYCLE, At(Post, idle))]
        }
    };
    run_up.into_iter().chain(work).collect()
}

/// A gunner at his post by a gun.
pub struct Crewman {
    gun: Entity,
    post: usize,
    /// A moment's delay behind the drill, different for each man, so no two move as one.
    lag: f32,
    /// How fast his walk and run clips cover the ground.
    walk: f32,
    run: f32,
}
impl Component for Crewman {}

/// A gun wheel, turned by how far the gun has rolled.
pub struct GunWheel {
    gun: Entity,
}
impl Component for GunWheel {}

fn spawn_gun(world: &mut World, parts: &[(Handle<Mesh>, Material)], wheel: &[(Handle<Mesh>, Material)], home: Vec2, phase: f32) -> Entity {
    let root = world.spawn((
        Transform::from_translation(ground(home)).with_rotation(Quat::from_rotation_y(yaw_toward(EAST))),
        Gun {
            home,
            phase,
            last_cycle: -1.0,
            back: 0.0,
        },
    ));
    for (mesh, material) in parts {
        world.spawn((Transform::IDENTITY, Mesh3d(*mesh), *material, Parent(root)));
    }
    for side in [-1.0f32, 1.0] {
        let hub = world.spawn((Transform::from_translation(Vec3::new(side * 0.8, WHEEL_RADIUS, 0.0)), GunWheel { gun: root }, Parent(root)));
        for (mesh, material) in wheel {
            world.spawn((Transform::IDENTITY, Mesh3d(*mesh), *material, Parent(hub)));
        }
    }
    root
}

pub fn setup(world: &mut World) {
    let wood_normal = std::fs::read(asset("textures/oak_wood_planks_nor_gl.jpg"))
        .ok()
        .and_then(|b| Image::from_bytes(&b, false).ok());
    let (wood, iron, bronze, soot) = cannon();
    let (wheel_wood, wheel_iron) = loose_wheel();
    let images = world.resource_mut::<Assets<Image>>();
    let normal = wood_normal.map(|img| images.add(img));
    // A gun on campaign: the olive paint faded and chipped, caked with mud to the axle, the
    // ironwork rusted, the bronze gone dull and dark, the muzzle black with powder.
    let paint = Material {
        color: Color::hex(0x4a5236),
        roughness: 0.85,
        normal_texture: normal,
        normal_strength: 0.8,
        weathering: 1.6,
        ..Default::default()
    };
    let rust = Material {
        color: Color::hex(0x2c231c),
        roughness: 0.78,
        metallic: 0.6,
        weathering: 1.0,
        ..Default::default()
    };
    // Bronze left out in all weathers goes brown and dull, streaked with verdigris; only where
    // hands grip does it shine.
    let bronze_material = Material {
        color: Color::hex(0x4e3c24),
        roughness: 0.62,
        metallic: 0.75,
        weathering: 1.2,
        ..Default::default()
    };
    let soot_material = Material {
        color: Color::hex(0x16120f),
        roughness: 0.9,
        metallic: 0.3,
        ..Default::default()
    };
    let meshes = world.resource_mut::<Assets<Mesh>>();
    let parts = vec![
        (meshes.add(wood), paint),
        (meshes.add(iron), rust),
        (meshes.add(bronze), bronze_material),
        (meshes.add(soot), soot_material),
    ];
    let wheel_parts = vec![(meshes.add(wheel_wood), paint), (meshes.add(wheel_iron), rust)];

    // The crew, one model per post, each with its drill.
    let crews: Vec<Option<(GltfScene, f32, f32)>> = POSTS
        .iter()
        .map(|post| {
            let name = post.name;
            let scene = world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                GltfScene::load(asset(&format!("models/army/crew_{name}.glb")), meshes, world.resource_mut::<Assets<Image>>())
                    .map_err(|e| log::error!("couldn't load the {name} crewman: {e:#}"))
                    .ok()
            });
            scene.map(|mut scene| {
                // The walks and runs were captured moving over the ground; take that out (the
                // drill moves him) and keep how fast they went.
                let (mut walk, mut run) = (1.0, 2.5);
                if let Some(skeleton) = scene.skeleton.clone() {
                    for clip in &mut scene.clips {
                        let is = |n: &str| clip.name == n || clip.name.starts_with(&format!("{n}_"));
                        if is("walk") || is("run") {
                            let fast = is("run");
                            let speed = std::sync::Arc::make_mut(clip).remove_root_motion(&skeleton);
                            if speed > 0.1 {
                                if fast { run = speed } else { walk = speed }
                            }
                        }
                    }
                }
                (scene, walk, run)
            })
        })
        .collect();
    for i in 0..6 {
        let home = BATTERY + EAST.perp() * -(i as f32) * 15.0;
        let gun = spawn_gun(world, &parts, &wheel_parts, home, 5.0 + i as f32 * 2.7);
        for (post, crew) in crews.iter().enumerate() {
            let Some((scene, walk, run)) = crew else { continue };
            let man = scene.spawn_animated(world, Transform::from_translation(ground(home)), |part| {
                let mut m = part.material;
                if !part.material_name.contains("brass") && !part.material_name.contains("steel") && !part.material_name.contains("iron") {
                    m.metallic = 0.0;
                }
                if part.material_name.ends_with("_head") {
                    m.subsurface = 0.7;
                }
                m
            });
            let lag = hash(9, home.y + post as f32 * 3.1, 1.0) * 0.35;
            world.insert(man, Crewman { gun, post, lag, walk: *walk, run: *run });
            // Feet that stay planted as the clips shift his weight and as he goes with the gun,
            // stepping when carried too far; hands that go to the gun wherever it has rolled.
            if let Some(skeleton) = scene.skeleton.clone() {
                let cycle = scene.clips.iter().find(|c| c.name == "walk").map_or(1.1, |c| c.duration);
                let mut gait = Gait::biped(&skeleton, "Bip01 ", "Toe0", Pattern::new(0.0, walk * cycle, 0.62, &voxl::render::gait::BIPED, 0.09));
                gait.follow_clip = true;
                gait.tolerance = 0.12;
                gait.bob = 0.0;
                world.insert(man, gait);
                world.insert(man, Reach::arms(&skeleton, "Bip01 "));
            }
        }
    }
    world.init_resource::<Shots>();
}

/// Each gun fires once a cycle: flash and smoke, the kick and recoil, and the crew run it back
/// up. Shot lands on the butts about a second later.
pub fn fire_guns(time: Res<Time>, mut smoke: ResMut<Smoke>, mut shots: ResMut<Shots>, mut guns: Query<(&mut Transform, &mut Gun)>) {
    let now = time.elapsed_secs();
    let forward = Vec3::new(EAST.x, 0.0, EAST.y);
    for (mut transform, mut gun) in &mut guns {
        let t = gun.cycle_time(now);
        let cycle = ((now - gun.phase) / CYCLE).floor();
        if now >= gun.phase && cycle > gun.last_cycle {
            gun.last_cycle = cycle;
            let muzzle = ground(gun.home) + forward * 1.9 + Vec3::Y * 1.15;
            smoke.cannon(muzzle, forward);
            let target = BUTTS[1] + Vec2::new(0.0, (hash(5, gun.home.y, now) - 0.5) * 30.0);
            shots.0.push((now + 1.0, ground(target)));
        }
        let back = if now >= gun.phase { recoil(t) } else { 0.0 };
        gun.back = back;
        // The muzzle kicks up about the trail as the gun leaps back, and settles.
        let kick = if now >= gun.phase { 0.09 * (-t * 7.0).exp() * (t * 9.0).cos().max(0.0) } else { 0.0 };
        let base = ground(gun.home - EAST * back);
        let yaw = Quat::from_rotation_y(yaw_toward(EAST));
        let pivot = Mat4::from_translation(TRAIL) * Mat4::from_rotation_x(kick) * Mat4::from_translation(-TRAIL);
        let (_, rotation, translation) = (Mat4::from_rotation_translation(yaw, base) * pivot).to_scale_rotation_translation();
        transform.translation = translation;
        transform.rotation = rotation;
    }
    shots.0.retain(|(at, place)| {
        if now >= *at {
            smoke.dust(*place);
            false
        } else {
            true
        }
    });
}

impl Gun {
    fn cycle_time(&self, now: f32) -> f32 {
        (now - self.phase).rem_euclid(CYCLE)
    }
}

/// Turns the wheels by how far each gun has rolled.
pub fn roll_wheels(guns: Query<&Gun>, mut wheels: Query<(&GunWheel, &mut Transform)>) {
    for (wheel, mut transform) in &mut wheels {
        if let Some(gun) = guns.get(wheel.gun) {
            transform.rotation = Quat::from_rotation_x(gun.back / WHEEL_RADIUS);
        }
    }
}

/// Moves each crewman through his part in the drill, in time with his gun: standing at his
/// post, going to the wheels or the trail and back, working the gun.
#[allow(clippy::type_complexity)]
pub fn work_guns(
    time: Res<Time>,
    guns: Query<(&Gun, &Transform), Without<Crewman>>,
    mut crew: Query<(&Crewman, &mut Transform, &mut Animator, Option<&mut Gait>, Option<&mut Reach>)>,
) {
    let now = time.elapsed_secs();
    let gun_yaw = Quat::from_rotation_y(yaw_toward(EAST));
    let yaw_of = |f: Vec2| f.x.atan2(f.y);
    for (man, mut transform, mut animator, gait, reach) in &mut crew {
        let Some((gun, gun_transform)) = guns.get(man.gun) else { continue };
        let post = &POSTS[man.post];
        let t = (gun.cycle_time(now) - man.lag).rem_euclid(CYCLE);
        let place = |p: Place| -> (Vec2, Vec2) {
            match p {
                Place::Post => (post.spot, post.facing),
                Place::Push => post.push.map(|(s, f)| (s + Vec2::new(0.0, gun.back), f)).unwrap_or((post.spot, post.facing)),
                Place::Handover => HANDOVER,
            }
        };
        let schedule = drill(man.post);
        let Some(&(start, end, act)) = schedule.iter().find(|(a, b, _)| t >= *a && t < *b) else { continue };
        let (at, facing, clip, speed) = match act {
            Act::At(p, clip) => {
                let (at, facing) = place(p);
                (at, facing, clip, 1.0)
            }
            Act::Go(from, to) => {
                let ((a, _), (b, fb)) = (place(from), place(to));
                let f = ((t - start) / (end - start)).clamp(0.0, 1.0);
                // Ease off and on, turning to face the way he goes and then his new task.
                let e = f * f * (3.0 - 2.0 * f);
                let at = a.lerp(b, e);
                let dir = (b - a).normalize_or(fb);
                let facing = if f > 0.8 { dir.lerp(fb, (f - 0.8) / 0.2) } else { dir };
                let speed = (b - a).length() / (end - start) * 1.3;
                let (clip, pace) = if speed > 1.8 { ("run", man.run) } else { ("walk", man.walk) };
                (at, facing, clip, speed / pace.max(0.1))
            }
        };
        animator.speed = speed;
        animator.play(clip, 0.3, 0.0);
        // Walking and running are captured from life, feet and all; standing and working, his
        // feet are locked to the ground.
        if let Some(mut gait) = gait {
            gait.weight = if matches!(act, Act::Go(..)) { 0.0 } else { 1.0 };
        }
        if let Some(mut reach) = reach {
            let to_world = gun_transform.matrix();
            let targets = if matches!(act, Act::At(..)) { hands(man.post, clip, t - start) } else { [None, None] };
            for (limb, target) in reach.limbs.iter_mut().zip(targets) {
                limb.reach(target.map(|p| to_world.transform_point3(p)));
            }
        }
        let offset = gun_yaw * Vec3::new(at.x, 0.0, at.y);
        transform.translation = ground(gun.home + Vec2::new(offset.x, offset.z));
        transform.rotation = gun_yaw * Quat::from_rotation_y(yaw_of(facing));
    }
}
