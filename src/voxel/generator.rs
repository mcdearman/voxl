use std::sync::Arc;

use glam::{IVec3, Vec3};

use super::{
    block::BlockId,
    chunk::{ChunkData, CHUNK_SIZE},
};

/// Produces the blocks of a chunk. Runs on worker threads, so it must be pure: the same
/// position always gives the same chunk.
#[derive(Clone)]
pub struct ChunkGenerator(pub Arc<dyn Fn(IVec3) -> ChunkData + Send + Sync>);

impl ChunkGenerator {
    pub fn new(generate: impl Fn(IVec3) -> ChunkData + Send + Sync + 'static) -> Self {
        Self(Arc::new(generate))
    }
}

fn hash(seed: u32, p: IVec3) -> f32 {
    let mut h = seed
        ^ (p.x as u32).wrapping_mul(0x85eb_ca6b)
        ^ (p.y as u32).wrapping_mul(0xc2b2_ae35)
        ^ (p.z as u32).wrapping_mul(0x27d4_eb2f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32
}

/// Smoothly interpolated lattice noise in `0..1`.
pub fn value_noise(seed: u32, p: Vec3) -> f32 {
    let cell = p.floor();
    let t = p - cell;
    let t = t * t * (3.0 - 2.0 * t);
    let cell = cell.as_ivec3();
    let corner = |x, y, z| hash(seed, cell + IVec3::new(x, y, z));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let plane = |z| {
        lerp(
            lerp(corner(0, 0, z), corner(1, 0, z), t.x),
            lerp(corner(0, 1, z), corner(1, 1, z), t.x),
            t.y,
        )
    };
    lerp(plane(0), plane(1), t.z)
}

/// Several octaves of `value_noise`, still in `0..1`.
pub fn fbm(seed: u32, p: Vec3, octaves: u32) -> f32 {
    let (mut sum, mut amplitude, mut frequency, mut total) = (0.0, 1.0, 1.0, 0.0);
    for octave in 0..octaves {
        sum += value_noise(seed.wrapping_add(octave), p * frequency) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / total
}

/// Rolling hills with beaches in the low spots and winding caves underneath.
#[derive(Clone, Copy, Debug)]
pub struct Terrain {
    pub seed: u32,
    pub grass: BlockId,
    pub dirt: BlockId,
    pub stone: BlockId,
    pub sand: BlockId,
}

impl Terrain {
    pub const SAND_LEVEL: i32 = 18;

    pub fn height(&self, x: i32, z: i32) -> i32 {
        let p = Vec3::new(x as f32, 0.0, z as f32);
        let hills = fbm(self.seed, p / 96.0, 4);
        // A slow mask so some regions are flat and others mountainous.
        let roughness = value_noise(self.seed ^ 0x9e37, p / 320.0);
        (10.0 + hills * (18.0 + roughness * 60.0)) as i32
    }

    fn is_cave(&self, p: Vec3) -> bool {
        // Two noise fields are both near their midpoint only along thin curves: tunnels.
        let a = fbm(self.seed ^ 0xa511, p / 28.0, 2);
        let b = fbm(self.seed ^ 0x51ed, p / 28.0, 2);
        (a - 0.5).abs() < 0.035 && (b - 0.5).abs() < 0.035
    }

    pub fn generate(&self, chunk: IVec3) -> ChunkData {
        let origin = chunk * CHUNK_SIZE;
        let mut data = ChunkData::default();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let height = self.height(origin.x + x, origin.z + z);
                for y in 0..CHUNK_SIZE.min(height - origin.y + 1) {
                    let world = origin + IVec3::new(x, y, z);
                    let depth = height - world.y;
                    // Leave the bottom layer intact so nothing falls out of the world.
                    if world.y > 0 && self.is_cave(world.as_vec3()) {
                        continue;
                    }
                    let block = match depth {
                        0..=2 if height <= Self::SAND_LEVEL => self.sand,
                        0 => self.grass,
                        1..=3 => self.dirt,
                        _ => self.stone,
                    };
                    data.set(IVec3::new(x, y, z), block);
                }
            }
        }
        data.compact();
        data
    }
}
