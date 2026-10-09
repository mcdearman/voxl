//! The land: rolling hills cut by a river, a road across it, and a patchwork of fields.
//!
//! Everything is a pure function of position, so the mesh, the placement of scenery and the
//! walking camera all agree on where the ground is. One unit is one metre; +X is east.

use std::sync::LazyLock;

use crate::world::{self, WorldMesh, WorldVertex};

use mira::{
    glam::{Vec2, Vec3},

    voxel::{fbm, value_noise},
};

pub const WATER_LEVEL: f32 = 0.0;
/// Half the width of the finely meshed square the scene takes place in.
pub const DETAIL_EXTENT: f32 = 768.0;
pub const DETAIL_STEP: f32 = 2.0;
/// Coarse terrain continues out to here, so the horizon is land fading into haze.
const OUTER_EXTENT: f32 = 4096.0;
const OUTER_STEP: f32 = 32.0;
pub const ROAD_HALF_WIDTH: f32 = 3.0;

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A stable pseudo-random number in `0..1` for a pair of integers.
pub fn hash(seed: u32, a: f32, b: f32) -> f32 {
    // Value noise returns its lattice hash exactly at integer coordinates.
    value_noise(seed, Vec3::new(a.floor(), b.floor(), 0.0))
}

/// The river runs roughly north-south, meandering.
pub fn river_x(z: f32) -> f32 {
    -40.0 + 70.0 * (z / 260.0).sin() + 30.0 * (z / 97.0 + 1.0).sin()
}

fn river_slope(z: f32) -> f32 {
    70.0 / 260.0 * (z / 260.0).cos() + 30.0 / 97.0 * (z / 97.0 + 1.0).cos()
}

pub fn river_distance(x: f32, z: f32) -> f32 {
    (x - river_x(z)).abs() / (1.0 + river_slope(z).powi(2)).sqrt()
}

/// The road runs roughly east-west, through the village and over the bridge.
pub fn road_z(x: f32) -> f32 {
    30.0 + 25.0 * (x / 210.0).sin()
}

pub fn road_slope(x: f32) -> f32 {
    25.0 / 210.0 * (x / 210.0).cos()
}

pub fn road_distance(x: f32, z: f32) -> f32 {
    (z - road_z(x)).abs() / (1.0 + road_slope(x).powi(2)).sqrt()
}

/// A point on the road and the direction it runs (eastward) there.
pub fn road_at(x: f32) -> (Vec2, Vec2) {
    (
        Vec2::new(x, road_z(x)),
        Vec2::new(1.0, road_slope(x)).normalize(),
    )
}

pub fn height(x: f32, z: f32) -> f32 {
    let p = Vec3::new(x, 0.0, z);
    let hills = 4.0 + fbm(3, p / 420.0, 4) * 45.0 + fbm(17, p / 90.0, 3) * 4.0;
    let d = river_distance(x, z);
    let floodplain = 1.3 + d * 0.02;
    let land = lerp(floodplain, hills, smoothstep(40.0, 260.0, d));
    lerp(-2.4, land, smoothstep(9.0, 16.0, d))
}

pub fn normal(x: f32, z: f32) -> Vec3 {
    let e = 1.0;
    Vec3::new(
        height(x - e, z) - height(x + e, z),
        2.0 * e,
        height(x, z - e) - height(x, z + e),
    )
    .normalize()
}

/// Where the fighting-free part of the story happens: no woods here.
pub fn in_stage(x: f32, z: f32) -> bool {
    (-60.0..720.0).contains(&x) && (-290.0..270.0).contains(&z)
}

/// Above zero where woodland grows.
pub fn forest(x: f32, z: f32) -> f32 {
    if in_stage(x, z) || river_distance(x, z) < 45.0 || road_distance(x, z) < 22.0 {
        return 0.0;
    }
    fbm(29, Vec3::new(x, 0.0, z) / 330.0, 3) - 0.58
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Crop {
    Wheat,
    Barley,
    Oats,
    Meadow,
    Pasture,
    Ploughed,
    Stubble,
}

pub const FIELD_ANGLE: f32 = 0.35;
pub const FIELD_WIDTH: f32 = 85.0;
pub const FIELD_LENGTH: f32 = 70.0;

/// Field coordinates: `u` across the strips, `v` along them.
pub fn field_space(x: f32, z: f32) -> Vec2 {
    let (sin, cos) = FIELD_ANGLE.sin_cos();
    Vec2::new(x * cos + z * sin, -x * sin + z * cos)
}

pub fn world_from_field(f: Vec2) -> Vec2 {
    let (sin, cos) = FIELD_ANGLE.sin_cos();
    Vec2::new(f.x * cos - f.y * sin, f.x * sin + f.y * cos)
}

/// Fields are strips `FIELD_WIDTH` wide, cut into lengths with a different offset per strip.
pub fn field_cell(x: f32, z: f32) -> (f32, f32) {
    let f = field_space(x, z);
    let column = (f.x / FIELD_WIDTH).floor();
    let offset = hash(5, column, 0.0) * FIELD_LENGTH;
    (column, ((f.y + offset) / FIELD_LENGTH).floor())
}

pub fn crop(x: f32, z: f32) -> Crop {
    let (column, row) = field_cell(x, z);
    match hash(8, column, row) {
        r if r < 0.28 => Crop::Wheat,
        r if r < 0.40 => Crop::Barley,
        r if r < 0.50 => Crop::Oats,
        r if r < 0.70 => Crop::Meadow,
        r if r < 0.82 => Crop::Pasture,
        r if r < 0.91 => Crop::Ploughed,
        _ => Crop::Stubble,
    }
}

/// The stone bridge carrying the road over the river.
#[derive(Clone, Copy, Debug)]
pub struct Bridge {
    pub center: Vec2,
    pub direction: Vec2,
    pub half_length: f32,
    pub half_width: f32,
    pub deck: f32,
}

impl Bridge {
    pub fn locate() -> Self {
        // Bisect for where the road meets the river.
        let (mut lo, mut hi) = (-200.0f32, 200.0f32);
        let side = |x: f32| x - river_x(road_z(x));
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if side(lo).signum() == side(mid).signum() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (center, direction) = road_at(lo);
        Self {
            center,
            direction,
            half_length: 22.0,
            half_width: 3.4,
            deck: 2.6,
        }
    }

    /// The deck height if `(x, z)` is on the bridge.
    pub fn deck_at(&self, x: f32, z: f32) -> Option<f32> {
        let d = Vec2::new(x, z) - self.center;
        let along = d.dot(self.direction).abs();
        let across = d.perp_dot(self.direction).abs();
        (along <= self.half_length && across <= self.half_width).then_some(self.deck)
    }
}

pub static BRIDGE: LazyLock<Bridge> = LazyLock::new(Bridge::locate);

/// The surface people and horses stand on: the ground, the bridge, or the shallows.
pub fn walk_height(x: f32, z: f32) -> f32 {
    let ground = height(x, z).max(WATER_LEVEL - 0.7);
    BRIDGE.deck_at(x, z).map_or(ground, |deck| deck.max(ground))
}


/// The highest point within `radius` of `center`, for putting things on hilltops.
pub fn high_point(center: Vec2, radius: f32) -> Vec2 {
    let mut best = (center, f32::MIN);
    let steps = 16;
    for j in -steps..=steps {
        for i in -steps..=steps {
            let p = center + Vec2::new(i as f32, j as f32) * radius / steps as f32;
            if (p - center).length() <= radius && height(p.x, p.y) > best.1 {
                best = (p, height(p.x, p.y));
            }
        }
    }
    best.0
}

// ---- Ground materials and meshes -------------------------------------------------------------

/// The seventh blend channel: farmland, where the pixel shader works out which crop grows.
const FARMLAND: u32 = 6;

/// Blends the ground layers toward one layer.
fn toward(w: &mut [f32; 7], layer: u32, t: f32) {
    for (i, v) in w.iter_mut().enumerate() {
        *v = *v * (1.0 - t) + if i as u32 == layer { t } else { 0.0 };
    }
}

/// How much of each ground layer shows at a point, and a colour multiplier. Fields are left
/// to the shader (`field_at` in world.wgsl), which draws their edges crisply; this covers
/// everything that isn't farmland.
pub fn ground(x: f32, z: f32, h: f32, n: Vec3) -> ([f32; 7], Vec3) {
    use crate::materials::{FOREST_FLOOR, GRASS, MUD, ROAD, SOIL};
    let mut w = [0.0; 7];
    w[FARMLAND as usize] = 1.0;
    let mut tint = Vec3::ONE;

    // Grazing by the river and around the village.
    let river = river_distance(x, z);
    let village = (Vec2::new(x, z) - crate::scene::VILLAGE).length();
    let lush = smoothstep(90.0, 50.0, river).max(smoothstep(150.0, 110.0, village));
    toward(&mut w, GRASS, lush);
    tint = tint.lerp(Vec3::new(0.92, 1.02, 0.86), lush);

    toward(&mut w, FOREST_FLOOR, smoothstep(-0.02, 0.03, forest(x, z)));
    toward(&mut w, SOIL, smoothstep(0.9, 0.78, n.y));
    toward(&mut w, MUD, smoothstep(18.0, 12.0, river));
    toward(&mut w, ROAD, smoothstep(ROAD_HALF_WIDTH + 1.5, ROAD_HALF_WIDTH - 0.5, road_distance(x, z)));
    if h < WATER_LEVEL + 0.2 {
        toward(&mut w, MUD, 1.0);
    }
    // The grass scan is a little yellow for July; freshen it everywhere.
    tint *= Vec3::new(0.94, 1.0, 0.86);
    let total: f32 = w.iter().sum();
    (w.map(|v| v / total.max(1e-4)), tint)
}

fn ground_vertex(x: f32, z: f32) -> WorldVertex {
    let h = height(x, z);
    let n = normal(x, z);
    let (w, tint) = ground(x, z, h, n);
    let mut weights = [0; 8];
    for (dst, v) in weights.iter_mut().zip(w) {
        *dst = (v * 255.0).round() as u8;
    }
    WorldVertex {
        position: [x, h, z],
        normal: n.into(),
        uv: [x, z],
        weights,
        tint: world::tint(tint, 0.0),
        layer: world::GROUND,
    }
}

/// A square grid of terrain, rows computed on all cores. `hole` leaves out cells inside that
/// half-width, where the detailed grid is.
fn grid(extent: f32, step: f32, hole: Option<f32>) -> WorldMesh {
    let n = (extent * 2.0 / step).round() as u32;
    let row = n + 1;
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get()) as u32;
    let rows_per_job = row.div_ceil(threads);
    let vertices: Vec<WorldVertex> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    let rows = (t * rows_per_job)..((t + 1) * rows_per_job).min(row);
                    rows.flat_map(|j| {
                        (0..row).map(move |i| {
                            ground_vertex(-extent + i as f32 * step, -extent + j as f32 * step)
                        })
                    })
                    .collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    });
    let mut indices = Vec::with_capacity((n * n * 6) as usize);
    for j in 0..n {
        for i in 0..n {
            if let Some(hole) = hole {
                let (x0, z0) = (-extent + i as f32 * step, -extent + j as f32 * step);
                let inside = |v: f32| v >= -hole && v + step <= hole;
                if inside(x0) && inside(z0) {
                    continue;
                }
            }
            let a = j * row + i;
            let (b, c, d) = (a + 1, a + row, a + row + 1);
            indices.extend([a, c, b, b, c, d]);
        }
    }
    WorldMesh { vertices, indices }
}

/// The detailed terrain around the scene, a coarse ring reaching to the horizon, and the
/// heights of the detailed grid for things that grow on it.
pub struct Ground {
    pub detail: WorldMesh,
    pub outer: WorldMesh,
    /// `samples × samples` heights, `DETAIL_STEP` apart, starting at `-DETAIL_EXTENT`.
    pub heights: Vec<f32>,
    pub samples: u32,
}

pub fn build_ground() -> Ground {
    let detail = grid(DETAIL_EXTENT, DETAIL_STEP, None);
    let samples = (DETAIL_EXTENT * 2.0 / DETAIL_STEP).round() as u32 + 1;
    let heights = detail.vertices.iter().map(|v| v.position[1]).collect();
    Ground {
        outer: grid(OUTER_EXTENT, OUTER_STEP, Some(DETAIL_EXTENT)),
        detail,
        heights,
        samples,
    }
}

/// The river's surface, under all the land; it only shows where the ground dips below it.
pub fn water() -> WorldMesh {
    let e = DETAIL_EXTENT;
    let corner = |x: f32, z: f32| WorldVertex {
        position: [x, WATER_LEVEL, z],
        normal: [0.0, 1.0, 0.0],
        uv: [x, z],
        weights: [0; 8],
        tint: world::NEUTRAL,
        layer: 0,
    };
    WorldMesh {
        vertices: vec![corner(-e, -e), corner(e, -e), corner(-e, e), corner(e, e)],
        indices: vec![0, 2, 1, 1, 2, 3],
    }
}
