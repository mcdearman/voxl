use std::{collections::HashMap, ops::Range};

use glam::{Mat3, Mat4};
use wgpu::util::DeviceExt;

use super::{
    gpu::{Gpu, DEPTH_FORMAT},
    mesh::{Mesh, Vertex},
    RenderFrame,
};
use crate::assets::Assets;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
    camera_position: [f32; 4],
    light_direction: [f32; 4],
    light_color: [f32; 4],
    ambient_color: [f32; 4],
}

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
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    meshes: HashMap<u32, GpuMesh>,
    instances: Vec<InstanceRaw>,
    batches: Vec<(u32, Range<u32>)>,
    /// Set when the surface needs reconfiguring; handled by an exclusive system after drawing.
    pub(crate) surface_lost: bool,
}

impl MeshRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view uniform"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view bind group"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[&view_layout],
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
            view_buffer,
            view_bind_group,
            instance_buffer: create_instance_buffer(device, instance_capacity),
            instance_capacity,
            meshes: HashMap::new(),
            instances: Vec::new(),
            batches: Vec::new(),
            surface_lost: false,
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

    pub fn render(&mut self, gpu: &Gpu, frame: &mut RenderFrame) {
        let view = ViewUniform {
            view_proj: frame.view_proj.to_cols_array_2d(),
            camera_position: frame.camera_position.extend(1.0).into(),
            light_direction: frame.light_direction.extend(0.0).into(),
            light_color: frame.light_color.extend(1.0).into(),
            ambient_color: frame.ambient_color.extend(1.0).into(),
        };
        gpu.queue
            .write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view));

        self.build_batches(gpu, frame);

        let output = match gpu.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface_lost = true;
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(err) => {
                log::error!("failed to acquire a frame: {err}");
                return;
            }
        };
        let target = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let c = frame.clear_color;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c.r as f64,
                            g: c.g as f64,
                            b: c.b as f64,
                            a: c.a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gpu.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.view_bind_group, &[]);
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
        gpu.queue.submit([encoder.finish()]);
        output.present();
    }

    /// Sorts objects by mesh so each mesh becomes one contiguous range in the instance buffer.
    fn build_batches(&mut self, gpu: &Gpu, frame: &mut RenderFrame) {
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
