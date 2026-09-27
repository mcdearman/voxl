//! Hardware ray tracing, through wgpu's (experimental, Vulkan-only) ray queries.
//!
//! Every opaque mesh gets a bottom-level acceleration structure once; each frame the scene's
//! instances are gathered into a top-level structure that shaders trace against, for sun
//! shadows, ambient occlusion and reflections.
//!
//! Reflections shade what they hit properly: every traced mesh's normals and texture
//! coordinates live in one storage buffer, each instance has a material entry, and all
//! base-colour textures are bound together as an array, so a reflected wall shows its real
//! stonework, lit by the sun with its own shadow ray.
//!
//! Ray queries can't yet test alpha, so cut-out geometry such as leaves stays out of the
//! traced scene and keeps casting through the shadow maps.

use std::collections::HashMap;

use glam::Mat4;
use wgpu::util::DeviceExt;

use super::{gpu::Gpu, mesh::Mesh, probes::{ProbeGrid, ProbeVolume}, Color};
use crate::assets::Assets;

/// The most instances the ray-traced scene holds at once.
pub const MAX_INSTANCES: u32 = 1 << 16;
/// How many distinct textures reflections can show; slot 0 is plain white.
pub const TEXTURE_SLOTS: u32 = 256;
/// Floats per vertex in the hit buffer: normal, then texture coordinate.
const HIT_STRIDE: usize = 5;

/// Whether to use ray tracing when the GPU supports it. Insert before `DefaultPlugins`.
#[derive(Clone, Copy, Debug)]
pub struct RayTracingSettings {
    pub enabled: bool,
    /// Meshes further from the camera than this are left out of the traced scene.
    pub max_distance: f32,
    /// Light probes for the light from all around, baked once the scene settles. Without
    /// them surfaces see only the sky's light, as if nothing stood around them.
    pub probes: Option<ProbeGrid>,
}

impl Default for RayTracingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_distance: 400.0,
            probes: None,
        }
    }
}

struct Geometry {
    positions: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Bytes from one vertex position to the next.
    stride: u64,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
    blas: wgpu::Blas,
    built: bool,
    /// Rebuilt every frame, from vertices the GPU poses (a skinned figure).
    dynamic: bool,
}

/// A handle to geometry added with [`RayTracing::add_geometry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GeometryId(u32);

/// How a traced surface looks when a reflection ray hits it. Must match `HitInstance` in
/// `rt_on.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HitMaterial {
    pub color: [f32; 4],
    pub emissive: [f32; 4],
    pub(crate) geometry: u32,
    /// Slot in the texture array; 0 is plain white.
    pub texture: u32,
    pub roughness: f32,
    pub metallic: f32,
}

impl HitMaterial {
    pub fn plain(color: Color) -> Self {
        Self {
            color: color.to_array(),
            emissive: [0.0; 4],
            geometry: 0,
            texture: 0,
            roughness: 0.8,
            metallic: 0.0,
        }
    }
}

/// The ray-traced scene. Present only when the GPU supports ray queries.
pub struct RayTracing {
    pub tlas: wgpu::Tlas,
    geometry: Vec<Geometry>,
    meshes: HashMap<u32, GeometryId>,
    /// Instances that never move.
    fixed: Vec<(GeometryId, Mat4, HitMaterial)>,
    /// This frame's instances, fixed ones first.
    frame: Vec<(GeometryId, Mat4, HitMaterial)>,
    used: u32,
    pub settings: RayTracingSettings,

    /// Normals and texture coordinates of every geometry, end to end.
    hit_vertices: Vec<f32>,
    hit_indices: Vec<u32>,
    /// (first vertex, first index) per geometry.
    hit_ranges: Vec<[u32; 2]>,
    pub(crate) vertex_buffer: wgpu::Buffer,
    pub(crate) index_buffer: wgpu::Buffer,
    pub(crate) range_buffer: wgpu::Buffer,
    pub(crate) instance_buffer: wgpu::Buffer,
    buffers_stale: bool,

    textures: Vec<wgpu::TextureView>,
    slots: HashMap<u32, u32>,
    pub(crate) sampler: wgpu::Sampler,
    /// Changes whenever the bindings shaders see must be rebuilt.
    pub(crate) generation: u64,
    pub(crate) probes: ProbeVolume,
}

fn storage(device: &wgpu::Device, label: &str, contents: &[u8]) -> wgpu::Buffer {
    // Bindings can't be empty.
    let padded;
    let contents = if contents.is_empty() {
        padded = [0u8; 16];
        &padded[..]
    } else {
        contents
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

impl RayTracing {
    pub(crate) fn new(gpu: &Gpu, settings: RayTracingSettings, white: wgpu::TextureView) -> Self {
        let device = &gpu.device;
        let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("scene"),
            max_instances: MAX_INSTANCES,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hit instances"),
            size: MAX_INSTANCES as u64 * std::mem::size_of::<HitMaterial>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hit sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            tlas,
            geometry: Vec::new(),
            meshes: HashMap::new(),
            fixed: Vec::new(),
            frame: Vec::new(),
            used: 0,
            settings,
            hit_vertices: Vec::new(),
            hit_indices: Vec::new(),
            hit_ranges: Vec::new(),
            vertex_buffer: storage(device, "hit vertices", &[]),
            index_buffer: storage(device, "hit indices", &[]),
            range_buffer: storage(device, "hit ranges", &[]),
            instance_buffer,
            buffers_stale: false,
            textures: vec![white],
            slots: HashMap::new(),
            sampler,
            generation: 1,
            probes: ProbeVolume::new(gpu, settings.probes),
        }
    }

    /// Adds triangles to trace against; place them with [`Self::add_fixed`] or per frame.
    pub fn add_geometry(
        &mut self,
        gpu: &Gpu,
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
    ) -> GeometryId {
        let device = &gpu.device;
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: positions.len() as u32,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(indices.len() as u32),
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("blas"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        let input = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::BLAS_INPUT,
            })
        };
        self.geometry.push(Geometry {
            positions: input("blas positions", bytemuck::cast_slice(positions)),
            indices: input("blas indices", bytemuck::cast_slice(indices)),
            stride: 12,
            size,
            blas,
            built: false,
            dynamic: false,
        });

        let first_vertex = (self.hit_vertices.len() / HIT_STRIDE) as u32;
        for (i, n) in normals.iter().enumerate() {
            let uv = uvs.get(i).copied().unwrap_or([0.0; 2]);
            self.hit_vertices.extend([n[0], n[1], n[2], uv[0], uv[1]]);
        }
        let first_index = self.hit_indices.len() as u32;
        self.hit_indices.extend_from_slice(indices);
        self.hit_ranges.push([first_vertex, first_index]);
        self.buffers_stale = true;
        GeometryId(self.geometry.len() as u32 - 1)
    }

    /// An instance that stays put for the rest of the run, like terrain or a building.
    pub fn add_fixed(&mut self, geometry: GeometryId, transform: Mat4, material: HitMaterial) {
        self.fixed.push((geometry, transform, material));
    }

    /// The traced copy of an engine mesh, made the first time it's needed.
    pub(crate) fn mesh_geometry(&mut self, gpu: &Gpu, meshes: &Assets<Mesh>, id: u32) -> Option<GeometryId> {
        if let Some(g) = self.meshes.get(&id) {
            return Some(*g);
        }
        let mesh = meshes.get_by_id(id)?;
        if mesh.indices.is_empty() {
            return None;
        }
        let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
        let normals: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.normal).collect();
        let uvs: Vec<[f32; 2]> = mesh.vertices.iter().map(|v| v.uv).collect();
        let g = self.add_geometry(gpu, &positions, &normals, &uvs, &mesh.indices);
        self.meshes.insert(id, g);
        Some(g)
    }

    /// The traced copy of a skinned mesh: built from its GPU vertex buffer (the engine's
    /// `Vertex`, positions first) and rebuilt every frame as it moves.
    pub(crate) fn skinned_geometry(&mut self, gpu: &Gpu, id: u32, mesh: &Mesh, vertices: &wgpu::Buffer, indices: &wgpu::Buffer) -> GeometryId {
        if let Some(g) = self.meshes.get(&id) {
            return *g;
        }
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: mesh.vertices.len() as u32,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(mesh.indices.len() as u32),
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = gpu.device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("skinned blas"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        self.geometry.push(Geometry {
            positions: vertices.clone(),
            indices: indices.clone(),
            stride: std::mem::size_of::<super::mesh::Vertex>() as u64,
            size,
            blas,
            built: false,
            dynamic: true,
        });
        // Reflections shade it with its rest-pose normals: near enough.
        let first_vertex = (self.hit_vertices.len() / HIT_STRIDE) as u32;
        for v in &mesh.vertices {
            self.hit_vertices.extend([v.normal[0], v.normal[1], v.normal[2], v.uv[0], v.uv[1]]);
        }
        let first_index = self.hit_indices.len() as u32;
        self.hit_indices.extend_from_slice(&mesh.indices);
        self.hit_ranges.push([first_vertex, first_index]);
        self.buffers_stale = true;
        let g = GeometryId(self.geometry.len() as u32 - 1);
        self.meshes.insert(id, g);
        g
    }

    /// The texture array slot for an image, adding it if there's room.
    pub(crate) fn texture_slot(&mut self, image: u32, view: Option<&wgpu::TextureView>) -> u32 {
        if let Some(slot) = self.slots.get(&image) {
            return *slot;
        }
        let Some(view) = view else {
            return 0;
        };
        if self.textures.len() >= TEXTURE_SLOTS as usize {
            return 0;
        }
        self.textures.push(view.clone());
        let slot = self.textures.len() as u32 - 1;
        self.slots.insert(image, slot);
        self.generation += 1;
        slot
    }

    /// Every texture slot, padded with white to the array's full length.
    pub(crate) fn texture_views(&self) -> Vec<&wgpu::TextureView> {
        (0..TEXTURE_SLOTS as usize)
            .map(|i| self.textures.get(i).unwrap_or(&self.textures[0]))
            .collect()
    }

    /// Whether every piece of geometry has been built into the traced scene.
    pub(crate) fn ready(&self) -> bool {
        self.geometry.iter().all(|g| g.built || g.dynamic)
    }

    /// Identifies the traced scene's contents: its geometry and textures, and how many
    /// instances it holds.
    pub(crate) fn scene_key(&self) -> (u64, usize) {
        (self.generation, self.frame.len())
    }

    pub(crate) fn begin_frame(&mut self) {
        self.frame.clear();
        self.frame.extend_from_slice(&self.fixed);
    }

    pub(crate) fn push(&mut self, geometry: GeometryId, transform: Mat4, material: HitMaterial) {
        if self.frame.len() < MAX_INSTANCES as usize {
            self.frame.push((geometry, transform, material));
        }
    }

    /// Uploads this frame's instance materials, and the hit buffers if geometry was added.
    pub(crate) fn upload(&mut self, gpu: &Gpu) {
        if self.buffers_stale {
            self.buffers_stale = false;
            self.vertex_buffer = storage(&gpu.device, "hit vertices", bytemuck::cast_slice(&self.hit_vertices));
            self.index_buffer = storage(&gpu.device, "hit indices", bytemuck::cast_slice(&self.hit_indices));
            self.range_buffer = storage(&gpu.device, "hit ranges", bytemuck::cast_slice(&self.hit_ranges));
            self.generation += 1;
        }
        let materials: Vec<HitMaterial> = self
            .frame
            .iter()
            .map(|(geometry, _, m)| HitMaterial {
                geometry: geometry.0,
                ..*m
            })
            .collect();
        if !materials.is_empty() {
            gpu.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&materials));
        }
    }

    /// Records the acceleration structure builds for this frame.
    pub(crate) fn build(&mut self, encoder: &mut wgpu::CommandEncoder) {
        // Build a few million triangles' worth of new geometry a frame at most: a big scene
        // built all at once can keep the GPU busy long enough for Windows to reset it.
        let mut budget = 2_000_000u32;
        let pending: Vec<usize> = (0..self.geometry.len())
            .filter(|i| !self.geometry[*i].built && !self.geometry[*i].dynamic)
            .take_while(|i| {
                let triangles = self.geometry[*i].size.index_count.unwrap_or(0) / 3;
                let go = budget > 0;
                budget = budget.saturating_sub(triangles);
                go
            })
            .chain((0..self.geometry.len()).filter(|i| self.geometry[*i].dynamic))
            .collect();
        for (i, (geometry, transform, _)) in self.frame.iter().enumerate() {
            let g = geometry.0 as usize;
            if !self.geometry[g].built && !pending.contains(&g) {
                // Not built yet: leave it out this frame.
                self.tlas[i] = None;
                continue;
            }
            let t = transform.transpose().to_cols_array();
            // Row-major 3x4: the first three rows of the (column-major) matrix.
            let rows: [f32; 12] = t[..12].try_into().unwrap();
            // The custom index finds the instance's material, however many are left out.
            self.tlas[i] = Some(wgpu::TlasInstance::new(&self.geometry[g].blas, rows, i as u32, 0xff));
        }
        for i in self.frame.len()..self.used as usize {
            self.tlas[i] = None;
        }
        self.used = self.frame.len() as u32;

        let entries: Vec<wgpu::BlasBuildEntry> = pending
            .iter()
            .map(|&i| {
                let g = &self.geometry[i];
                wgpu::BlasBuildEntry {
                    blas: &g.blas,
                    geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                        size: &g.size,
                        vertex_buffer: &g.positions,
                        first_vertex: 0,
                        vertex_stride: g.stride,
                        index_buffer: Some(&g.indices),
                        first_index: Some(0),
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    }]),
                }
            })
            .collect();
        encoder.build_acceleration_structures(entries.iter(), std::iter::once(&self.tlas));
        drop(entries);
        for i in pending {
            self.geometry[i].built = true;
        }
    }
}

