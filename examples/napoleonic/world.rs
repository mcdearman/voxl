//! The ground and everything built on it, drawn with the photographic materials through a
//! pipeline of its own, plugged into the engine's main and shadow passes.

use mira::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::{
        main_depth_state, main_multisample, shadow_depth_state, DrawFunctions, Gpu,
        HitMaterial, RayTracing, ShadowDrawFunctions, ShadowMaps, TextureArray, ViewBinding, HDR_FORMAT,
    },
};
use wgpu::util::DeviceExt;

use crate::materials::Materials;

/// Marks a vertex as terrain, blending the ground layers by its weights.
pub const GROUND: u32 = 255;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Metres.
    pub uv: [f32; 2],
    /// Ground layer weights: grass, straw, soil, road, forest floor, mud; then two unused.
    pub weights: [u8; 8],
    /// rgb: colour multiplier where 128 is neutral; a: occlusion (0 none, 255 black).
    pub tint: [u8; 4],
    pub layer: u32,
}

impl WorldVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4, 4 => Unorm8x4,
        5 => Unorm8x4, 6 => Uint32
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Packs a colour multiplier (1 is neutral, up to 2) and an occlusion amount.
pub fn tint(color: Vec3, occlusion: f32) -> [u8; 4] {
    let c = |v: f32| (v * 127.5).round().clamp(0.0, 255.0) as u8;
    [c(color.x), c(color.y), c(color.z), (occlusion * 255.0).clamp(0.0, 255.0) as u8]
}

pub const NEUTRAL: [u8; 4] = [128, 128, 128, 0];

#[derive(Clone, Default)]
pub struct WorldMesh {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
}

impl WorldMesh {
    /// Appends an engine mesh as one material. UVs are projected in the part's own space along
    /// whichever axis each face most nearly faces, in metres, so textures keep their real size
    /// however the part is stretched; `place` then puts the part in the world.
    pub fn add(&mut self, shape: &Mesh, part: Mat4, place: Mat4, layer: u32, tint: [u8; 4]) {
        let base = self.vertices.len() as u32;
        let part_normals = Mat3::from_mat4(part).inverse().transpose();
        let world = place * part;
        let world_normals = Mat3::from_mat4(world).inverse().transpose();
        for v in &shape.vertices {
            let local = part.transform_point3(Vec3::from(v.position));
            let n_local = (part_normals * Vec3::from(v.normal)).normalize_or_zero();
            let a = n_local.abs();
            let uv = if a.y >= a.x && a.y >= a.z {
                Vec2::new(local.x, local.z)
            } else if a.x >= a.z {
                Vec2::new(local.z * n_local.x.signum(), -local.y)
            } else {
                Vec2::new(-local.x * n_local.z.signum(), -local.y)
            };
            self.vertices.push(WorldVertex {
                position: world.transform_point3(Vec3::from(v.position)).into(),
                normal: (world_normals * Vec3::from(v.normal)).normalize_or_zero().into(),
                uv: uv.into(),
                weights: [0; 8],
                tint,
                layer,
            });
        }
        let mirrored = world.determinant() < 0.0;
        for t in shape.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| base + i);
            self.indices.extend(if mirrored { [a, c, b] } else { [a, b, c] });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The land: receives shadows but is too big to be worth casting them.
    Ground,
    /// Buildings, walls and the bridge.
    Solid,
    Water,
}

/// Geometry for the world pipeline, put in place by the scene before the renderer starts.
#[derive(Default)]
pub struct WorldGeometry(pub Vec<(Kind, WorldMesh)>);

struct Draw {
    kind: Kind,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

pub struct WorldRenderer {
    pipeline: wgpu::RenderPipeline,
    water: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    draws: Vec<Draw>,
}

pub fn plugin(app: &mut App) {
    app.world.resource_mut::<DrawFunctions>().0.push(draw);
    app.world.resource_mut::<ShadowDrawFunctions>().0.push(draw_shadows);
}

/// Uploads the materials and the geometry. Runs once, after the scene is laid out.
pub fn init(world: &mut World) {
    let started = std::time::Instant::now();
    let materials = crate::materials::load_all().expect("failed to load the materials");
    let gpu = world.resource::<Gpu>();
    let renderer = WorldRenderer::new(
        gpu,
        world.resource::<ViewBinding>(),
        world.resource::<ShadowMaps>(),
        &materials,
        &world.resource::<WorldGeometry>().0,
    );
    log::info!("world materials ready in {:.1?}", started.elapsed());
    world.insert_resource(renderer);
    trace_world(world);
    world.remove_resource::<WorldGeometry>();
}

impl WorldRenderer {
    fn new(
        gpu: &Gpu,
        view: &ViewBinding,
        shadows: &ShadowMaps,
        materials: &Materials,
        geometry: &[(Kind, WorldMesh)],
    ) -> Self {
        let device = &gpu.device;
        let albedo = TextureArray::from_images(gpu, &materials.albedo, true);
        let normals = TextureArray::from_images(gpu, &materials.normal_roughness, false);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world layers"),
            contents: bytemuck::cast_slice(&materials.params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world materials"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world materials"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&albedo.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&normals.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&albedo.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world shader"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", gpu.pbr_wgsl(), include_str!("world.wgsl")).into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world layout"),
            bind_group_layouts: &[&view.layout, &layout],
            push_constant_ranges: &[],
        });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("world pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[WorldVertex::layout()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(main_depth_state(true)),
                multisample: main_multisample(false),
                multiview: None,
                cache: None,
            })
        };

        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world shadow shader"),
            source: wgpu::ShaderSource::Wgsl(
                "@group(0) @binding(0) var<uniform> light_view_proj: mat4x4<f32>;
                 @vertex fn vs_main(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
                     return light_view_proj * vec4<f32>(p, 1.0);
                 }"
                .into(),
            ),
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world shadow layout"),
            bind_group_layouts: &[&shadows.layout],
            push_constant_ranges: &[],
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("world shadow pipeline"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                buffers: &[WorldVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: Some(shadow_depth_state()),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let draws = geometry
            .iter()
            .filter(|(_, mesh)| !mesh.indices.is_empty())
            .map(|(kind, mesh)| Draw {
                kind: *kind,
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("world vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("world indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                count: mesh.indices.len() as u32,
            })
            .collect();
        Self {
            pipeline: pipeline("fs_main"),
            water: pipeline("fs_water"),
            shadow,
            bind_group,
            draws,
        }
    }
}

fn draw(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    let Some(r) = world.get_resource::<WorldRenderer>() else {
        return;
    };
    pass.set_bind_group(1, &r.bind_group, &[]);
    for d in &r.draws {
        pass.set_pipeline(if d.kind == Kind::Water { &r.water } else { &r.pipeline });
        pass.set_vertex_buffer(0, d.vertices.slice(..));
        pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..d.count, 0, 0..1);
    }
}

fn draw_shadows(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    let Some(r) = world.get_resource::<WorldRenderer>() else {
        return;
    };
    pass.set_pipeline(&r.shadow);
    for d in r.draws.iter().filter(|d| d.kind == Kind::Solid) {
        pass.set_vertex_buffer(0, d.vertices.slice(..));
        pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..d.count, 0, 0..1);
    }
}

/// Puts the land and buildings into the ray-traced scene, if there is one.
fn trace_world(world: &mut World) {
    if !world.contains_resource::<RayTracing>() {
        return;
    }
    world.resource_scope(|world, rt: &mut RayTracing| {
        let gpu = world.resource::<Gpu>();
        for (kind, mesh) in &world.resource::<WorldGeometry>().0 {
            // Roughly the colour each looks from a distance, for reflections.
            let color = match kind {
                Kind::Ground => Color::rgb(0.13, 0.13, 0.06),
                Kind::Solid => Color::rgb(0.35, 0.32, 0.28),
                Kind::Water => continue,
            };
            let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
            let normals: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.normal).collect();
            let uvs: Vec<[f32; 2]> = mesh.vertices.iter().map(|v| v.uv).collect();
            let geometry = rt.add_geometry(gpu, &positions, &normals, &uvs, &mesh.indices);
            rt.add_fixed(geometry, Mat4::IDENTITY, HitMaterial::plain(color));
        }
    });
}
