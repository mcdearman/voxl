use glam::IVec3;

use super::block::BlockId;

pub const CHUNK_SIZE: i32 = 32;
const VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// The blocks of one chunk. Chunks made of a single block type (open sky, deep stone) store
/// just that id, and only allocate the full grid when something different is written.
#[derive(Clone, Debug, PartialEq)]
pub enum ChunkData {
    Uniform(BlockId),
    Dense(Box<[BlockId]>),
}

impl Default for ChunkData {
    fn default() -> Self {
        Self::Uniform(BlockId::AIR)
    }
}

fn index(local: IVec3) -> usize {
    debug_assert!(
        local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all(),
        "{local} is outside the chunk"
    );
    ((local.y * CHUNK_SIZE + local.z) * CHUNK_SIZE + local.x) as usize
}

impl ChunkData {
    pub fn get(&self, local: IVec3) -> BlockId {
        match self {
            Self::Uniform(id) => *id,
            Self::Dense(blocks) => blocks[index(local)],
        }
    }

    /// Returns the block that was there before.
    pub fn set(&mut self, local: IVec3, id: BlockId) -> BlockId {
        match self {
            Self::Uniform(current) if *current == id => id,
            Self::Uniform(current) => {
                let previous = *current;
                let mut blocks = vec![previous; VOLUME].into_boxed_slice();
                blocks[index(local)] = id;
                *self = Self::Dense(blocks);
                previous
            }
            Self::Dense(blocks) => std::mem::replace(&mut blocks[index(local)], id),
        }
    }

    /// Collapses back to `Uniform` if every block is the same. Worth calling after generation.
    pub fn compact(&mut self) {
        if let Self::Dense(blocks) = self {
            let first = blocks[0];
            if blocks.iter().all(|b| *b == first) {
                *self = Self::Uniform(first);
            }
        }
    }

    pub fn is_uniform(&self, id: BlockId) -> bool {
        matches!(self, Self::Uniform(current) if *current == id)
    }
}

/// The chunk containing a world-space block position.
pub fn chunk_of(block: IVec3) -> IVec3 {
    block.div_euclid(IVec3::splat(CHUNK_SIZE))
}

/// A block position relative to its chunk's origin.
pub fn local_of(block: IVec3) -> IVec3 {
    block.rem_euclid(IVec3::splat(CHUNK_SIZE))
}
