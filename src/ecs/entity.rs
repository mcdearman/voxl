use crate::ecs::chunk::Chunk;

pub enum Entity {
    Voxel(u64),
    Chunk(Chunk)
}
