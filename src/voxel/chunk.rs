use glam::IVec3;

use super::block::BlockId;

pub const CHUNK_SIZE: i32 = 32;
pub(crate) const VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// A chunk as runs of one block, in storage order: how many, then which. Rows run along x
/// inside horizontal layers, so terrain comes out as a few runs per layer. This is how
/// edited chunks are kept while they aren't loaded, and how they are written to a file.
pub(crate) type Runs = Vec<(u16, BlockId)>;

// A uniform chunk is a single run, so a run's length has to be able to hold a whole chunk.
const _: () = assert!(VOLUME <= u16::MAX as usize);

/// Whether the runs add up to exactly one chunk, with no empty run among them.
pub(crate) fn runs_fill_a_chunk(runs: &[(u16, BlockId)]) -> bool {
    let total: usize = runs.iter().map(|(length, _)| *length as usize).sum();
    total == VOLUME && runs.iter().all(|(length, _)| *length != 0)
}

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

    pub(crate) fn to_runs(&self) -> Runs {
        let blocks = match self {
            Self::Uniform(id) => return vec![(VOLUME as u16, *id)],
            Self::Dense(blocks) => blocks,
        };
        let mut runs = Runs::new();
        for &id in blocks.iter() {
            match runs.last_mut() {
                Some((length, last)) if *last == id => *length += 1,
                _ => runs.push((1, id)),
            }
        }
        runs
    }

    /// `None` unless the runs fill a chunk exactly.
    pub(crate) fn from_runs(runs: &[(u16, BlockId)]) -> Option<Self> {
        if !runs_fill_a_chunk(runs) {
            return None;
        }
        if let [(_, id)] = runs {
            return Some(Self::Uniform(*id));
        }
        let mut blocks = Vec::with_capacity(VOLUME);
        for &(length, id) in runs {
            blocks.extend(std::iter::repeat_n(id, length as usize));
        }
        Some(Self::Dense(blocks.into_boxed_slice()))
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
