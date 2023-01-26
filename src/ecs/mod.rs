mod chunk;
mod components;
mod entity;
mod voxel;

use crate::ecs::entity::Entity;

pub struct ECS {
    entities: Vec<Entity>,
    
}