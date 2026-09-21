use glam::{IVec3, Vec3};

use super::{
    block::{BlockId, BlockRegistry},
    chunk::{ChunkData, CHUNK_SIZE},
    world::VoxelWorld,
};

/// 24 bytes per vertex. `packed` holds the face index (bits 0-2), the ambient occlusion level
/// (bits 3-4) and the texture layer (the rest); the shader looks the normal up from the face.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VoxelVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub packed: u32,
}

impl VoxelVertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    pub vertices: Vec<VoxelVertex>,
    pub indices: Vec<u32>,
}

/// Normal, then the two axes spanning the face. `u × v = normal` keeps the winding
/// counter-clockwise, and `v` points up on side faces so textures are upright.
/// The order matches `Block::layers` and the table in `voxel.wgsl`.
pub const FACES: [(IVec3, IVec3, IVec3); 6] = [
    (IVec3::X, IVec3::NEG_Z, IVec3::Y),
    (IVec3::NEG_X, IVec3::Z, IVec3::Y),
    (IVec3::Y, IVec3::Z, IVec3::X),
    (IVec3::NEG_Y, IVec3::X, IVec3::Z),
    (IVec3::Z, IVec3::X, IVec3::Y),
    (IVec3::NEG_Z, IVec3::NEG_X, IVec3::Y),
];

const CORNERS: [(i32, i32); 4] = [(-1, -1), (1, -1), (1, 1), (-1, 1)];

const PADDED: i32 = CHUNK_SIZE + 2;

/// A copy of a chunk plus a one-block border from its 26 neighbours. It owns its data, so it
/// can be meshed on another thread while the world keeps changing.
pub struct PaddedChunk {
    blocks: Vec<BlockId>,
}

impl PaddedChunk {
    pub fn capture(world: &VoxelWorld, chunk: IVec3) -> Self {
        let mut neighbours: [Option<&ChunkData>; 27] = [None; 27];
        let slot =
            |offset: IVec3| (((offset.z + 1) * 3 + offset.y + 1) * 3 + offset.x + 1) as usize;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let offset = IVec3::new(dx, dy, dz);
                    neighbours[slot(offset)] = world.chunk(chunk + offset);
                }
            }
        }

        let size = IVec3::splat(CHUNK_SIZE);
        let mut blocks = Vec::with_capacity((PADDED * PADDED * PADDED) as usize);
        for y in -1..=CHUNK_SIZE {
            for z in -1..=CHUNK_SIZE {
                for x in -1..=CHUNK_SIZE {
                    let position = IVec3::new(x, y, z);
                    let source = neighbours[slot(position.div_euclid(size))];
                    blocks.push(
                        source.map_or(BlockId::UNLOADED, |c| c.get(position.rem_euclid(size))),
                    );
                }
            }
        }
        Self { blocks }
    }

    /// `local` may be anywhere from -1 to `CHUNK_SIZE` on each axis.
    fn get(&self, local: IVec3) -> BlockId {
        let p = local + 1;
        self.blocks[((p.y * PADDED + p.z) * PADDED + p.x) as usize]
    }
}

/// Builds a mesh with hidden faces removed and per-vertex ambient occlusion.
/// Vertices are in world space, so chunks need no per-draw transform.
pub fn mesh_chunk(padded: &PaddedChunk, registry: &BlockRegistry, chunk: IVec3) -> ChunkMesh {
    let mut mesh = ChunkMesh::default();
    let origin = chunk * CHUNK_SIZE;
    let solid = |local: IVec3| registry.is_solid(padded.get(local));

    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let local = IVec3::new(x, y, z);
                let id = padded.get(local);
                if id == BlockId::AIR {
                    continue;
                }
                let Some(block) = registry.get(id) else {
                    continue;
                };
                for (face, (normal, u, v)) in FACES.iter().enumerate() {
                    let front = local + *normal;
                    if solid(front) {
                        continue;
                    }

                    // A corner gets darker the more of its three neighbours in the layer in
                    // front of the face are solid. 3 is fully lit, 0 is fully occluded.
                    let ao = CORNERS.map(|(su, sv)| {
                        let (side_a, side_b) = (solid(front + *u * su), solid(front + *v * sv));
                        if side_a && side_b {
                            0
                        } else {
                            3 - (side_a as u32
                                + side_b as u32
                                + solid(front + *u * su + *v * sv) as u32)
                        }
                    });

                    let base = mesh.vertices.len() as u32;
                    let center = (origin + local).as_vec3() + 0.5 + normal.as_vec3() * 0.5;
                    for (i, (su, sv)) in CORNERS.iter().enumerate() {
                        let offset = (*u * *su + *v * *sv).as_vec3() * 0.5;
                        mesh.vertices.push(VoxelVertex {
                            position: (center + offset).into(),
                            uv: [(*su + 1) as f32 * 0.5, (1 - *sv) as f32 * 0.5],
                            packed: face as u32 | ao[i] << 3 | block.layers[face] << 5,
                        });
                    }
                    // Split the quad along the diagonal that keeps the shading symmetric;
                    // otherwise a single dark corner smears across the whole face.
                    let order = if ao[0] + ao[2] < ao[1] + ao[3] {
                        [1, 2, 3, 1, 3, 0]
                    } else {
                        [0, 1, 2, 0, 2, 3]
                    };
                    mesh.indices.extend(order.map(|i| base + i));
                }
            }
        }
    }
    mesh
}

/// The world-space bounds of a chunk, for culling.
pub fn chunk_bounds(chunk: IVec3) -> (Vec3, Vec3) {
    let min = (chunk * CHUNK_SIZE).as_vec3();
    (min, min + CHUNK_SIZE as f32)
}
