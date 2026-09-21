use std::sync::Arc;

/// Identifies a block type. What it means is defined by the `BlockRegistry`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockId(pub u16);

impl BlockId {
    pub const AIR: Self = Self(0);
    /// Stands in for blocks in chunks that aren't loaded. Never stored in a chunk.
    pub(crate) const UNLOADED: Self = Self(u16::MAX);
}

/// Face order used everywhere: +X, -X, +Y, -Y, +Z, -Z.
#[derive(Clone, Debug)]
pub struct Block {
    pub name: String,
    /// Solid blocks hide the faces behind them and stop movement.
    pub solid: bool,
    /// Texture array layer for each face.
    pub layers: [u32; 6],
}

impl Block {
    pub fn uniform(name: &str, layer: u32) -> Self {
        Self {
            name: name.into(),
            solid: true,
            layers: [layer; 6],
        }
    }

    pub fn pillar(name: &str, top: u32, side: u32, bottom: u32) -> Self {
        Self {
            name: name.into(),
            solid: true,
            layers: [side, side, top, bottom, side, side],
        }
    }
}

/// The table of block types. Cheap to clone, so worker threads can hold a snapshot.
#[derive(Clone)]
pub struct BlockRegistry {
    blocks: Arc<Vec<Block>>,
}

impl Default for BlockRegistry {
    fn default() -> Self {
        let air = Block {
            name: "air".into(),
            solid: false,
            layers: [0; 6],
        };
        Self {
            blocks: Arc::new(vec![air]),
        }
    }
}

impl BlockRegistry {
    pub fn register(&mut self, block: Block) -> BlockId {
        let blocks = Arc::make_mut(&mut self.blocks);
        let id = u16::try_from(blocks.len()).expect("too many block types");
        assert!(id != BlockId::UNLOADED.0, "too many block types");
        blocks.push(block);
        BlockId(id)
    }

    pub fn get(&self, id: BlockId) -> Option<&Block> {
        self.blocks.get(id.0 as usize)
    }

    pub fn find(&self, name: &str) -> Option<BlockId> {
        let index = self.blocks.iter().position(|b| b.name == name)?;
        Some(BlockId(index as u16))
    }

    /// Unknown ids and unloaded space count as solid, so nothing falls out of the world.
    pub fn is_solid(&self, id: BlockId) -> bool {
        self.get(id).is_none_or(|b| b.solid)
    }
}
