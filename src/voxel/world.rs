use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};

use super::{
    block::{BlockId, BlockRegistry},
    chunk::{chunk_of, local_of, ChunkData, Runs, CHUNK_SIZE},
};

/// All loaded chunks. Block data lives here rather than in entities: a single chunk holds
/// 32,768 blocks, and they are only ever accessed by position.
///
/// It also remembers which chunks have been edited, and keeps the edited ones that aren't
/// loaded, because the generator can't make those again.
#[derive(Default)]
pub struct VoxelWorld {
    chunks: HashMap<IVec3, ChunkData>,
    /// Chunks whose mesh is out of date.
    pub(crate) dirty: HashSet<IVec3>,
    /// Loaded chunks that no longer match what the generator makes.
    edited: HashSet<IVec3>,
    /// Edited chunks that aren't loaded, waiting for a viewer to come back. Never holds a
    /// chunk that is also in `chunks`.
    unloaded_edits: HashMap<IVec3, Runs>,
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
    ///
    /// The data is taken to be what the generator makes there, so edits made to the chunk
    /// before are forgotten. Chunks arriving from the generator go through
    /// [`insert_generated`](Self::insert_generated) instead, which keeps them.
    pub fn insert_chunk(&mut self, chunk: IVec3, data: ChunkData) {
        self.edited.remove(&chunk);
        self.unloaded_edits.remove(&chunk);
        self.place_chunk(chunk, data);
    }

    /// Adds a chunk the generator made, unless the chunk has edits: then the edited blocks
    /// are the ones loaded and `data` is dropped.
    pub fn insert_generated(&mut self, chunk: IVec3, data: ChunkData) {
        if self.edited.contains(&chunk) || self.restore_edited(chunk) {
            return;
        }
        self.place_chunk(chunk, data);
    }

    /// Loads a chunk from its kept edits. Returns false if there are none, which means it
    /// has to be generated.
    pub fn restore_edited(&mut self, chunk: IVec3) -> bool {
        let Some(runs) = self.unloaded_edits.remove(&chunk) else {
            return false;
        };
        let data = ChunkData::from_runs(&runs).expect("kept runs always fill a chunk");
        self.edited.insert(chunk);
        self.place_chunk(chunk, data);
        true
    }

    fn place_chunk(&mut self, chunk: IVec3, data: ChunkData) {
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

    /// Takes a chunk out of the world for good: if it was edited, the edits go with it.
    pub fn remove_chunk(&mut self, chunk: IVec3) -> Option<ChunkData> {
        self.dirty.remove(&chunk);
        self.edited.remove(&chunk);
        self.chunks.remove(&chunk)
    }

    /// Drops a chunk nobody is near. An edited chunk is kept in run-length form and comes
    /// back when it is next loaded; any other can be generated again, so it is discarded.
    /// Returns false if the chunk wasn't loaded.
    pub fn unload_chunk(&mut self, chunk: IVec3) -> bool {
        self.dirty.remove(&chunk);
        let Some(data) = self.chunks.remove(&chunk) else {
            return false;
        };
        if self.edited.remove(&chunk) {
            self.unloaded_edits.insert(chunk, data.to_runs());
        }
        true
    }

    /// Whether the chunk has been changed from what the generator makes, loaded or not.
    pub fn is_edited(&self, chunk: IVec3) -> bool {
        self.edited.contains(&chunk) || self.unloaded_edits.contains_key(&chunk)
    }

    pub fn edited_chunk_count(&self) -> usize {
        self.edited.len() + self.unloaded_edits.len()
    }

    /// Every edited chunk, loaded or not, in a fixed order so that saving the same world
    /// twice writes the same bytes.
    pub(crate) fn edited_chunks(&self) -> Vec<(IVec3, Runs)> {
        let loaded = self
            .edited
            .iter()
            .map(|chunk| (*chunk, self.chunks[chunk].to_runs()));
        let unloaded = self
            .unloaded_edits
            .iter()
            .map(|(chunk, runs)| (*chunk, runs.clone()));
        let mut chunks: Vec<(IVec3, Runs)> = loaded.chain(unloaded).collect();
        chunks.sort_unstable_by_key(|(chunk, _)| chunk.to_array());
        chunks
    }

    /// Makes `chunk` an edited chunk with these blocks: replaced and remeshed if it is
    /// loaded, kept for when it streams in if it isn't. The runs must fill a chunk.
    pub(crate) fn set_edited_chunk(&mut self, chunk: IVec3, runs: Runs) {
        if self.chunks.contains_key(&chunk) {
            let data = ChunkData::from_runs(&runs).expect("the runs must fill a chunk");
            self.edited.insert(chunk);
            self.place_chunk(chunk, data);
        } else {
            self.unloaded_edits.insert(chunk, runs);
        }
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
        self.edited.insert(chunk);
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
