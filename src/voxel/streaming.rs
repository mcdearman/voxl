use std::collections::HashSet;

use glam::{IVec3, Vec3Swizzles};

use super::{
    block::{BlockId, BlockRegistry},
    chunk::{chunk_of, ChunkData},
    generator::ChunkGenerator,
    mesher::{mesh_chunk, ChunkMesh, PaddedChunk},
    world::VoxelWorld,
};
use crate::{
    ecs::{Component, Query, Res, ResMut, With},
    tasks::{Mailbox, TaskPool},
    transform::GlobalTransform,
};

/// Chunks are kept loaded around entities with this component (usually the camera).
#[derive(Clone, Copy, Debug, Default, crate::reflect::Reflect)]
#[reflect(name = "mira.ChunkViewer")]
pub struct ChunkViewer;

impl Component for ChunkViewer {}

#[derive(Clone, Copy, Debug)]
pub struct VoxelSettings {
    /// How far, in chunks, terrain is visible horizontally.
    pub view_radius: i32,
    /// The inclusive vertical range of chunks that exist. Space outside it is never loaded.
    pub min_chunk_y: i32,
    pub max_chunk_y: i32,
}

impl Default for VoxelSettings {
    fn default() -> Self {
        Self {
            view_radius: 8,
            min_chunk_y: 0,
            max_chunk_y: 3,
        }
    }
}

impl VoxelSettings {
    fn in_vertical_range(&self, chunk: IVec3) -> bool {
        (self.min_chunk_y..=self.max_chunk_y).contains(&chunk.y)
    }
}

/// Bookkeeping for work in flight, and finished meshes waiting for the renderer.
#[derive(Default)]
pub struct ChunkStreaming {
    generating: HashSet<IVec3>,
    meshing: HashSet<IVec3>,
    generated: Mailbox<(IVec3, ChunkData)>,
    meshed: Mailbox<(IVec3, ChunkMesh)>,
    /// Drained by the renderer's prepare system.
    pub(crate) uploads: Vec<(IVec3, ChunkMesh)>,
    pub(crate) removals: Vec<IVec3>,
}

impl ChunkStreaming {
    pub fn in_flight(&self) -> usize {
        self.generating.len() + self.meshing.len()
    }
}

fn horizontal_distance_squared(a: IVec3, b: IVec3) -> i32 {
    (a.xz() - b.xz()).length_squared()
}

/// Requests generation for missing chunks near the viewer and drops chunks far from it.
pub(crate) fn stream_chunks(
    settings: Res<VoxelSettings>,
    generator: Res<ChunkGenerator>,
    pool: Res<TaskPool>,
    mut world: ResMut<VoxelWorld>,
    mut streaming: ResMut<ChunkStreaming>,
    viewers: Query<&GlobalTransform, With<ChunkViewer>>,
) {
    let streaming = &mut *streaming;
    let Some(viewer) = viewers.iter().next() else {
        return;
    };
    let center = chunk_of(viewer.translation().floor().as_ivec3());
    // Data extends one chunk past the visible radius, because a chunk can only be meshed
    // once all of its neighbours exist. Unloading waits a little further out so that
    // pacing back and forth across a chunk border doesn't thrash.
    let load_radius = settings.view_radius + 1;
    let unload_radius = load_radius + 2;

    while let Ok((chunk, data)) = streaming.generated.receiver.try_recv() {
        streaming.generating.remove(&chunk);
        if horizontal_distance_squared(chunk, center) <= unload_radius * unload_radius {
            // Not `insert_chunk`: edits loaded from a file while this chunk was being
            // generated must win over what the generator made.
            world.insert_generated(chunk, data);
        }
    }

    let stale: Vec<IVec3> = world
        .chunk_positions()
        .filter(|c| horizontal_distance_squared(*c, center) > unload_radius * unload_radius)
        .collect();
    for chunk in stale {
        // Keeps the chunk's blocks if they were edited, since generating can't bring those back.
        world.unload_chunk(chunk);
        streaming.removals.push(chunk);
    }

    // Keep the queue short so requests stay near the viewer as it moves.
    let budget = (pool.threads() * 2).saturating_sub(streaming.generating.len());
    if budget == 0 {
        return;
    }
    let mut missing = Vec::new();
    for x in -load_radius..=load_radius {
        for z in -load_radius..=load_radius {
            if x * x + z * z > load_radius * load_radius {
                continue;
            }
            for y in settings.min_chunk_y..=settings.max_chunk_y {
                let chunk = IVec3::new(center.x + x, y, center.z + z);
                if world.contains_chunk(chunk) || streaming.generating.contains(&chunk) {
                    continue;
                }
                // An edited chunk comes back as it was left, without a trip to the generator.
                if !world.restore_edited(chunk) {
                    missing.push(chunk);
                }
            }
        }
    }
    missing.sort_unstable_by_key(|c| horizontal_distance_squared(*c, center));
    for chunk in missing.into_iter().take(budget) {
        streaming.generating.insert(chunk);
        let generator = generator.clone();
        pool.spawn(&streaming.generated.sender, move || {
            (chunk, (generator.0)(chunk))
        });
    }
}

/// Sends out-of-date chunks to be meshed, and collects the results.
pub(crate) fn remesh_chunks(
    settings: Res<VoxelSettings>,
    registry: Res<BlockRegistry>,
    pool: Res<TaskPool>,
    mut world: ResMut<VoxelWorld>,
    mut streaming: ResMut<ChunkStreaming>,
    viewers: Query<&GlobalTransform, With<ChunkViewer>>,
) {
    let streaming = &mut *streaming;
    while let Ok((chunk, mesh)) = streaming.meshed.receiver.try_recv() {
        streaming.meshing.remove(&chunk);
        if world.contains_chunk(chunk) {
            streaming.uploads.push((chunk, mesh));
        }
    }
    if world.dirty.is_empty() {
        return;
    }

    let center = viewers.iter().next().map_or(IVec3::ZERO, |v| {
        chunk_of(v.translation().floor().as_ivec3())
    });
    let mut ready: Vec<IVec3> = world
        .dirty
        .iter()
        .copied()
        .filter(|chunk| {
            // A chunk being meshed stays dirty and goes again afterwards, so an edit made
            // while its job was running isn't lost.
            !streaming.meshing.contains(chunk) && has_all_neighbours(&world, &settings, *chunk)
        })
        .collect();
    ready.sort_unstable_by_key(|c| (*c - center).length_squared());

    let budget = (pool.threads() * 2).saturating_sub(streaming.meshing.len());
    for chunk in ready.into_iter().take(budget) {
        world.dirty.remove(&chunk);
        if world
            .chunk(chunk)
            .is_some_and(|c| c.is_uniform(BlockId::AIR))
        {
            streaming.uploads.push((chunk, ChunkMesh::default()));
            continue;
        }
        streaming.meshing.insert(chunk);
        let padded = PaddedChunk::capture(&world, chunk);
        let registry = registry.clone();
        pool.spawn(&streaming.meshed.sender, move || {
            (chunk, mesh_chunk(&padded, &registry, chunk))
        });
    }
}

/// Meshing before the neighbours arrive would only produce a mesh that's redone moments later.
fn has_all_neighbours(world: &VoxelWorld, settings: &VoxelSettings, chunk: IVec3) -> bool {
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let neighbour = chunk + IVec3::new(dx, dy, dz);
                if settings.in_vertical_range(neighbour) && !world.contains_chunk(neighbour) {
                    return false;
                }
            }
        }
    }
    true
}
