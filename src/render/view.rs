use super::{environment::GpuEnvironment, gpu::Gpu, raytrace::{RayTracing, TEXTURE_SLOTS}, shadow::ShadowMaps, RenderFrame};

/// Must match `View` in `pbr.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
    inverse_view_proj: [[f32; 4]; 4],
    camera_position: [f32; 4],
    camera_forward: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    fog: [f32; 4],
    sh: [[f32; 4]; 9],
    cascades: [[[f32; 4]; 4]; 4],
    cascade_splits: [f32; 4],
    cascade_texels: [f32; 4],
    shadow_params: [f32; 4],
    viewport: [f32; 4],
    probe_origin: [f32; 4],
    probe_dims: [f32; 4],
    temporal: [f32; 4],
}

/// Camera, sun, sky and shadow data shared by every pipeline in the main pass. Always bound at
/// group 0; the matching WGSL is `render::PBR_WGSL`.
pub struct ViewBinding {
    /// Which version of the ray-traced scene's bindings the bind group holds.
    pub(crate) rt_generation: u64,
    buffer: wgpu::Buffer,
    sky_sampler: wgpu::Sampler,
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl ViewBinding {
    /// `scene` is the ray-traced scene, bound at 5 when the GPU traces rays.
    pub(crate) fn new(gpu: &Gpu, shadows: &ShadowMaps, sky: &GpuEnvironment, scene: Option<&RayTracing>) -> Self {
        let device = &gpu.device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view uniform"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Compute too, for the light probe bake.
        let visible = wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: visible,
            ty,
            count: None,
        };
        let mut entries = vec![
            entry(
                0,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                1,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            ),
            entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
            entry(
                3,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            entry(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        ];
        if scene.is_some() {
            let fragment = |binding, ty, count| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty,
                count,
            };
            let probe_texture = || wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            };
            let read_only = || wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            };
            entries.extend([
                fragment(5, wgpu::BindingType::AccelerationStructure { vertex_return: false }, None),
                fragment(6, read_only(), None),
                fragment(7, read_only(), None),
                fragment(8, read_only(), None),
                fragment(9, read_only(), None),
                fragment(
                    10,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    std::num::NonZeroU32::new(TEXTURE_SLOTS),
                ),
                fragment(11, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), None),
                fragment(12, probe_texture(), None),
                fragment(13, probe_texture(), None),
                fragment(14, probe_texture(), None),
                fragment(15, probe_texture(), None),
            ]);
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view layout"),
            entries: &entries,
        });
        let sky_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let bind_group = Self::bind(device, &layout, &buffer, shadows, sky, &sky_sampler, scene);
        Self {
            rt_generation: scene.map_or(0, |rt| rt.generation),
            buffer,
            sky_sampler,
            layout,
            bind_group,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        shadows: &ShadowMaps,
        sky: &GpuEnvironment,
        sky_sampler: &wgpu::Sampler,
        scene: Option<&RayTracing>,
    ) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&shadows.array_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&shadows.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&sky.view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(sky_sampler),
            },
        ];
        let views = scene.map(|rt| rt.texture_views()).unwrap_or_default();
        if let Some(rt) = scene {
            entries.extend([
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: rt.tlas.as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: rt.vertex_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: rt.index_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: rt.range_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: rt.instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureViewArray(&views),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(&rt.sampler),
                },
            ]);
            for (i, probes) in rt.probes.views().iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: 12 + i as u32,
                    resource: wgpu::BindingResource::TextureView(probes),
                });
            }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view bind group"),
            layout,
            entries: &entries,
        })
    }

    /// Points the bind group at a new sky texture.
    pub(crate) fn rebind(&mut self, gpu: &Gpu, shadows: &ShadowMaps, sky: &GpuEnvironment, scene: Option<&RayTracing>) {
        self.bind_group = Self::bind(&gpu.device, &self.layout, &self.buffer, shadows, sky, &self.sky_sampler, scene);
    }


    pub(crate) fn write(&self, gpu: &Gpu, frame: &RenderFrame, sky_mips: u32, shadows: &ShadowMaps, probes: [[f32; 4]; 2]) {
        let v4 = |v: glam::Vec3, w: f32| [v.x, v.y, v.z, w];
        let mut sh = [[0.0; 4]; 9];
        for (dst, src) in sh.iter_mut().zip(frame.sh) {
            *dst = v4(src, 0.0);
        }
        let cascades = &frame.cascades;
        let (w, h) = (gpu.config.width as f32, gpu.config.height as f32);
        let view = ViewUniform {
            view_proj: frame.view_proj.to_cols_array_2d(),
            inverse_view_proj: frame.view_proj.inverse().to_cols_array_2d(),
            camera_position: v4(frame.camera_position, frame.time),
            camera_forward: v4(frame.camera_forward, frame.exposure),
            sun_direction: v4(frame.sun_direction, frame.sky_rotation),
            sun_color: v4(frame.sun_color, frame.sky_intensity),
            ambient: v4(frame.ambient_color, frame.fog.start),
            fog: [
                frame.fog.density,
                frame.fog.height_falloff,
                frame.fog.base_height,
                sky_mips as f32,
            ],
            sh,
            cascades: cascades.matrices.map(|m| m.to_cols_array_2d()),
            cascade_splits: cascades.splits,
            cascade_texels: cascades.texel_sizes,
            shadow_params: [
                1.0 / shadows.resolution as f32,
                if frame.shadows { 1.0 } else { 0.0 },
                0.0,
                cascades.splits[3] * 0.8,
            ],
            viewport: [w, h, 1.0 / w, 1.0 / h],
            probe_origin: probes[0],
            probe_dims: probes[1],
            temporal: [
                (frame.frame_index % 1024) as f32,
                if frame.temporal { 1.0 } else { 0.0 },
                frame.jitter.x,
                frame.jitter.y,
            ],
        };
        gpu.queue
            .write_buffer(&self.buffer, 0, bytemuck::bytes_of(&view));
    }
}
