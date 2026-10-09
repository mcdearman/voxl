//! Skinning on the GPU: every frame a compute pass poses each skinned mesh by its joint
//! palette, writing the posed vertices straight into the mesh's own vertex buffer. Drawing,
//! the shadow cascades and the ray-traced scene then all see the posed figure.

use std::{collections::HashMap, sync::Arc};

use glam::Mat4;
use wgpu::util::DeviceExt;

use super::{animation::SkinWeights, gpu::Gpu, mesh::Mesh, renderer::MeshRenderer};
use crate::assets::Assets;

/// One skinned mesh to pose this frame.
pub(crate) struct SkinJob {
    /// The entity's own copy of the mesh, which the posed vertices overwrite.
    pub mesh: u32,
    /// The shared mesh in its bind pose.
    pub source: u32,
    pub weights: Arc<SkinWeights>,
    pub palette: Vec<Mat4>,
}

struct Source {
    rest: wgpu::Buffer,
    influences: wgpu::Buffer,
    vertices: u32,
}

pub(crate) struct Skinner {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    sources: HashMap<u32, Source>,
    palette: wgpu::Buffer,
    palette_capacity: usize,
    params: wgpu::Buffer,
    params_capacity: usize,
    /// Bind groups and workgroup counts for this frame's dispatches.
    dispatches: Vec<(wgpu::BindGroup, u32)>,
}

const PARAMS_STRIDE: u64 = 256;

impl Skinner {
    /// Takes the pipeline of a skinner freshly built from the current shader, keeping this
    /// one's buffers.
    pub(crate) fn adopt_pipeline(&mut self, fresh: Self) {
        self.pipeline = fresh.pipeline;
    }

    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let module = crate::shader!("skin.wgsl").module(device);
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skinning layout"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, true),
                storage(3, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skinning"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("skinning"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("skin"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            layout,
            sources: HashMap::new(),
            palette: Self::buffer(device, "skin palette", 64 * 64, wgpu::BufferUsages::STORAGE),
            palette_capacity: 64,
            params: Self::buffer(device, "skin params", PARAMS_STRIDE * 64, wgpu::BufferUsages::UNIFORM),
            params_capacity: 64,
            dispatches: Vec::new(),
        }
    }

    fn buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads this frame's palettes and prepares a dispatch for every job whose meshes are on
    /// the GPU.
    pub(crate) fn prepare(&mut self, gpu: &Gpu, renderer: &MeshRenderer, meshes: &Assets<Mesh>, jobs: &[SkinJob]) {
        self.dispatches.clear();
        if jobs.is_empty() {
            return;
        }
        let device = &gpu.device;
        for job in jobs {
            if self.sources.contains_key(&job.source) {
                continue;
            }
            let Some(mesh) = meshes.get_by_id(job.source) else { continue };
            if mesh.vertices.len() != job.weights.joints.len() {
                log::warn!("skin weights don't match the mesh ({} vertices, {} weights)", mesh.vertices.len(), job.weights.joints.len());
                continue;
            }
            // Per vertex: four joint indices, then four weights (as bits).
            let influences: Vec<u32> = job
                .weights
                .joints
                .iter()
                .zip(&job.weights.weights)
                .flat_map(|(j, w)| [j[0] as u32, j[1] as u32, j[2] as u32, j[3] as u32, w[0].to_bits(), w[1].to_bits(), w[2].to_bits(), w[3].to_bits()])
                .collect();
            let make = |label, contents: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                })
            };
            self.sources.insert(
                job.source,
                Source {
                    rest: make("skin rest pose", bytemuck::cast_slice(&mesh.vertices)),
                    influences: make("skin influences", bytemuck::cast_slice(&influences)),
                    vertices: mesh.vertices.len() as u32,
                },
            );
        }

        let joints: usize = jobs.iter().map(|j| j.palette.len()).sum();
        if joints > self.palette_capacity {
            self.palette_capacity = joints.next_power_of_two();
            self.palette = Self::buffer(device, "skin palette", self.palette_capacity as u64 * 64, wgpu::BufferUsages::STORAGE);
        }
        if jobs.len() > self.params_capacity {
            self.params_capacity = jobs.len().next_power_of_two();
            self.params = Self::buffer(device, "skin params", PARAMS_STRIDE * self.params_capacity as u64, wgpu::BufferUsages::UNIFORM);
        }
        let mut palette: Vec<Mat4> = Vec::with_capacity(joints);
        let mut params = vec![0u8; PARAMS_STRIDE as usize * jobs.len()];
        for (i, job) in jobs.iter().enumerate() {
            let (Some(source), Some(target)) = (self.sources.get(&job.source), renderer.vertex_buffer(job.mesh)) else {
                palette.extend_from_slice(&job.palette);
                continue;
            };
            let offset = palette.len() as u32;
            palette.extend_from_slice(&job.palette);
            let p = [source.vertices, offset, 0, 0];
            params[i * PARAMS_STRIDE as usize..][..16].copy_from_slice(bytemuck::bytes_of(&p));
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skinning"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: source.rest.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: source.influences.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.palette.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: target.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.params,
                            offset: i as u64 * PARAMS_STRIDE,
                            size: wgpu::BufferSize::new(16),
                        }),
                    },
                ],
            });
            self.dispatches.push((bind_group, source.vertices.div_ceil(64)));
        }
        gpu.queue.write_buffer(&self.palette, 0, bytemuck::cast_slice(&palette));
        gpu.queue.write_buffer(&self.params, 0, &params);
    }

    /// Records the posing, before anything reads the vertices.
    pub(crate) fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.dispatches.is_empty() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (bind_group, groups) in &self.dispatches {
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(*groups, 1, 1);
        }
    }
}
