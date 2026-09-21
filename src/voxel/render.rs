use std::collections::HashMap;

use glam::{IVec3, Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

use super::mesher::{chunk_bounds, ChunkMesh, VoxelVertex};
use crate::render::{Gpu, TextureArray, ViewBinding, DEPTH_FORMAT};

struct GpuChunk {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Owns the chunk meshes on the GPU and draws the ones inside the camera's view.
pub struct VoxelRenderer {
    pipeline: wgpu::RenderPipeline,
    texture_bind_group: wgpu::BindGroup,
    chunks: HashMap<IVec3, GpuChunk>,
    visible: Vec<IVec3>,
}

impl VoxelRenderer {
    pub fn new(gpu: &Gpu, view: &ViewBinding, textures: &TextureArray) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("voxel.wgsl"));
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("block texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("block textures"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&textures.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&textures.sampler),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel pipeline layout"),
            bind_group_layouts: &[&view.layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[VoxelVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            texture_bind_group,
            chunks: HashMap::new(),
            visible: Vec::new(),
        }
    }

    /// Replaces a chunk's mesh. An empty mesh just removes the old one.
    pub fn upload(&mut self, gpu: &Gpu, chunk: IVec3, mesh: &ChunkMesh) {
        if mesh.indices.is_empty() {
            self.chunks.remove(&chunk);
            return;
        }
        let vertices = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let indices = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.chunks.insert(
            chunk,
            GpuChunk {
                vertices,
                indices,
                index_count: mesh.indices.len() as u32,
            },
        );
    }

    pub fn remove(&mut self, chunk: IVec3) {
        self.chunks.remove(&chunk);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    /// Picks the chunks whose bounds touch the view frustum.
    pub fn cull(&mut self, view_proj: Mat4) {
        let planes = frustum_planes(view_proj);
        self.visible.clear();
        self.visible
            .extend(self.chunks.keys().copied().filter(|chunk| {
                let (min, max) = chunk_bounds(*chunk);
                planes.iter().all(|plane| {
                    // Test only the corner furthest along the plane's normal.
                    let corner = Vec3::select(plane.truncate().cmpge(Vec3::ZERO), max, min);
                    plane.truncate().dot(corner) + plane.w >= 0.0
                })
            }));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.visible.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.texture_bind_group, &[]);
        for chunk in &self.visible {
            let mesh = &self.chunks[chunk];
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
}

/// Inward-facing planes `(normal, d)` of a view-projection matrix with a 0..1 depth range.
fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let (x, y, z, w) = (
        view_proj.row(0),
        view_proj.row(1),
        view_proj.row(2),
        view_proj.row(3),
    );
    [w + x, w - x, w + y, w - y, z, w - z]
}
