//! Sunlight scattered by the air on its way to the eye: marched along each view ray through
//! the shadow maps, so the air glows where the sun reaches it and stays dark where buildings or
//! trees shade it. The shafts come out of the gaps between them.
//!
//! Each pixel starts its march at a different random offset every frame; TAA averages the
//! offsets into smooth shafts, so a few dozen steps are enough.

use super::gpu::{Gpu, HDR_FORMAT};

/// How hazy the air is, for light shafts. Off unless inserted with `enabled: true`.
#[derive(Clone, Copy, Debug)]
pub struct VolumetricLight {
    pub enabled: bool,
    /// Scattering per metre at `base_height`. Around 0.005 is a hazy day.
    pub density: f32,
    /// How strongly light scatters forward, toward the viewer looking at the sun: 0 evenly, near
    /// 1 only straight on. Air with dust or mist is about 0.6.
    pub anisotropy: f32,
    /// How fast the haze thins with height, per metre.
    pub height_falloff: f32,
    pub base_height: f32,
    /// Metres the march reaches; beyond, the analytic haze takes over.
    pub max_distance: f32,
}

impl Default for VolumetricLight {
    fn default() -> Self {
        Self {
            enabled: false,
            density: 0.005,
            anisotropy: 0.6,
            height_falloff: 0.02,
            base_height: 0.0,
            max_distance: 150.0,
        }
    }
}

/// Must match `Shafts` in `volumetric.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShaftsUniform {
    params: [f32; 4],
    more: [f32; 4],
}

pub(crate) struct Shafts {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
}

impl Shafts {
    pub(crate) fn new(gpu: &Gpu, view_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gpu.device;
        let source = format!("{}{}", gpu.pbr_wgsl(), crate::shader!("volumetric.wgsl").source());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("light shafts"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light shafts layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: true,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Depth,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
            label: Some("light shafts"),
            bind_group_layouts: &[view_layout, &layout],
            push_constant_ranges: &[],
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("light shafts"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_shafts"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_shafts"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState { color: add, alpha: add }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("light shafts uniform"),
                size: std::mem::size_of::<ShaftsUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        }
    }

    /// Adds the scattered light to the resolved HDR image.
    pub(crate) fn render(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, view: &wgpu::BindGroup, settings: &VolumetricLight) {
        let uniform = ShaftsUniform {
            params: [settings.density, settings.anisotropy, settings.max_distance, settings.height_falloff],
            more: [settings.base_height, 0.0, 0.0, 0.0],
        };
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light shafts"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.targets.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("light shafts"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gpu.targets.hdr,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
