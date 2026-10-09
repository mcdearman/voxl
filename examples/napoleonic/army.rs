//! The soldiers: Rocketbox figures dressed and posed by `tools/soldiers.py`, drilling and
//! marching.

use std::f32::consts::PI;

use mira::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::{GltfScene, Image},
};

use crate::{
    materials::asset,
    smoke::Smoke,
    scene::{ground, knoll, yaw_toward, BATTERY, CAVALRY, EAST, LINE},
    terrain,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Fusilier,
    Grenadier,
    Gunner,
    Officer,
    Cuirassier,
    Emperor,
    Staff,
    Driver,
    Team,
}

impl Kind {
    const ALL: [Kind; 9] = [
        Kind::Fusilier,
        Kind::Grenadier,
        Kind::Gunner,
        Kind::Officer,
        Kind::Cuirassier,
        Kind::Emperor,
        Kind::Staff,
        Kind::Driver,
        Kind::Team,
    ];

    fn file(self) -> &'static str {
        match self {
            Kind::Fusilier => "fusilier",
            Kind::Grenadier => "grenadier",
            Kind::Gunner => "gunner",
            Kind::Officer => "officer",
            Kind::Cuirassier => "cuirassier",
            Kind::Emperor => "emperor",
            Kind::Staff => "staff",
            Kind::Driver => "driver",
            Kind::Team => "team",
        }
    }
}

/// What a part of a figure is, for dressing each soldier differently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Look {
    Plain,
    /// The head, whose texture changes from man to man.
    Head,
    /// Eyebrows and lashes, tinted to match the hair.
    Hair,
}

type Part = (Handle<Mesh>, Material, Look);

/// One troop type's poses, each a list of parts (one per material).
pub struct Figure {
    poses: Vec<(String, Vec<Part>)>,
}

impl Figure {
    fn load(kind: Kind, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> anyhow::Result<Self> {
        let scene = GltfScene::load(asset(&format!("models/army/{}.glb", kind.file())), meshes, images)?;
        let mut poses: Vec<(String, Vec<Part>)> = Vec::new();
        for part in scene.parts {
            let look = match part.material_name.as_str() {
                "m014_head" => Look::Head,
                "m014_opacity" => Look::Hair,
                _ => Look::Plain,
            };
            // The figures were exported facing +Z; turn them to face -Z like everything else.
            let mut mesh = Mesh::default();
            if let Some(m) = meshes.get(part.mesh) {
                mesh.append(m, Mat4::from_rotation_y(PI) * part.transform);
            }
            let handle = meshes.add(mesh);
            let entry = (handle, part.material, look);
            match poses.iter_mut().find(|(n, _)| *n == part.name) {
                Some((_, parts)) => parts.push(entry),
                None => poses.push((part.name.clone(), vec![entry])),
            }
        }
        anyhow::ensure!(!poses.is_empty(), "{} has no poses", kind.file());
        Ok(Self { poses })
    }

    fn index(&self, pose: &str) -> usize {
        self.poses.iter().position(|(n, _)| n == pose).unwrap_or(0)
    }

    fn parts(&self, pose: usize) -> &[Part] {
        &self.poses[pose.min(self.poses.len() - 1)].1
    }

    fn max_parts(&self) -> usize {
        self.poses.iter().map(|(_, p)| p.len()).max().unwrap_or(0)
    }
}

pub struct Figures {
    figures: Vec<(Kind, Figure)>,
    /// Faces to choose from: each a head texture and the colour of the hair that goes with it.
    faces: Vec<(Handle<Image>, Color)>,
    /// Stands in for parts a pose doesn't use.
    nothing: Handle<Mesh>,
}

impl Figures {
    pub fn load(meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> anyhow::Result<Self> {
        let mut figures = Vec::new();
        for kind in Kind::ALL {
            figures.push((kind, Figure::load(kind, meshes, images)?));
        }
        let hair = std::fs::read_to_string(asset("models/army/heads/hair.txt"))?;
        let mut faces = Vec::new();
        for (i, line) in hair.lines().enumerate() {
            let bytes = std::fs::read(asset(&format!("models/army/heads/head_{i}.jpg")))?;
            let image = images.add(Image::from_bytes(&bytes, true)?);
            let rgb = u32::from_str_radix(line.trim(), 16)?;
            faces.push((image, Color::hex(rgb)));
        }
        Ok(Self {
            figures,
            faces,
            nothing: meshes.add(Mesh::default()),
        })
    }

    fn get(&self, kind: Kind) -> &Figure {
        &self.figures.iter().find(|(k, _)| *k == kind).unwrap().1
    }

    /// The mesh and material a part is drawn with on one particular man.
    fn dress(&self, part: Option<&Part>, face: usize) -> (Handle<Mesh>, Material) {
        let Some(&(mesh, mut material, look)) = part else {
            return (self.nothing, Material::default());
        };
        if let Some((head, hair)) = self.faces.get(face % self.faces.len().max(1)) {
            match look {
                Look::Head => material.base_color_texture = Some(*head),
                // The lash and brow cards are dark already; pull them toward the hair colour.
                Look::Hair => material.color = Color::rgb(hair.r * 2.0, hair.g * 2.0, hair.b * 2.0),
                Look::Plain => {}
            }
        }
        (mesh, material)
    }
}

/// A figure and the pose it's in. Its parts are child entities.
pub struct Soldier {
    kind: Kind,
    pose: usize,
    shown: usize,
    /// Which face he has.
    face: usize,
    parts: Vec<Entity>,
}
impl Component for Soldier {}

/// Marches along the road in the column.
pub struct Marcher {
    behind: f32,
    across: f32,
}
impl Component for Marcher {}

/// Stands in the battalion's front ranks, presenting and firing with the rest.
pub struct Musketeer;
impl Component for Musketeer {}

pub struct Column {
    head: f32,
}

/// Where the column's route starts and ends, as x along the road.
const ROUTE: (f32, f32) = (-300.0, 700.0);
/// Quick march: a hundred paces a minute.
const MARCH_SPEED: f32 = 1.1;
const STRIDE_SECONDS: f32 = 1.2;

fn spawn(world: &mut World, figures: &Figures, kind: Kind, pose: &str, transform: Transform) -> Entity {
    let figure = figures.get(kind);
    let pose = figure.index(pose);
    // Where a man stands decides which face he has.
    let p = transform.translation;
    let face = (terrain::hash(141, (p.x * 7.3).floor(), (p.z * 5.1).floor()) * 1000.0) as usize;
    let root = world.spawn(transform);
    let parts = (0..figure.max_parts())
        .map(|i| {
            let (mesh, material) = figures.dress(figure.parts(pose).get(i), face);
            world.spawn((Transform::IDENTITY, Mesh3d(mesh), material, Parent(root)))
        })
        .collect();
    world.insert(
        root,
        Soldier {
            kind,
            pose,
            shown: pose,
            face,
            parts,
        },
    );
    root
}

fn facing(p: Vec2, direction: Vec2) -> Transform {
    Transform::from_translation(ground(p)).with_rotation(Quat::from_rotation_y(yaw_toward(direction)))
}

/// Spawns the battalion in line, the column on the road, and the battery's gunners.
pub fn setup(world: &mut World) {
    let figures = world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
        Figures::load(meshes, world.resource_mut::<Assets<Image>>())
    });
    let figures = match figures {
        Ok(f) => f,
        Err(err) => {
            log::error!("couldn't load the soldiers: {err:#}");
            return;
        }
    };

    // The battalion: three ranks, facing east toward the butts. Grenadiers on the right.
    let files = 48;
    let across = EAST.perp();
    for rank in 0..3 {
        for file in 0..files {
            let p = LINE - EAST * rank as f32 * 0.7 + across * (file as f32 - files as f32 * 0.5) * 0.56;
            let jitter = Vec2::new(terrain::hash(3, rank as f32, file as f32) - 0.5, terrain::hash(4, rank as f32, file as f32) - 0.5) * 0.06;
            let kind = if file >= files - 8 { Kind::Grenadier } else { Kind::Fusilier };
            let seed = terrain::hash(5, rank as f32, file as f32);
            let soldier = spawn(world, &figures, kind, "order", varied(facing(p + jitter, EAST), seed));
            if rank < 2 {
                world.insert(soldier, Musketeer);
            }
        }
    }
    for k in [-18.0, 0.0, 18.0] {
        spawn(world, &figures, Kind::Officer, "attention", facing(LINE + EAST * 3.0 + across * k, -EAST));
    }

    // The column, four abreast, marching east along the road.
    spawn_marcher(world, &figures, Kind::Officer, "attention", 0.0, 0.0);
    for rank in 0..30 {
        for file in 0..4 {
            spawn_marcher(world, &figures, Kind::Fusilier, "march0", 3.0 + rank as f32 * 1.3, (file as f32 - 1.5) * 0.75);
        }
    }
    world.insert_resource(Column { head: 60.0 });

    // The chief of each piece stands back behind his gun; the crew who work it are animated
    // (see artillery.rs).
    for i in 0..6 {
        let home = BATTERY + EAST.perp() * -(i as f32) * 15.0;
        spawn(world, &figures, Kind::Gunner, "attention", facing(home + Vec2::new(-4.2, 0.3), EAST));
    }

    // Men of the camp standing round their fires.
    for (i, fire) in crate::scene::fires().enumerate() {
        for k in 0..5 {
            let a = k as f32 * 1.3 + i as f32;
            let p = fire + Vec2::from_angle(a) * (1.9 + terrain::hash(131, i as f32, k as f32) * 0.5);
            let kind = if k % 3 == 0 { Kind::Grenadier } else { Kind::Fusilier };
            let t = varied(facing(p, fire - p), terrain::hash(132, i as f32, k as f32));
            spawn(world, &figures, kind, "order", t);
        }
    }

    // Cuirassiers in reserve, two ranks knee to knee, on horses of every colour.
    let coats = ["mounted_bay", "mounted_black", "mounted_chestnut"];
    for rank in 0..2 {
        for file in 0..16 {
            let p = CAVALRY - EAST * rank as f32 * 3.6 + EAST.perp() * (file as f32 - 8.0) * 1.2;
            let r = terrain::hash(101, rank as f32, file as f32);
            let t = varied(facing(p, EAST), r) .with_scale(Vec3::splat(0.9));
            spawn(world, &figures, Kind::Cuirassier, coats[(r * 3.0) as usize % 3], t);
        }
    }

    // The Emperor and his staff on the knoll, watching the practice.
    let hill = knoll();
    log::info!("the Emperor watches from {hill}");
    let view = Vec2::new(1.0, 0.25).normalize();
    spawn(world, &figures, Kind::Emperor, "mounted_grey", facing(hill, view).with_scale(Vec3::splat(0.88)));
    for (i, offset) in [Vec2::new(-3.5, -3.0), Vec2::new(-4.0, 2.8), Vec2::new(-7.0, -0.5), Vec2::new(-6.5, 4.8)].into_iter().enumerate() {
        let t = varied(facing(hill + offset, view), i as f32 * 0.31).with_scale(Vec3::splat(0.9));
        spawn(world, &figures, Kind::Staff, coats[i % 3].replace("black", "bay").as_str(), t);
    }

    // Each gun's team waits behind it, facing the rear, ready to limber up.
    for i in 0..6 {
        let home = BATTERY + EAST.perp() * -(i as f32) * 15.0;
        for pair in 0..2 {
            for (side, driven) in [(-0.7f32, true), (0.7, false)] {
                let p = home - EAST * (23.5 + pair as f32 * 3.0) + EAST.perp() * side;
                let r = terrain::hash(111, i as f32, (pair * 2) as f32 + side);
                let t = facing(p, -EAST).with_scale(Vec3::splat(0.92));
                if driven {
                    spawn(world, &figures, Kind::Driver, if r < 0.5 { "mounted_bay" } else { "mounted_chestnut" }, t);
                } else {
                    let coat = ["team_bay", "team_chestnut", "team_black"][(r * 3.0) as usize % 3];
                    spawn(world, &figures, Kind::Team, coat, t);
                }
            }
        }
    }
    world.insert_resource(figures);
}

fn spawn_marcher(world: &mut World, figures: &Figures, kind: Kind, pose: &str, behind: f32, across: f32) {
    let soldier = spawn(world, figures, kind, pose, Transform::IDENTITY);
    world.insert(soldier, Marcher { behind, across });
}

/// Shows each soldier's current pose.
pub fn show_poses(figures: Option<Res<Figures>>, mut soldiers: Query<&mut Soldier>, mut parts: Query<(&mut Mesh3d, &mut Material)>) {
    let Some(figures) = figures else {
        return;
    };
    for mut soldier in &mut soldiers {
        if soldier.shown == soldier.pose {
            continue;
        }
        let figure = figures.get(soldier.kind);
        let pose = figure.parts(soldier.pose);
        for (i, part) in soldier.parts.iter().enumerate() {
            if let Some((mut mesh, mut material)) = parts.get_mut(*part) {
                let (m, mat) = figures.dress(pose.get(i), soldier.face);
                mesh.0 = m;
                *material = mat;
            }
        }
        soldier.shown = soldier.pose;
    }
}

pub fn march(
    time: Res<Time>,
    figures: Option<Res<Figures>>,
    column: Option<ResMut<Column>>,
    mut marchers: Query<(&mut Transform, &mut Soldier, &Marcher)>,
) {
    let (Some(figures), Some(mut column)) = (figures, column) else {
        return;
    };
    column.head += MARCH_SPEED * time.delta_secs();
    if column.head - 45.0 > ROUTE.1 {
        column.head = ROUTE.0;
    }
    // Everyone in step.
    let cycle = (time.elapsed_secs() / STRIDE_SECONDS).fract();
    let frame = ((cycle * 8.0) as usize) % 8;
    let march_pose = figures.get(Kind::Fusilier).index(&format!("march{frame}"));
    for (mut transform, mut soldier, marcher) in &mut marchers {
        let (p, t) = terrain::road_at(column.head - marcher.behind);
        let at = p + t.perp() * marcher.across;
        transform.translation = ground(at);
        transform.rotation = Quat::from_rotation_y(yaw_toward(t));
        if soldier.kind == Kind::Fusilier {
            soldier.pose = march_pose;
        }
    }
}

/// The battalion's volley drill, as a phase in seconds through a 26 s cycle.
pub const DRILL_PERIOD: f32 = 26.0;
pub const PRESENT: f32 = 17.0;
pub const FIRE: f32 = 20.0;
pub const RECOVER: f32 = 22.5;

pub fn drill_phase(time: &Time) -> f32 {
    (time.elapsed_secs() + 8.0) % DRILL_PERIOD
}

/// The front ranks present at the word, fire together, and recover.
pub fn drill(
    time: Res<Time>,
    figures: Option<Res<Figures>>,
    mut smoke: ResMut<Smoke>,
    mut last: Local<f32>,
    mut soldiers: Query<(&Transform, &mut Soldier), With<Musketeer>>,
) {
    let Some(figures) = figures else {
        return;
    };
    let t = drill_phase(&time);
    let fired = *last < FIRE && t >= FIRE;
    *last = t;
    let presenting = (PRESENT..RECOVER).contains(&t);
    let mut n = 0;
    for (transform, mut soldier) in &mut soldiers {
        let figure = figures.get(soldier.kind);
        let wanted = figure.index(if presenting { "present" } else { "order" });
        if soldier.pose != wanted {
            soldier.pose = wanted;
        }
        n += 1;
        // Every other man's smoke is enough to fill the front.
        if fired && n % 2 == 0 {
            let forward = transform.forward();
            let right = transform.right();
            let muzzle = transform.translation + forward * 1.55 + right * 0.12 + Vec3::Y * 1.42;
            smoke.musket(muzzle, forward);
        }
    }
}

/// No two men the same height or standing quite square.
fn varied(t: Transform, seed: f32) -> Transform {
    let height = 0.93 + 0.1 * seed;
    let turn = (terrain::hash(9, seed * 1000.0, 1.0) - 0.5) * 0.12;
    Transform {
        rotation: t.rotation * Quat::from_rotation_y(turn),
        scale: Vec3::splat(height),
        ..t
    }
}
