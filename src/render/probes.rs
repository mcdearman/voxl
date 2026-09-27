//! Light probes: the light arriving from all around, measured on a grid through the scene.
//!
//! Tracing the light from every direction at every pixel is too slow and too noisy without a
//! denoiser. Instead each probe traces a few hundred rays once, when the scene settles, and
//! keeps what it saw as a linear spherical harmonic (a mean colour, and how the light leans
//! along each axis). Surfaces then blend the eight probes around them.
//!
//! The bake runs twice: first the probes see sunlit and skylit surfaces, then surfaces lit by
//! the probes themselves, so light bounces twice: the glow of a sunlit wall reaches the
//! shaded side of the street and comes back again.
//!
//! Probes inside walls see mostly the backs of surfaces; they mark themselves invalid, and
//! surfaces also give less weight to probes behind them, so light doesn't leak through walls.

use glam::Vec3;

use super::gpu::Gpu;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Probe counts along each axis are kept below this.
const MAX_PROBES_PER_AXIS: u32 = 256;
/// Slices of probes (along z) baked each frame.
const SLICES_PER_FRAME: u32 = 8;

/// Where to place light probes; set as `RayTracingSettings::probes`.
#[derive(Clone, Copy, Debug)]
pub struct ProbeGrid {
    /// The first probe.
    pub min: Vec3,
    /// The grid reaches at least this far.
    pub max: Vec3,
    /// Metres between probes.
    pub spacing: f32,
    /// Rays each probe traces per bounce.
    pub rays: u32,
}

impl ProbeGrid {
    pub fn new(min: Vec3, max: Vec3, spacing: f32) -> Self {
        Self {
            min,
            max,
            spacing,
            rays: 256,
        }
    }

    fn dims(&self) -> [u32; 3] {
        let n = ((self.max - self.min) / self.spacing).ceil();
        [n.x, n.y, n.z].map(|n| (n.max(0.0) as u32 + 1).min(MAX_PROBES_PER_AXIS))
    }
}

/// Four 3D textures: the mean light (w: the probe's validity) and its lean along x, y and z.
struct ProbeTextures {
    textures: [wgpu::Texture; 4],
    views: [wgpu::TextureView; 4],
}

impl ProbeTextures {
    fn new(device: &wgpu::Device, dims: [u32; 3], usage: wgpu::TextureUsages, label: &str) -> Self {
        let textures = [0, 1, 2, 3].map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: dims[0],
                    height: dims[1],
                    depth_or_array_layers: dims[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
        });
        let views = [0, 1, 2, 3].map(|i| textures[i].create_view(&Default::default()));
        Self { textures, views }
    }
}

/// The probes shaders read, and the scratch copy a bake writes.
pub(crate) struct ProbeVolume {
    grid: Option<ProbeGrid>,
    dims: [u32; 3],
    read: ProbeTextures,
    write: ProbeTextures,
    baked: bool,
    /// The scene as it was last frame, and as it was at the last bake.
    seen: Option<(u64, usize)>,
    baked_scene: Option<(u64, usize)>,
    /// A bake under way: the scene, the bounce, and the next slice of probes along z.
    progress: Option<((u64, usize), u32, u32)>,
}

impl ProbeVolume {
    pub(crate) fn new(gpu: &Gpu, grid: Option<ProbeGrid>) -> Self {
        let dims = grid.map_or([1; 3], |g| g.dims());
        let device = &gpu.device;
        let read = ProbeTextures::new(
            device,
            dims,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "light probes",
        );
        let write = ProbeTextures::new(
            device,
            dims,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            "light probe bake",
        );
        if let Some(g) = grid {
            log::info!(
                "light probes: {}×{}×{} at {} m",
                dims[0],
                dims[1],
                dims[2],
                g.spacing
            );
        }
        Self {
            grid,
            dims,
            read,
            write,
            baked: false,
            seen: None,
            baked_scene: None,
            progress: None,
        }
    }

    pub(crate) fn views(&self) -> &[wgpu::TextureView; 4] {
        &self.read.views
    }

    /// For the view uniform: the first probe and spacing, then the counts and whether the
    /// probes hold a finished bake.
    pub(crate) fn uniform(&self) -> [[f32; 4]; 2] {
        let (min, spacing) = self.grid.map_or((Vec3::ZERO, 1.0), |g| (g.min, g.spacing));
        let [x, y, z] = self.dims.map(|d| d as f32);
        [
            [min.x, min.y, min.z, spacing],
            [x, y, z, if self.baked { 1.0 } else { 0.0 }],
        ]
    }

    /// Whether to bake this frame: once the scene has held still for a frame, and differs
    /// from the one last baked. A bake runs a few slices of probes a frame, so no one frame
    /// stalls (or trips the operating system's GPU watchdog).
    pub(crate) fn wants_bake(&mut self, scene: (u64, usize)) -> bool {
        if self.grid.is_none() {
            return false;
        }
        let settled = self.seen == Some(scene);
        self.seen = Some(scene);
        if !settled {
            return false;
        }
        match self.progress {
            Some((s, ..)) if s == scene => true,
            _ if self.baked_scene == Some(scene) => false,
            _ => {
                self.progress = Some((scene, 0, 0));
                true
            }
        }
    }
}

/// Must match `Bake` in `probes.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BakeUniform {
    origin: [f32; 4],
    dims: [u32; 4],
    bounce: [u32; 4],
}

pub(crate) struct ProbeBaker {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
}

impl ProbeBaker {
    pub(crate) fn new(gpu: &Gpu, view_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gpu.device;
        let source = format!("{}{}", gpu.pbr_wgsl(), include_str!("probes.wgsl"));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe bake"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FORMAT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe bake layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1),
                storage(2),
                storage(3),
                storage(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe bake"),
            bind_group_layouts: &[view_layout, &layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("probe bake"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("bake_probes"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe bake params"),
            size: std::mem::size_of::<BakeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            params,
        }
    }

    /// Records the next few slices of a bake; at the end of each bounce, copies the result
    /// to where shaders read it.
    pub(crate) fn bake(&self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, view: &wgpu::BindGroup, volume: &mut ProbeVolume) {
        let (Some(grid), Some((scene, bounce, first))) = (volume.grid, volume.progress) else {
            return;
        };
        let [x, y, z] = volume.dims;
        let size = wgpu::Extent3d {
            width: x,
            height: y,
            depth_or_array_layers: z,
        };
        let last = (first + SLICES_PER_FRAME).min(z);
        let params = &self.params;
        {
            let uniform = BakeUniform {
                origin: [grid.min.x, grid.min.y, grid.min.z, grid.spacing],
                dims: [x, y, z, grid.rays],
                bounce: [bounce, first, 0, 0],
            };
            gpu.queue.write_buffer(params, 0, bytemuck::bytes_of(&uniform));
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }];
            for (i, view) in volume.write.views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: i as u32 + 1,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe bake"),
                layout: &self.layout,
                entries: &entries,
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("probe bake"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, view, &[]);
                pass.set_bind_group(1, &bind_group, &[]);
                pass.dispatch_workgroups(x.div_ceil(4), y.div_ceil(4), (last - first).div_ceil(4));
            }
        }
        if last < z {
            volume.progress = Some((scene, bounce, last));
            return;
        }
        // The next bounce, and then the frame, read what this one wrote.
        for (from, to) in volume.write.textures.iter().zip(&volume.read.textures) {
            encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), size);
        }
        if bounce == 0 {
            volume.progress = Some((scene, 1, 0));
        } else {
            volume.progress = None;
            volume.baked = true;
            volume.baked_scene = Some(scene);
            log::info!("baked {} light probes", x * y * z);
        }
    }
}
