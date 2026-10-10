//! Temporal anti-aliasing: each frame the camera is nudged by a fraction of a pixel, and the
//! frames are blended over time, so every pixel ends up averaged over many sub-pixel positions.
//! Edges and fine texture stop crawling, thin things like grass blades and railings hold
//! steady, and effects that trace a few noisy rays a frame (contact shadows, light shafts)
//! settle into smooth results.
//!
//! The history is re-projected from the depth buffer and the camera's motion, or, for things
//! that themselves moved, along the way they came (see `motion.rs`), and clamped to the range
//! of colours around each pixel this frame.

use glam::{Mat4, Vec2};

use super::gpu::{Gpu, HDR_FORMAT};

/// Sub-pixel offsets, a Halton (2, 3) sequence, in pixels from the pixel centre.
const JITTER: [[f32; 2]; 8] = [
    [0.0, -0.166_666_7],
    [-0.25, 0.166_666_7],
    [0.25, -0.388_888_9],
    [-0.375, -0.055_555_6],
    [0.125, 0.277_777_8],
    [-0.125, -0.277_777_8],
    [0.375, 0.055_555_6],
    [-0.437_5, 0.388_888_9],
];

/// The camera nudge for a frame, in pixels.
pub(crate) fn jitter(frame: u32) -> Vec2 {
    Vec2::from(JITTER[frame as usize % JITTER.len()])
}

/// Must match `Taa` in `taa.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TaaUniform {
    /// From this frame's clip space to the last frame's, without the jitter.
    reproject: [[f32; 4]; 4],
    /// x: 1 if there is a history, y: how much of this frame to blend in, zw: 1 / size.
    params: [f32; 4],
    /// x: 1 if the motion target was drawn into this frame.
    motion: [f32; 4],
}

pub(crate) struct Taa {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// Two histories: read one, write the other, then swap.
    history: Option<(u32, u32, [wgpu::TextureView; 2])>,
    current: usize,
    valid: bool,
    previous_view_proj: Mat4,
}

impl Taa {
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = crate::shader!("taa.wgsl").module(device);
        let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa layout"),
            entries: &[
                texture(0, float, false),
                texture(1, float, false),
                texture(2, wgpu::TextureSampleType::Depth, true),
                texture(5, wgpu::TextureSampleType::Float { filterable: false }, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
            label: Some("taa"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("taa"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("taa sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("taa uniform"),
                size: std::mem::size_of::<TaaUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            history: None,
            current: 0,
            valid: false,
            previous_view_proj: Mat4::IDENTITY,
        }
    }

    /// Blends this frame into the history and returns the result, which post-processing reads.
    /// `view_proj` is this frame's camera without the jitter.
    pub(crate) fn resolve<'a>(&'a mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, view_proj: Mat4, moving: bool) -> &'a wgpu::TextureView {
        let (w, h) = (gpu.config.width, gpu.config.height);
        if !matches!(&self.history, Some((hw, hh, _)) if *hw == w && *hh == h) {
            let make = || {
                gpu.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("taa history"),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: HDR_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            self.history = Some((w, h, [make(), make()]));
            self.valid = false;
        }
        let reproject = self.previous_view_proj * view_proj.inverse();
        let uniform = TaaUniform {
            reproject: reproject.to_cols_array_2d(),
            params: [if self.valid { 1.0 } else { 0.0 }, 0.1, 1.0 / w as f32, 1.0 / h as f32],
            motion: [if moving { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.previous_view_proj = view_proj;

        let histories = &self.history.as_ref().unwrap().2;
        let (read, write) = (&histories[self.current], &histories[1 - self.current]);
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("taa"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.targets.hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(read),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&gpu.targets.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&gpu.targets.motion),
                },
            ],
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("taa"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: write,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.current = 1 - self.current;
        self.valid = true;
        write
    }

    /// Forgets the history, as after a cut to a new view.
    pub(crate) fn reset(&mut self) {
        self.valid = false;
    }
}
