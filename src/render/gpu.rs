use std::sync::Arc;

use anyhow::Context;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The scene is lit and blended in linear HDR, then tone mapped onto the swapchain.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const MSAA_SAMPLES: u32 = 4;
/// Depth is reversed (near = 1, far = 0) for precision far from the camera, so nearer
/// fragments have *greater* depth. Pipelines drawing into the main pass should use this.
pub const DEPTH_COMPARE: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;
pub const DEPTH_CLEAR: f32 = 0.0;

/// Depth-stencil state for an ordinary opaque pipeline in the main pass.
pub fn main_depth_state(write: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: write,
        depth_compare: DEPTH_COMPARE,
        stencil: Default::default(),
        bias: Default::default(),
    }
}

/// Multisample state for pipelines drawing into the main pass. `alpha_to_coverage` gives
/// alpha-tested foliage soft, anti-aliased edges.
pub fn main_multisample(alpha_to_coverage: bool) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: MSAA_SAMPLES,
        mask: !0,
        alpha_to_coverage_enabled: alpha_to_coverage,
    }
}

/// The wgpu device and surface, and the render targets every frame draws into.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub targets: Targets,
    /// Hardware ray queries are available and enabled.
    pub ray_tracing: bool,
}

/// Size-dependent textures, recreated on resize.
pub struct Targets {
    pub depth: wgpu::TextureView,
    /// Multisampled HDR color, resolved into `hdr` at the end of the main pass.
    pub hdr_msaa: wgpu::TextureView,
    pub hdr: wgpu::TextureView,
}

impl Gpu {
    /// Opens the GPU. With `ray_tracing`, prefers a Vulkan adapter with hardware ray queries,
    /// and quietly falls back to ordinary rendering where there isn't one.
    pub async fn new(window: Arc<winit::window::Window>, vsync: bool, ray_tracing: bool) -> anyhow::Result<Self> {
        let size = window.inner_size();
        // Ray queries, and an array of every texture so reflections can show what they hit.
        let rt_features = wgpu::Features::EXPERIMENTAL_RAY_QUERY
            | wgpu::Features::EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE
            | wgpu::Features::TEXTURE_BINDING_ARRAY
            | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING;
        let open = |backends: wgpu::Backends| {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                ..Default::default()
            });
            let surface = instance.create_surface(window.clone());
            (instance, surface)
        };
        let pick = |instance: &wgpu::Instance, surface: &wgpu::Surface<'static>| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: false,
            }))
            .ok()
        };
        let mut chosen = None;
        if ray_tracing {
            // Ray queries exist only on Vulkan for now.
            let (instance, surface) = open(wgpu::Backends::VULKAN);
            if let Ok(surface) = surface {
                if let Some(adapter) = pick(&instance, &surface) {
                    if adapter.features().contains(rt_features) {
                        chosen = Some((surface, adapter, true));
                    } else {
                        log::info!("{:?} can't trace rays; using shadow maps only", adapter.get_info().name);
                    }
                }
            }
        }
        let (surface, adapter, traced) = match chosen {
            Some(c) => c,
            None => {
                let (instance, surface) = open(wgpu::Backends::all());
                let surface = surface?;
                let adapter = pick(&instance, &surface).context("no compatible GPU adapter")?;
                (surface, adapter, false)
            }
        };
        let info = adapter.get_info();
        log::info!(
            "using adapter {:?} ({:?}){}",
            info.name,
            info.backend,
            if traced { " with ray tracing" } else { "" }
        );

        let available = adapter.limits();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("voxl device"),
                required_features: if traced { rt_features } else { wgpu::Features::empty() },
                required_limits: wgpu::Limits {
                    // Big merged scenery meshes.
                    max_buffer_size: available.max_buffer_size,
                    max_blas_primitive_count: if traced { available.max_blas_primitive_count } else { 0 },
                    max_blas_geometry_count: if traced { available.max_blas_geometry_count } else { 0 },
                    max_tlas_instance_count: if traced { available.max_tlas_instance_count } else { 0 },
                    max_acceleration_structures_per_shader_stage: if traced {
                        available.max_acceleration_structures_per_shader_stage
                    } else {
                        0
                    },
                    max_binding_array_elements_per_shader_stage: if traced {
                        available.max_binding_array_elements_per_shader_stage
                    } else {
                        0
                    },
                    ..Default::default()
                },
                ..Default::default()
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            // COPY_SRC lets screenshots read the frame back, where the platform allows it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | (caps.usages & wgpu::TextureUsages::COPY_SRC),
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let targets = Targets::new(&device, &config);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            targets,
            ray_tracing: traced,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.reconfigure();
    }

    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
        self.targets = Targets::new(&self.device, &self.config);
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }
}

impl Targets {
    fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let texture = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: config.width,
                        height: config.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        Self {
            // Read after the main pass, by the light shafts and TAA.
            depth: texture("depth", DEPTH_FORMAT, MSAA_SAMPLES, attachment | wgpu::TextureUsages::TEXTURE_BINDING),
            hdr_msaa: texture("hdr msaa", HDR_FORMAT, MSAA_SAMPLES, attachment),
            hdr: texture(
                "hdr",
                HDR_FORMAT,
                1,
                attachment | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
        }
    }
}

impl Gpu {
    /// The shared lighting code for main-pass shaders, ray traced where the GPU allows. Prepend
    /// it to a custom shader's source.
    pub fn pbr_wgsl(&self) -> &'static str {
        if self.ray_tracing {
            super::PBR_RT_WGSL
        } else {
            super::PBR_WGSL
        }
    }
}
