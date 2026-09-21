use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};

use super::{
    block::{BlockId, BlockRegistry},
    chunk::{chunk_of, local_of, ChunkData, CHUNK_SIZE},
};

/// All loaded chunks. Block data lives here rather than in entities: a single chunk holds
/// 32,768 blocks, and they are only ever accessed by position.
#[derive(Default)]
pub struct VoxelWorld {
    chunks: HashMap<IVec3, ChunkData>,
    /// Chunks whose mesh is out of date.
    pub(crate) dirty: HashSet<IVec3>,
}

/// What a ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaycastHit {
    /// The solid block that was hit.
    pub block: IVec3,
    /// The face that was hit, pointing out of the block. Zero if the ray started inside it.
    pub normal: IVec3,
    pub distance: f32,
}

impl RaycastHit {
    /// The empty cell in front of the hit face, which is where a placed block goes.
    pub fn adjacent(&self) -> IVec3 {
        self.block + self.normal
    }
}

impl VoxelWorld {
    pub fn chunk(&self, chunk: IVec3) -> Option<&ChunkData> {
        self.chunks.get(&chunk)
    }

    pub fn contains_chunk(&self, chunk: IVec3) -> bool {
        self.chunks.contains_key(&chunk)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunk_positions(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.chunks.keys().copied()
    }

    /// Adds or replaces a chunk, and schedules it and its neighbours for meshing. Neighbours
    /// are included because their border faces depend on this chunk.
    pub fn insert_chunk(&mut self, chunk: IVec3, data: ChunkData) {
        self.chunks.insert(chunk, data);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let neighbour = chunk + IVec3::new(dx, dy, dz);
                    if self.chunks.contains_key(&neighbour) {
                        self.dirty.insert(neighbour);
                    }
                }
            }
        }
    }

    pub fn remove_chunk(&mut self, chunk: IVec3) -> Option<ChunkData> {
        self.dirty.remove(&chunk);
        self.chunks.remove(&chunk)
    }

    /// `None` if the chunk isn't loaded.
    pub fn get_block(&self, block: IVec3) -> Option<BlockId> {
        Some(self.chunks.get(&chunk_of(block))?.get(local_of(block)))
    }

    /// Returns false if the chunk isn't loaded.
    pub fn set_block(&mut self, block: IVec3, id: BlockId) -> bool {
        let chunk = chunk_of(block);
        let Some(data) = self.chunks.get_mut(&chunk) else {
            return false;
        };
        if data.set(local_of(block), id) == id {
            return true;
        }
        // Remesh every chunk whose mesh can see this block. Ambient occlusion reads diagonal
        // neighbours, so a block on a chunk edge or corner affects up to eight chunks.
        let local = local_of(block);
        let reach = |v: i32| match v {
            0 => -1..=0,
            v if v == CHUNK_SIZE - 1 => 0..=1,
            _ => 0..=0,
        };
        for dz in reach(local.z) {
            for dy in reach(local.y) {
                for dx in reach(local.x) {
                    let neighbour = chunk + IVec3::new(dx, dy, dz);
                    if self.chunks.contains_key(&neighbour) {
                        self.dirty.insert(neighbour);
                    }
                }
            }
        }
        true
    }

    /// Whether the block at `block` stops movement. Unloaded space counts as solid.
    pub fn is_solid(&self, registry: &BlockRegistry, block: IVec3) -> bool {
        self.get_block(block).is_none_or(|id| registry.is_solid(id))
    }

    /// Finds the first solid block along a ray by stepping from cell to cell
    /// (Amanatides & Woo), so nothing is skipped no matter how thin.
    pub fn raycast(
        &self,
        registry: &BlockRegistry,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<RaycastHit> {
        let direction = direction.try_normalize()?;
        let mut block = origin.floor().as_ivec3();
        let step = direction.signum().as_ivec3();
        // Distance along the ray between grid lines, and to the first one, per axis.
        let delta = (1.0 / direction).abs();
        let mut next = Vec3::ZERO;
        for axis in 0..3 {
            let edge = if direction[axis] > 0.0 {
                block[axis] as f32 + 1.0
            } else {
                block[axis] as f32
            };
            next[axis] = if direction[axis] == 0.0 {
                f32::INFINITY
            } else {
                (edge - origin[axis]) / direction[axis]
            };
        }

        let mut normal = IVec3::ZERO;
        let mut distance = 0.0;
        while distance <= max_distance {
            match self.get_block(block) {
                None => return None, // left the loaded world
                Some(id) if registry.is_solid(id) => {
                    return Some(RaycastHit {
                        block,
                        normal,
                        distance,
                    });
                }
                Some(_) => {}
            }
            let axis = if next.x <= next.y && next.x <= next.z {
                0
            } else if next.y <= next.z {
                1
            } else {
                2
            };
            distance = next[axis];
            next[axis] += delta[axis];
            block[axis] += step[axis];
            normal = IVec3::ZERO;
            normal[axis] = -step[axis];
        }
        None
    }
}
