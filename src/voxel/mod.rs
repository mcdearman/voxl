//! Voxel terrain: chunked block storage, background generation and meshing, and a renderer.
//!
//! Blocks are not entities. They live in the [`VoxelWorld`] resource and are addressed by
//! position; entities interact with them through it (raycasts, collision, edits).

mod block;
mod chunk;
mod generator;
mod mesher;
mod render;
mod streaming;
mod world;

#[cfg(test)]
mod tests;

pub use block::{Block, BlockId, BlockRegistry};
pub use chunk::{chunk_of, local_of, ChunkData, CHUNK_SIZE};
pub use generator::{fbm, value_noise, ChunkGenerator, Terrain};
pub use mesher::{mesh_chunk, ChunkMesh, PaddedChunk, VoxelVertex};
pub use render::VoxelRenderer;
pub use streaming::{ChunkStreaming, ChunkViewer, VoxelSettings};
pub use world::{RaycastHit, VoxelWorld};

use crate::{
    app::{App, Plugin, Stage},
    ecs::{Res, ResMut, World},
    render::{DrawFunctions, Fog, Gpu, RenderFrame, TextureArray, ViewBinding},
    tasks::TaskPool,
};

/// The tiles that become the block texture array. Push more before startup to add textures;
/// a tile's index is the `layer` blocks refer to.
pub struct BlockTextures {
    pub tile_size: u32,
    pub tiles: Vec<Vec<u8>>,
}

impl BlockTextures {
    /// Splits a vertical strip of square tiles.
    pub fn from_strip(png: &[u8]) -> Self {
        let image = image::load_from_memory(png)
            .expect("invalid block texture image")
            .to_rgba8();
        let tile_size = image.width();
        let bytes = (tile_size * tile_size * 4) as usize;
        Self {
            tile_size,
            tiles: image
                .as_raw()
                .chunks_exact(bytes)
                .map(<[u8]>::to_vec)
                .collect(),
        }
    }

    /// Adds a speckled tile of roughly the given sRGB colour, and returns its layer.
    pub fn push_noise(&mut self, seed: u32, color: [u8; 3], contrast: f32) -> u32 {
        let mut tile = Vec::with_capacity((self.tile_size * self.tile_size * 4) as usize);
        for y in 0..self.tile_size {
            for x in 0..self.tile_size {
                let p = glam::Vec3::new(x as f32, y as f32, 0.0);
                let shade = 1.0 + (value_noise(seed, p) - 0.5) * contrast;
                tile.extend(color.map(|c| (c as f32 * shade).clamp(0.0, 255.0) as u8));
                tile.push(255);
            }
        }
        self.tiles.push(tile);
        self.tiles.len() as u32 - 1
    }
}

/// Adds voxel terrain. Add it after `DefaultPlugins`, and give a camera [`ChunkViewer`].
pub struct VoxelPlugin {
    pub seed: u32,
    pub settings: VoxelSettings,
}

impl Default for VoxelPlugin {
    fn default() -> Self {
        Self {
            seed: 1,
            settings: VoxelSettings::default(),
        }
    }
}

impl Plugin for VoxelPlugin {
    fn build(&self, app: &mut App) {
        // Layers 0-2 come from the atlas: grass side, grass top, dirt.
        let mut textures =
            BlockTextures::from_strip(include_bytes!("../../res/textures/atlas.png"));
        let stone_layer = textures.push_noise(7, [128, 128, 132], 0.35);
        let sand_layer = textures.push_noise(11, [219, 203, 150], 0.15);

        let mut registry = BlockRegistry::default();
        let terrain = Terrain {
            seed: self.seed,
            grass: registry.register(Block::pillar("grass", 1, 0, 2)),
            dirt: registry.register(Block::uniform("dirt", 2)),
            stone: registry.register(Block::uniform("stone", stone_layer)),
            sand: registry.register(Block::uniform("sand", sand_layer)),
        };

        let view_distance = (self.settings.view_radius * CHUNK_SIZE) as f32;
        app.insert_resource(textures)
            .insert_resource(registry)
            .insert_resource(ChunkGenerator::new(move |chunk| terrain.generate(chunk)))
            .insert_resource(self.settings)
            .insert_resource(Fog {
                start: view_distance * 0.6,
                end: view_distance * 0.95,
            })
            .init_resource::<TaskPool>()
            .init_resource::<VoxelWorld>()
            .init_resource::<ChunkStreaming>()
            .add_systems(Stage::PreStartup, init_renderer)
            .add_systems(
                Stage::PostUpdate,
                (streaming::stream_chunks, streaming::remesh_chunks),
            )
            .add_systems(Stage::Prepare, prepare_chunks);
        app.world
            .get_resource_mut::<DrawFunctions>()
            .expect("add VoxelPlugin after DefaultPlugins")
            .0
            .push(draw_chunks);
    }
}

fn init_renderer(world: &mut World) {
    let gpu = world.resource::<Gpu>();
    let tiles = world.resource::<BlockTextures>();
    let textures = TextureArray::from_tiles(gpu, tiles.tile_size, &tiles.tiles);
    let renderer = VoxelRenderer::new(gpu, world.resource::<ViewBinding>(), &textures);
    world.insert_resource(renderer);
}

fn prepare_chunks(
    gpu: Res<Gpu>,
    frame: Res<RenderFrame>,
    mut streaming: ResMut<ChunkStreaming>,
    mut renderer: ResMut<VoxelRenderer>,
) {
    if !streaming.uploads.is_empty() || !streaming.removals.is_empty() {
        for (chunk, mesh) in streaming.uploads.drain(..) {
            renderer.upload(&gpu, chunk, &mesh);
        }
        for chunk in streaming.removals.drain(..) {
            renderer.remove(chunk);
        }
    }
    renderer.cull(frame.view_proj);
}

fn draw_chunks(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    world.resource::<VoxelRenderer>().draw(pass);
}
