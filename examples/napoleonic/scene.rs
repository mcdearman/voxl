//! Lays out the countryside: land, river, village, bridge, windmill, woods and hedgerows,
//! and where the army stands.

use std::f32::consts::TAU;

use mira::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::Image,
};

use crate::{
    buildings::{self, Build, Style},
    grass,
    materials::{PLASTER, SLATE, STONE, TILES},
    terrain::{self, hash, walk_height, BRIDGE},
    vegetation::{Forest, Species},
    world::{tint, Kind, WorldGeometry, WorldMesh, NEUTRAL},
};

pub const VILLAGE: Vec2 = Vec2::new(130.0, 44.0);
/// Centre of the battalion's front rank. It faces east, toward the butts.
pub const LINE: Vec2 = Vec2::new(410.0, -25.0);
/// The battery's southernmost gun; the others follow northward toward the road.
pub const BATTERY: Vec2 = Vec2::new(335.0, 160.0);
pub const CAVALRY: Vec2 = Vec2::new(250.0, -150.0);
pub const CAMP: Vec2 = Vec2::new(-45.0, -150.0);
pub const BUTTS: [Vec2; 2] = [Vec2::new(650.0, -25.0), Vec2::new(700.0, 150.0)];
pub const EAST: Vec2 = Vec2::X;

/// A point on the ground, or on the bridge.
pub fn ground(p: Vec2) -> Vec3 {
    Vec3::new(p.x, walk_height(p.x, p.y), p.y)
}

pub fn knoll() -> Vec2 {
    terrain::high_point(Vec2::new(265.0, -20.0), 45.0)
}

pub fn windmill() -> Vec2 {
    terrain::high_point(Vec2::new(190.0, -300.0), 70.0)
}

/// How trodden the ground is where the army has been standing and marching, 0 to 1.
pub fn trampled(p: Vec2) -> f32 {
    let rect = |center: Vec2, half: Vec2, feather: f32| {
        let d = (p - center).abs() - half;
        1.0 - terrain::smoothstep(0.0, feather, d.max_element())
    };
    let circle = |center: Vec2, r: f32| 1.0 - terrain::smoothstep(r * 0.6, r, (p - center).length());
    [
        rect(LINE + Vec2::new(-4.0, 0.0), Vec2::new(9.0, 20.0), 6.0),
        rect(BATTERY + Vec2::new(-15.0, -37.5), Vec2::new(28.0, 44.0), 8.0),
        rect(CAVALRY + Vec2::new(-2.0, 0.0), Vec2::new(6.0, 11.0), 6.0),
        rect(CAMP, Vec2::new(28.0, 28.0), 10.0),
        circle(knoll(), 18.0),
        circle(BUTTS[0], 20.0) * 0.6,
        circle(BUTTS[1], 20.0) * 0.6,
    ]
    .into_iter()
    .fold(0.0, f32::max)
}

/// Somewhere trees and hedges must leave room for people.
fn reserved(p: Vec2) -> bool {
    trampled(p) > 0.05 || (p - windmill()).length() < 25.0
}

/// Builds the world. Runs once at startup, with the whole world to hand.
pub fn setup(world: &mut World) {
    let started = std::time::Instant::now();
    let land = terrain::build_ground();
    log::info!("terrain in {:.1?}", started.elapsed());

    let mut solid = WorldMesh::default();
    village(&mut solid);
    {
        let mut b = Build {
            mesh: &mut solid,
            place: Mat4::IDENTITY,
        };
        let bridge = *BRIDGE;
        b.at(
            Vec3::new(bridge.center.x, 0.0, bridge.center.y),
            (-bridge.direction.x).atan2(-bridge.direction.y),
        );
        buildings::bridge(&mut b, bridge.half_length, bridge.half_width, bridge.deck);
        b.at(ground(windmill()), yaw_toward(Vec2::new(-1.0, 0.3)));
        buildings::windmill_tower(&mut b);
        camp(&mut b);
        for butt in BUTTS {
            b.at(ground(butt), yaw_toward(-EAST));
            // Earth banks with target boards in front.
            b.cuboid(Vec3::new(0.0, 0.6, 1.5), Vec3::new(24.0, 3.2, 4.0), crate::materials::SOIL, NEUTRAL);
            for i in -3..=3 {
                let x = i as f32 * 3.0;
                b.cuboid(Vec3::new(x, 2.0, -0.6), Vec3::new(1.6, 1.8, 0.06), crate::materials::WOOD, tint(Vec3::splat(1.8), 0.0));
                for s in [-0.75, 0.75] {
                    b.cuboid(Vec3::new(x + s, 1.2, -0.55), Vec3::new(0.1, 3.0, 0.1), crate::materials::WOOD, NEUTRAL);
                }
            }
        }
    }
    let mut geometry = vec![
        (Kind::Ground, land.detail.clone()),
        (Kind::Ground, land.outer.clone()),
        (Kind::Solid, solid),
        (Kind::Water, terrain::water()),
    ];
    geometry.retain(|(_, m)| !m.indices.is_empty());
    world.insert_resource(WorldGeometry(geometry));

    grass::init(world, &land, &trampled);

    let forest = world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
        Forest::load(meshes, world.resource_mut::<Assets<Image>>())
    });
    match forest {
        Ok(forest) => plant(world, &forest),
        Err(err) => log::error!("couldn't load the trees: {err:#}"),
    }
    spawn_sails(world);
    log::info!("world laid out in {:.1?}", started.elapsed());
}

pub fn yaw_toward(direction: Vec2) -> f32 {
    (-direction.x).atan2(-direction.y)
}

fn village(mesh: &mut WorldMesh) {
    let wall_tints = [
        Vec3::new(1.12, 1.08, 1.0),
        Vec3::new(1.05, 1.0, 0.9),
        Vec3::new(1.15, 1.12, 1.05),
        Vec3::new(0.98, 0.94, 0.86),
    ];
    let shutter_tints = [
        Vec3::new(0.55, 0.75, 0.8),
        Vec3::new(0.65, 0.8, 0.6),
        Vec3::new(0.9, 0.55, 0.45),
        Vec3::new(0.7, 0.75, 0.78),
        Vec3::new(0.45, 0.55, 0.7),
    ];
    let pick = |r: f32, n: usize| (r * n as f32) as usize % n;
    let style = |r: &dyn Fn(u32) -> f32| Style {
        wall: if r(20) < 0.25 { STONE } else { PLASTER },
        wall_tint: tint(wall_tints[pick(r(21), wall_tints.len())], 0.0),
        roof: if r(22) < 0.55 { SLATE } else { TILES },
        roof_tint: tint(Vec3::splat(0.9 + r(23) * 0.2), 0.0),
        shutters: tint(shutter_tints[pick(r(24), shutter_tints.len())], 0.0),
    };
    let mut b = Build {
        mesh,
        place: Mat4::IDENTITY,
    };
    let church_x = 135.0;
    for side in [-1.0f32, 1.0] {
        let mut x = 40.0;
        while x < 228.0 {
            let r = |k: u32| hash(80 + k, x, side);
            let width = 7.0 + r(0) * 6.0;
            let depth = 6.5 + r(1) * 2.0;
            let center_x = x + width * 0.5;
            let square = side < 0.0 && (church_x - 22.0..church_x + 22.0).contains(&center_x);
            if !square && r(2) > 0.12 {
                let (p, t) = terrain::road_at(center_x);
                let normal = t.perp() * side;
                let at = p + normal * (depth * 0.5 + 5.5 + r(3) * 2.0);
                b.at(ground(at), yaw_toward(-normal));
                let stories = if r(7) < 0.55 { 2 } else { 1 };
                buildings::house(&mut b, width, depth, stories, &style(&r), r(8), false);
                if r(9) < 0.5 {
                    let barn = at + normal * (depth + 7.0) + t * (r(10) - 0.5) * 6.0;
                    b.at(ground(barn), yaw_toward(t));
                    let mut s = style(&|k| r(k + 10));
                    s.wall = STONE;
                    buildings::house(&mut b, 7.0 + r(13) * 4.0, 7.5, 1, &s, 0.3, true);
                }
            }
            x += width + 0.5 + r(14) * r(14) * 9.0;
        }
    }

    let (p, t) = terrain::road_at(church_x);
    let away = -t.perp();
    let church = p + away * 26.0;
    b.at(ground(church) - Vec3::Y * 0.3, yaw_toward(away));
    buildings::church(&mut b);
    // A churchyard wall.
    let height = |q: Vec2| walk_height(q.x, q.y);
    let corner = |u: f32, v: f32| {
        let q = church + t * u + away * v;
        Vec3::new(q.x, 0.0, q.y)
    };
    b.place = Mat4::IDENTITY;
    for (a, c) in [
        (corner(-16.0, -14.0), corner(-16.0, 24.0)),
        (corner(-16.0, 24.0), corner(16.0, 24.0)),
        (corner(16.0, 24.0), corner(16.0, -14.0)),
        (corner(16.0, -14.0), corner(4.0, -14.0)),
        (corner(-4.0, -14.0), corner(-16.0, -14.0)),
    ] {
        buildings::stone_wall(&mut b, a, c, height);
    }

    // Farmsteads out in the fields, with paddocks fenced in.
    for (i, at) in [Vec2::new(-220.0, -60.0), Vec2::new(480.0, 260.0), Vec2::new(-150.0, 300.0), Vec2::new(560.0, -300.0)]
        .into_iter()
        .enumerate()
    {
        let r = |k: u32| hash(90 + k, i as f32, 0.0);
        let yaw = r(3) * TAU;
        b.at(ground(at), yaw);
        buildings::house(&mut b, 11.0, 7.5, 2, &style(&r), 0.3, false);
        let turn = Vec2::from_angle(-yaw);
        let barn = at + turn.rotate(Vec2::new(0.0, 15.0));
        b.at(ground(barn), yaw);
        let mut s = style(&|k| r(k + 5));
        s.wall = STONE;
        buildings::house(&mut b, 16.0, 9.0, 1, &s, 0.0, true);
        b.place = Mat4::IDENTITY;
        let paddock: Vec<Vec3> = [(-14.0, -10.0), (14.0, -10.0), (14.0, -34.0), (-14.0, -34.0)]
            .iter()
            .map(|(u, v)| {
                let q = at + turn.rotate(Vec2::new(*u, *v));
                Vec3::new(q.x, 0.0, q.y)
            })
            .collect();
        for k in 0..4 {
            buildings::fence(&mut b, paddock[k], paddock[(k + 1) % 4], height);
        }
    }
}

fn plant(world: &mut World, forest: &Forest) {
    let mut count = 0;
    let mut tree = |world: &mut World, kind: Species, p: Vec2, seed: f32, scale: f32| {
        let at = ground(p) - Vec3::Y * 0.15;
        forest.spawn(world, kind, at, seed * TAU, scale);
        count += 1;
    };

    // Woods.
    let step = 9.0;
    let n = (terrain::DETAIL_EXTENT / step) as i32 - 1;
    for j in -n..=n {
        for i in -n..=n {
            let (fi, fj) = (i as f32, j as f32);
            let jitter = Vec2::new(hash(31, fi, fj), hash(32, fi, fj)) - 0.5;
            let p = (Vec2::new(fi, fj) + jitter * 0.9) * step;
            let density = terrain::forest(p.x, p.y);
            let r = hash(33, fi, fj);
            if density <= 0.0 || (density < 0.03 && r < 0.5) {
                continue;
            }
            let kind = if hash(34, fi, fj) < 0.6 { Species::Oak } else { Species::Ash };
            tree(world, kind, p, r, 0.75 + r * 0.45);
        }
    }

    // Poplars along the road outside the village.
    let bridge = *BRIDGE;
    let mut x = -740.0;
    while x < 740.0 {
        let (p, t) = terrain::road_at(x);
        if (p - bridge.center).length() > 34.0 && !(35.0..235.0).contains(&x) {
            for side in [-1.0, 1.0] {
                let q = p + t.perp() * side * 7.5;
                if terrain::river_distance(q.x, q.y) > 18.0 && !reserved(q) {
                    let r = hash(21, x, side);
                    tree(world, Species::Poplar, q, r, 0.85 + r * 0.3);
                }
            }
        }
        x += 12.0;
    }

    // Trees along the river banks.
    let mut z = -760.0;
    while z < 760.0 {
        for side in [-1.0f32, 1.0] {
            let r = hash(51, z, side);
            if r < 0.4 {
                let p = Vec2::new(terrain::river_x(z) + side * (18.0 + r * 12.0), z);
                if (p - bridge.center).length() > 30.0 && terrain::road_distance(p.x, p.y) > 8.0 && !reserved(p) {
                    tree(world, Species::Ash, p, r, 0.6 + r * 0.5);
                }
            }
        }
        z += 13.0;
    }

    // Hedgerows along some field edges, with the odd oak grown up out of them.
    let columns = (terrain::DETAIL_EXTENT * 1.6 / terrain::FIELD_WIDTH) as i32;
    for column in -columns..=columns {
        if hash(61, column as f32, 0.0) > 0.45 {
            continue;
        }
        let u = column as f32 * terrain::FIELD_WIDTH;
        let mut v = -terrain::DETAIL_EXTENT * 1.5;
        while v < terrain::DETAIL_EXTENT * 1.5 {
            let p = terrain::world_from_field(Vec2::new(u, v));
            let gap = hash(62, column as f32, (v / 14.0).floor()) < 0.12;
            if !gap && p.abs().max_element() < terrain::DETAIL_EXTENT - 10.0 && hedge_allowed(p) {
                let r = hash(63, v, u);
                tree(world, Species::Shrub, p, r, 0.85 + r * 0.4);
                if r < 0.03 {
                    tree(world, Species::Oak, p, r * 30.0, 0.8 + r * 10.0);
                }
            }
            v += 2.4;
        }
    }
    // Lone trees standing in the fields.
    for j in -19..=19 {
        for i in -19..=19 {
            let (fi, fj) = (i as f32, j as f32);
            let r = hash(71, fi, fj);
            let p = Vec2::new(fi + hash(72, fi, fj), fj + hash(73, fi, fj)) * 40.0;
            if r < 0.05 && hedge_allowed(p) {
                tree(world, Species::Oak, p, r * 20.0, 0.9 + r * 4.0);
            }
        }
    }
    log::info!("planted {count} trees and shrubs");
}

fn hedge_allowed(p: Vec2) -> bool {
    terrain::road_distance(p.x, p.y) > 8.0
        && terrain::river_distance(p.x, p.y) > 22.0
        && terrain::forest(p.x, p.y) < -0.03
        && (p - VILLAGE).length() > 140.0
        && !reserved(p)
}

pub struct Spin {
    pub axis: Vec3,
    pub speed: f32,
}
impl Component for Spin {}

fn spawn_sails(world: &mut World) {
    let mut sails = Mesh::default();
    let wood = Mesh::cube(1.0);
    let hub = Mat4::IDENTITY;
    let mut canvas = Mesh::default();
    for i in 0..4 {
        let a = i as f32 * TAU / 4.0 + 0.3;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let side = Vec3::new(-a.sin(), a.cos(), 0.0);
        let rot = Quat::from_rotation_z(a - TAU / 4.0);
        sails.append(&wood, hub * Mat4::from_scale_rotation_translation(Vec3::new(0.28, 10.5, 0.28), rot, dir * 5.25));
        for k in 0..7 {
            let p = dir * (2.4 + k as f32 * 1.3) + side * 0.95;
            sails.append(&wood, Mat4::from_scale_rotation_translation(Vec3::new(1.9, 0.08, 0.1), rot, p));
        }
        for s in [0.05, 1.85] {
            sails.append(&wood, Mat4::from_scale_rotation_translation(Vec3::new(0.08, 8.2, 0.1), rot, dir * 6.3 + side * s));
        }
        canvas.append(&wood, Mat4::from_scale_rotation_translation(Vec3::new(1.75, 7.8, 0.02), rot, dir * 6.3 + side * 0.95 + Vec3::Z * 0.06));
    }
    sails.append(&Mesh::uv_sphere(0.45, 12, 8), Mat4::IDENTITY);
    let yaw = Quat::from_rotation_y(yaw_toward(Vec2::new(-1.0, 0.3)));
    let at = ground(windmill()) + yaw * buildings::SAIL_HUB;
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let (sails, canvas) = (meshes.add(sails), meshes.add(canvas));
    let _ = &mut meshes;
    let spin = Spin {
        axis: Vec3::Z,
        speed: 0.4,
    };
    let root = world.spawn((Transform::from_translation(at).with_rotation(yaw), spin));
    world.spawn((
        Transform::IDENTITY,
        Mesh3d(sails),
        Material {
            color: Color::hex(0x6b5a45),
            roughness: 0.85,
            ..Default::default()
        },
        Parent(root),
    ));
    world.spawn((
        Transform::IDENTITY,
        Mesh3d(canvas),
        Material {
            color: Color::hex(0xcfc3a6),
            roughness: 0.95,
            translucency: 0.35,
            double_sided: true,
            ..Default::default()
        },
        Parent(root),
    ));
}

pub fn spin(time: Res<Time>, mut query: Query<(&mut Transform, &Spin)>) {
    for (mut transform, spin) in &mut query {
        transform.rotation *= Quat::from_axis_angle(spin.axis, spin.speed * time.delta_secs());
    }
}

/// Where the camp's fires burn, relative to the camp.
const FIRES: [Vec2; 4] = [
    Vec2::new(-22.0, -8.0),
    Vec2::new(24.0, 2.0),
    Vec2::new(-20.0, 14.0),
    Vec2::new(25.0, 22.0),
];

pub fn fires() -> impl Iterator<Item = Vec2> {
    FIRES.iter().map(|f| CAMP + *f)
}

/// Rows of tents by the river, and stone-ringed fires with cooking pots.
fn camp(b: &mut Build) {
    use crate::materials::{CANVAS, WOOD};
    let canvas = tint(Vec3::new(1.0, 1.0, 0.97), 0.0);
    for row in 0..4 {
        for k in 0..7 {
            let p = CAMP + Vec2::new(k as f32 * 5.0 - 15.0, row as f32 * 10.0 - 15.0);
            let r = hash(121, row as f32, k as f32);
            b.at(ground(p), yaw_toward(Vec2::new(0.0, -1.0)) + (r - 0.5) * 0.08);
            b.roof(2.6, 2.4, 0.02, 1.9, CANVAS, canvas, CANVAS, canvas);
            // The door flap drawn back, and the two poles.
            b.cuboid(Vec3::new(0.0, 0.75, -1.34), Vec3::new(0.7, 1.5, 0.02), CANVAS, tint(Vec3::splat(0.35), 0.0));
            for x in [-1.32, 1.32] {
                b.cylinder(Vec3::new(x, 1.0, 0.0), 0.03, 2.1, WOOD, NEUTRAL);
            }
        }
    }
    for fire in fires() {
        b.at(ground(fire), 0.0);
        for i in 0..9 {
            let a = i as f32 * 0.7;
            b.cuboid(Vec3::new(a.cos() * 0.8, 0.08, a.sin() * 0.8), Vec3::new(0.25, 0.2, 0.22), STONE, tint(Vec3::splat(0.8), 0.0));
        }
        for i in 0..5 {
            let a = i as f32 * 1.26;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            b.beam(d * 0.55 + Vec3::Y * 0.05, Vec3::Y * 0.3 - d * 0.05, 0.1, 0.1, WOOD, tint(Vec3::splat(0.35), 0.1));
        }
        for i in 0..3 {
            let a = i as f32 * 2.09;
            b.beam(Vec3::new(a.cos() * 0.8, 0.0, a.sin() * 0.8), Vec3::Y * 1.35, 0.04, 0.04, WOOD, NEUTRAL);
        }
        b.cylinder(Vec3::new(0.0, 0.8, 0.0), 0.22, 0.3, STONE, tint(Vec3::splat(0.15), 0.0));
    }
    b.place = Mat4::IDENTITY;
}

pub struct Flame(f32);
impl Component for Flame {}

/// Flames for the campfires: small emissive shapes that flicker.
pub fn light_fires(world: &mut World) {
    let flame = world.resource_mut::<Assets<Mesh>>().add(Mesh::cone(0.28, 0.6, 10));
    for (i, fire) in fires().enumerate() {
        world.spawn((
            Transform::from_translation(ground(fire) + Vec3::Y * 0.35),
            Mesh3d(flame),
            Material::emissive(Color::rgb(28.0, 9.0, 1.6)),
            mira::render::NotShadowCaster,
            Flame(i as f32 * 1.7),
        ));
    }
}

pub fn campfires(time: Res<Time>, mut smoke: ResMut<crate::smoke::Smoke>, mut timer: Local<f32>, mut flames: Query<(&mut Transform, &Flame)>) {
    let now = time.elapsed_secs();
    for (mut transform, flame) in &mut flames {
        let f = (now * 13.0 + flame.0).sin() * 0.5 + (now * 7.3 + flame.0 * 2.0).sin() * 0.5;
        transform.scale = Vec3::new(1.0 + f * 0.1, 1.0 + f * 0.25, 1.0 + f * 0.1);
    }
    *timer -= time.delta_secs();
    if *timer <= 0.0 {
        *timer = 0.35;
        for fire in fires() {
            smoke.woodsmoke(ground(fire) + Vec3::Y * 0.9);
        }
    }
}
