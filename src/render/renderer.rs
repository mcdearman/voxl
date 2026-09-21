use std::{collections::HashMap, ops::Range};

use glam::{Mat3, Mat4};
use wgpu::util::DeviceExt;

use super::{
    gpu::{Gpu, DEPTH_FORMAT},
    mesh::{Mesh, Vertex},
    view::ViewBinding,
    RenderFrame,
};
use crate::assets::Assets;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 3]; 3],
    color: [f32; 4],
}

impl InstanceRaw {
    const ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        7 => Float32x3, 8 => Float32x3, 9 => Float32x3,
        10 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    fn new(model: Mat4, color: [f32; 4]) -> Self {
        // Inverse-transpose keeps normals perpendicular under non-uniform scale.
        let normal = Mat3::from_mat4(model).inverse().transpose();
        Self {
            model: model.to_cols_array_2d(),
            normal: normal.to_cols_array_2d(),
            color,
        }
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Draws every `Mesh3d` with one instanced draw call per mesh.
pub struct MeshRenderer {
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    meshes: HashMap<u32, GpuMesh>,
    instances: Vec<InstanceRaw>,
    batches: Vec<(u32, Range<u32>)>,
}

impl MeshRenderer {
    pub fn new(gpu: &Gpu, view: &ViewBinding) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[&view.layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::layout(), InstanceRaw::layout()],
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

        let instance_capacity = 64;
        Self {
            pipeline,
            instance_buffer: create_instance_buffer(device, instance_capacity),
            instance_capacity,
            meshes: HashMap::new(),
            instances: Vec::new(),
            batches: Vec::new(),
        }
    }

    /// Uploads meshes that were added or modified, and frees removed ones.
    pub fn sync_meshes(&mut self, gpu: &Gpu, meshes: &mut Assets<Mesh>) {
        let (modified, removed) = meshes.take_changes();
        for id in removed {
            self.meshes.remove(&id);
        }
        for id in modified {
            let Some(mesh) = meshes.get_by_id(id) else {
                continue;
            };
            let vertices = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let indices = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
            self.meshes.insert(
                id,
                GpuMesh {
                    vertices,
                    indices,
                    index_count: mesh.indices.len() as u32,
                },
            );
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.batches.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        for (mesh_id, range) in &self.batches {
            let Some(mesh) = self.meshes.get(mesh_id) else {
                continue;
            };
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, range.clone());
        }
    }

    /// Sorts objects by mesh so each mesh becomes one contiguous range in the instance buffer.
    pub fn build_batches(&mut self, gpu: &Gpu, frame: &mut RenderFrame) {
        frame.objects.sort_unstable_by_key(|(mesh, _, _)| *mesh);
        self.instances.clear();
        self.batches.clear();
        for (i, (mesh, model, color)) in frame.objects.iter().enumerate() {
            let i = i as u32;
            match self.batches.last_mut() {
                Some((id, range)) if id == mesh => range.end = i + 1,
                _ => self.batches.push((*mesh, i..i + 1)),
            }
            self.instances
                .push(InstanceRaw::new(*model, color.to_array()));
        }

        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(&gpu.device, self.instance_capacity);
        }
        gpu.queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&self.instances),
        );
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * std::mem::size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
