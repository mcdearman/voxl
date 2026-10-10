//! Bloom and tone mapping: from the HDR scene to the swapchain.

use super::{
    gpu::{Gpu, HDR_FORMAT},
    Color,
};

const BLOOM_LEVELS: u32 = 6;

/// How the HDR image becomes the displayed one.
#[derive(Clone, Copy, Debug, crate::reflect::Reflect)]
#[reflect(name = "mira.PostProcess", default)]
pub struct PostProcess {
    /// Multiplies the scene's light before the tone curve. Photographic sky images are
    /// relative, so each scene picks its own.
    pub exposure: f32,
    /// How much of the blurred light is added back.
    pub bloom: f32,
    /// Corner darkening, 0 for none.
    pub vignette: f32,
    pub saturation: f32,
    /// Contrast of the tone curve: 1 is plain AgX, which is flat like log footage; around 1.3
    /// looks like a finished photograph.
    pub contrast: f32,
    /// Temporal anti-aliasing (see `taa.rs`).
    pub taa: bool,
    /// How much to sharpen the final image, 0 for none; a little restores what TAA softens.
    pub sharpen: f32,
    /// Colour grading: tints for the shadows and the highlights of the finished image.
    pub shadow_tint: Color,
    pub highlight_tint: Color,
    /// White balance: positive warms the image, negative cools it.
    pub temperature: f32,
    /// Film grain, 0 for none, 1 for heavy.
    pub grain: f32,
}

impl Default for PostProcess {
    fn default() -> Self {
        Self {
            // The defaults suit a plain scene under the built-in sky and a sun of the default
            // strength. With exposure 1 and the flat curve such a scene comes out pale and
            // washed, and much below 0.7 the sky goes dark; scenes lit by a photographed sky
            // set their own.
            exposure: 0.7,
            bloom: 0.04,
            vignette: 0.3,
            saturation: 1.0,
            contrast: 1.35,
            taa: true,
            sharpen: 0.25,
            shadow_tint: Color::WHITE,
            highlight_tint: Color::WHITE,
            temperature: 0.0,
            grain: 0.0,
        }
    }
}

pub(crate) struct PostRenderer {
    sampler: wgpu::Sampler,
    source_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    downsample_first: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    upsample: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bloom: Option<(u32, u32, Vec<wgpu::TextureView>)>,
}

impl PostRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let bloom_shader = crate::shader!("bloom.wgsl").module(device);
        let tonemap_shader = crate::shader!("tonemap.wgsl").module(device);
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post source layout"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                sampler_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        let pipeline = |label, layout: &wgpu::BindGroupLayout, module, entry, format, blend| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        Self {
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("post sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            downsample_first: pipeline("bloom first", &source_layout, &bloom_shader, "fs_downsample_first", HDR_FORMAT, None),
            downsample: pipeline("bloom down", &source_layout, &bloom_shader, "fs_downsample", HDR_FORMAT, None),
            upsample: pipeline("bloom up", &source_layout, &bloom_shader, "fs_upsample", HDR_FORMAT, Some(additive)),
            tonemap: pipeline("tonemap", &tonemap_layout, &tonemap_shader, "fs_main", gpu.config.format, None),
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post uniform"),
                size: 80,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            source_layout,
            tonemap_layout,
            bloom: None,
        }
    }

    fn bloom_views(&mut self, gpu: &Gpu) -> &[wgpu::TextureView] {
        let (w, h) = (gpu.config.width, gpu.config.height);
        if !matches!(&self.bloom, Some((bw, bh, _)) if *bw == w && *bh == h) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bloom"),
                size: wgpu::Extent3d {
                    width: (w / 2).max(1),
                    height: (h / 2).max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: BLOOM_LEVELS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let views = (0..BLOOM_LEVELS)
                .map(|level| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: level,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            self.bloom = Some((w, h, views));
        }
        &self.bloom.as_ref().unwrap().2
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        target: &wgpu::TextureView,
        settings: &PostProcess,
        time: f32,
        plain: bool,
    ) {
        let (s, h) = (settings.shadow_tint, settings.highlight_tint);
        gpu.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&[
                settings.exposure,
                settings.bloom,
                settings.vignette,
                settings.saturation,
                settings.contrast,
                1.0 + (settings.contrast - 1.0) * 0.8,
                if plain { 1.0 } else { 0.0 },
                0.0,
                s.r,
                s.g,
                s.b,
                0.0,
                h.r,
                h.g,
                h.b,
                0.0,
                settings.sharpen,
                settings.grain,
                time,
                settings.temperature,
            ]),
        );
        self.bloom_views(gpu);
        let views = &self.bloom.as_ref().unwrap().2;
        let source = |view: &wgpu::TextureView| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post source"),
                layout: &self.source_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    target: &wgpu::TextureView,
                    pipeline: &wgpu::RenderPipeline,
                    bind_group: &wgpu::BindGroup,
                    load: wgpu::LoadOp<wgpu::Color>| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);

        let hdr = source(scene);
        pass(encoder, &views[0], &self.downsample_first, &hdr, clear);
        for level in 1..views.len() {
            let bind = source(&views[level - 1]);
            pass(encoder, &views[level], &self.downsample, &bind, clear);
        }
        for level in (0..views.len() - 1).rev() {
            let bind = source(&views[level + 1]);
            pass(encoder, &views[level], &self.upsample, &bind, wgpu::LoadOp::Load);
        }

        let tonemap = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap"),
            layout: &self.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        pass(encoder, target, &self.tonemap, &tonemap, clear);
    }
}
