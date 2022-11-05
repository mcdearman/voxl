pub enum Entity {
    Voxel(u64),
    Chunk(Vec<Voxel>)
}

pub struct World {
    entities: Vec<Entity>,
}