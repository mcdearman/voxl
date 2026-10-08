use std::{collections::HashMap, ops::Range};

use glam::{Mat3, Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::{
    gpu::{main_depth_state, main_multisample, Gpu, HDR_FORMAT},
    image::Image,
    mesh::{Mesh, Vertex},
    shadow::{shadow_depth_state, ShadowMaps},
    view::ViewBinding,
    Material, RenderFrame,
};
use crate::assets::Assets;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 3]; 3],
    color: [f32; 4],
    params: [f32; 4],
    emissive: [f32; 4],
    extra: [f32; 4],
    more: [f32; 4],
}

impl InstanceRaw {
    const ATTRIBUTES: [wgpu::VertexAttribute; 12] = wgpu::vertex_attr_array![
        3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        7 => Float32x3, 8 => Float32x3, 9 => Float32x3,
        10 => Float32x4, 12 => Float32x4, 13 => Float32x4, 14 => Float32x4, 15 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    fn new(model: Mat4, m: &Material) -> Self {
        // Inverse-transpose keeps normals perpendicular under non-uniform scale.
        let normal = Mat3::from_mat4(model).inverse().transpose();
        Self {
            model: model.to_cols_array_2d(),
            normal: normal.to_cols_array_2d(),
            color: m.color.to_array(),
            params: [
                m.roughness,
                m.metallic,
                if m.normal_texture.is_some() { m.normal_strength } else { 0.0 },
                m.alpha_cutoff.unwrap_or(0.0),
            ],
            emissive: [m.emissive.r, m.emissive.g, m.emissive.b, m.translucency],
            extra: [
                if m.height_texture.is_some() { m.height_scale } else { 0.0 },
                m.subsurface,
                m.weathering,
                if m.decal { 1.0 } else { 0.0 },
            ],
            more: [m.puddles, m.waves, m.flow.x, m.flow.y],
        }
    }
}

/// What a batch of instances shares: the mesh, its textures and its pipeline variant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) struct BatchKey {
    variant: u8,
    mesh: u32,
    textures: TextureSet,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
struct TextureSet {
    base_color: u32,
    normal: u32,
    metallic_roughness: u32,
    height: u32,
}

const NONE: u32 = u32::MAX;

impl BatchKey {
    pub(crate) fn new(mesh: u32, m: &Material) -> Self {
        let id = |h: Option<crate::assets::Handle<Image>>| h.map_or(NONE, |h| h.id());
        Self {
            // Decals sort after everything else, so they lie over what is already drawn.
            variant: m.alpha_cutoff.is_some() as u8 | (m.double_sided as u8) << 1 | (m.decal as u8) << 2,
            mesh,
            textures: TextureSet {
                base_color: id(m.base_color_texture),
                normal: id(m.normal_texture),
                metallic_roughness: id(m.metallic_roughness_texture),
                height: id(m.height_texture),
            },
        }
    }

    pub(crate) fn mesh(&self) -> u32 {
        self.mesh
    }

    fn masked(&self) -> bool {
        self.variant & 1 != 0
    }
}

struct GpuMesh {
    /// Bounding sphere in mesh space, for culling.
    center: Vec3,
    radius: f32,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct Batch {
    key: BatchKey,
    instances: Range<u32>,
}

/// Draws every `Mesh3d` with one instanced draw call per mesh and material.
pub struct MeshRenderer {
    /// Indexed by `BatchKey::variant`: bit 0 alpha-tested, bit 1 double-sided, bit 2 decal.
    pipelines: [wgpu::RenderPipeline; 8],
    shadow_opaque: wgpu::RenderPipeline,
    shadow_masked: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    defaults: [wgpu::TextureView; 4],
    images: HashMap<u32, wgpu::TextureView>,
    materials: HashMap<TextureSet, wgpu::BindGroup>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    meshes: HashMap<u32, GpuMesh>,
    instances: Vec<InstanceRaw>,
    batches: Vec<Batch>,
    shadow_batches: Vec<Batch>,
}

impl MeshRenderer {
    pub fn new(gpu: &Gpu, view: &ViewBinding, shadows: &ShadowMaps) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh shader"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", gpu.pbr_wgsl(), crate::shader!("mesh.wgsl").source()).into()),
        });
        let shadow_shader = crate::shader!("mesh_shadow.wgsl").module(device);

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
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[&view.layout, &material_layout],
            push_constant_ranges: &[],
        });
        let pipeline = |masked: bool, double_sided: bool, decal: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                        format: HDR_FORMAT,
                        blend: decal.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: (!double_sided).then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(if decal {
                    // Tested against the surface beneath but not written, and pulled a little
                    // toward the eye (depth is reversed) so it never flickers through it.
                    wgpu::DepthStencilState {
                        bias: wgpu::DepthBiasState {
                            constant: 8,
                            slope_scale: 1.5,
                            clamp: 0.0,
                        },
                        ..main_depth_state(false)
                    }
                } else {
                    main_depth_state(true)
                }),
                multisample: main_multisample(masked && !decal),
                multiview: None,
                cache: None,
            })
        };
        let pipelines = std::array::from_fn(|variant| pipeline(variant & 1 != 0, variant & 2 != 0, variant & 4 != 0));

        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh shadow layout"),
            bind_group_layouts: &[&shadows.layout, &material_layout],
            push_constant_ranges: &[],
        });
        let shadow_pipeline = |masked: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh shadow pipeline"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::layout(), InstanceRaw::layout()],
                    compilation_options: Default::default(),
                },
                fragment: masked.then(|| wgpu::FragmentState {
                    module: &shadow_shader,
                    entry_point: Some("fs_mask"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                // Both faces cast, so thin and open geometry still blocks the sun.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(shadow_depth_state()),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let solid = |rgba, srgb| Image::solid(rgba, srgb).upload(device, &gpu.queue);
        let defaults = [
            solid([255, 255, 255, 255], true),
            solid([128, 128, 255, 255], false),
            solid([255, 255, 255, 255], false),
            solid([0, 0, 0, 255], false),
        ];

        let instance_capacity = 64;
        Self {
            pipelines,
            shadow_opaque: shadow_pipeline(false),
            shadow_masked: shadow_pipeline(true),
            material_layout,
            sampler,
            defaults,
            images: HashMap::new(),
            materials: HashMap::new(),
            instance_buffer: create_instance_buffer(device, instance_capacity),
            instance_capacity,
            meshes: HashMap::new(),
            instances: Vec::new(),
            batches: Vec::new(),
            shadow_batches: Vec::new(),
        }
    }

    /// Takes the pipelines of a renderer freshly built from the current shaders, keeping
    /// this one's meshes, textures and materials.
    pub(crate) fn adopt_pipelines(&mut self, fresh: Self) {
        self.pipelines = fresh.pipelines;
        self.shadow_opaque = fresh.shadow_opaque;
        self.shadow_masked = fresh.shadow_masked;
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
            if mesh.indices.is_empty() {
                self.meshes.remove(&id);
                continue;
            }
            // Skinning writes posed vertices into the buffer, and the ray-traced scene can build
            // from it directly.
            let traced = if gpu.ray_tracing { wgpu::BufferUsages::BLAS_INPUT } else { wgpu::BufferUsages::empty() };
            let vertices = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | traced,
                });
            let indices = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX | traced,
                });
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &mesh.vertices {
                lo = lo.min(Vec3::from(v.position));
                hi = hi.max(Vec3::from(v.position));
            }
            let center = (lo + hi) * 0.5;
            let radius = mesh
                .vertices
                .iter()
                .map(|v| Vec3::from(v.position).distance(center))
                .fold(0.0, f32::max);
            self.meshes.insert(
                id,
                GpuMesh {
                    center,
                    radius,
                    vertices,
                    indices,
                    index_count: mesh.indices.len() as u32,
                },
            );
        }
    }

    /// Uploads new images. Material bind groups that used a changed image are rebuilt.
    pub fn sync_images(&mut self, gpu: &Gpu, images: &mut Assets<Image>) {
        let (modified, removed) = images.take_changes();
        if modified.is_empty() && removed.is_empty() {
            return;
        }
        for id in &removed {
            self.images.remove(id);
        }
        // Mip chains take a while for big textures; make them on every core at once.
        let pending: Vec<(u32, &Image)> = modified.iter().filter_map(|id| images.get_by_id(*id).map(|image| (*id, image))).collect();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chains: Vec<Vec<(u32, u32, Vec<u8>)>> = std::thread::scope(|s| {
            let chunk = pending.len().div_ceil(threads).max(1);
            let jobs: Vec<_> = pending
                .chunks(chunk)
                .map(|part| s.spawn(move || part.iter().map(|(_, image)| image.mip_chain()).collect::<Vec<_>>()))
                .collect();
            jobs.into_iter().flat_map(|j| j.join().expect("mip generation panicked")).collect()
        });
        // Send the uploads in batches of their own, so no one submission keeps the GPU busy for
        // long (Windows resets a GPU that is unresponsive for two seconds).
        let mut batch = 0usize;
        for ((id, image), levels) in pending.iter().zip(&chains) {
            self.images.insert(*id, image.upload_levels(&gpu.device, &gpu.queue, levels));
            batch += levels.iter().map(|(_, _, data)| data.len()).sum::<usize>();
            if batch > 128 << 20 {
                gpu.queue.submit([]);
                batch = 0;
            }
        }
        gpu.queue.submit([]);
        let changed = |t: &TextureSet| {
            [t.base_color, t.normal, t.metallic_roughness, t.height]
                .iter()
                .any(|id| modified.contains(id) || removed.contains(id))
        };
        self.materials.retain(|set, _| !changed(set));
    }

    fn material(&mut self, gpu: &Gpu, set: TextureSet) {
        if self.materials.contains_key(&set) {
            return;
        }
        let pick = |id: u32, slot: usize| {
            self.images.get(&id).unwrap_or(&self.defaults[slot])
        };
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(pick(set.base_color, 0)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(pick(set.normal, 1)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(pick(set.metallic_roughness, 2)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(pick(set.height, 3)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.materials.insert(set, bind_group);
    }

    /// The bounding sphere of an uploaded mesh, in mesh space.
    pub fn bounds(&self, mesh: u32) -> Option<(Vec3, f32)> {
        self.meshes.get(&mesh).map(|m| (m.center, m.radius))
    }

    /// Sorts objects so each mesh and material becomes one contiguous range of instances,
    /// once for the camera's view and once for the shadow cascades.
    pub fn build_batches(&mut self, gpu: &Gpu, frame: &mut RenderFrame) {
        frame.objects.sort_unstable_by_key(|o| o.key);
        self.instances.clear();
        self.batches.clear();
        self.shadow_batches.clear();
        for (flag, batches) in [(VISIBLE, &mut self.batches), (CASTS_SHADOW, &mut self.shadow_batches)] {
            for object in frame.objects.iter().filter(|o| o.flags & flag != 0) {
                let i = self.instances.len() as u32;
                match batches.last_mut() {
                    Some(batch) if batch.key == object.key => batch.instances.end = i + 1,
                    _ => batches.push(Batch {
                        key: object.key,
                        instances: i..i + 1,
                    }),
                }
                self.instances.push(InstanceRaw::new(object.model, &object.material));
            }
        }
        let sets: Vec<_> = self
            .batches
            .iter()
            .chain(&self.shadow_batches)
            .map(|b| b.key.textures)
            .collect();
        for set in sets {
            self.material(gpu, set);
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

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_with(pass, &self.batches, |key| &self.pipelines[key.variant as usize]);
    }

    /// A mesh's vertex and index buffers on the GPU.
    pub(crate) fn vertex_buffer(&self, mesh: u32) -> Option<&wgpu::Buffer> {
        self.meshes.get(&mesh).map(|m| &m.vertices)
    }

    pub(crate) fn index_buffer(&self, mesh: u32) -> Option<&wgpu::Buffer> {
        self.meshes.get(&mesh).map(|m| &m.indices)
    }

    /// The uploaded texture for an image.
    pub(crate) fn image_view(&self, image: u32) -> Option<&wgpu::TextureView> {
        self.images.get(&image)
    }

    pub fn draw_shadows(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_with(pass, &self.shadow_batches, |key| {
            if key.masked() {
                &self.shadow_masked
            } else {
                &self.shadow_opaque
            }
        });
    }

    fn draw_with<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        batches: &[Batch],
        pipeline: impl Fn(&BatchKey) -> &'a wgpu::RenderPipeline,
    ) {
        if batches.is_empty() {
            return;
        }
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        let mut current: Option<*const wgpu::RenderPipeline> = None;
        for batch in batches {
            let Some(mesh) = self.meshes.get(&batch.key.mesh) else {
                continue;
            };
            let Some(material) = self.materials.get(&batch.key.textures) else {
                continue;
            };
            let p = pipeline(&batch.key);
            if current != Some(p as *const _) {
                pass.set_pipeline(p);
                current = Some(p as *const _);
            }
            pass.set_bind_group(1, material, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, batch.instances.clone());
        }
    }
}

/// Drawn in the camera's view.
pub(crate) const VISIBLE: u8 = 1;
/// Drawn into the shadow cascades.
pub(crate) const CASTS_SHADOW: u8 = 2;

/// One mesh instance to draw this frame.
pub(crate) struct RenderObject {
    pub key: BatchKey,
    pub model: Mat4,
    pub material: Material,
    pub flags: u8,
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * std::mem::size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
