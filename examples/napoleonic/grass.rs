//! Grass, ripe cereal and wildflowers, grown on the GPU in tiles around the camera.
//!
//! Nothing is stored per blade. Each frame picks the ground tiles near the camera that are in
//! view, and the vertex shader places every blade from a hash of its tile and index, the
//! terrain heights, and a cover map saying what grows where.

use voxl::{
    glam::{Vec2, Vec3, Vec4},
    prelude::*,
    render::{main_depth_state, main_multisample, DrawFunctions, Gpu, RenderFrame, ViewBinding, HDR_FORMAT},
};
use wgpu::util::DeviceExt;

use crate::terrain::{self, Crop, Ground, DETAIL_EXTENT, DETAIL_STEP};

const TILE: f32 = 8.0;
const BLADES_PER_TILE: u32 = 36864;
const FADE_START: f32 = 48.0;
const FADE_END: f32 = 62.0;
/// Tiles closer than this get every blade; beyond, density falls with the square of distance.
const FULL_DENSITY: f32 = 16.0;
const MAX_TILES: usize = 512;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    grid: [f32; 4],
    extent: [f32; 4],
    wind: [f32; 4],
}

pub struct GrassRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    tiles: wgpu::Buffer,
    /// (tile slot, blades) for this frame.
    draws: Vec<(u32, u32)>,
}

pub fn plugin(app: &mut App) {
    app.add_systems(Stage::Prepare, prepare);
    app.world.resource_mut::<DrawFunctions>().0.push(draw);
}

/// What grows at a point: grass density, cereal density, dryness, grass height, each 0 to 1.
/// `trampled` is how much feet, hooves and wheels have flattened it.
pub fn cover(x: f32, z: f32, trampled: f32) -> [f32; 4] {
    let road = terrain::road_distance(x, z);
    let river = terrain::river_distance(x, z);
    if road < terrain::ROAD_HALF_WIDTH + 0.2 || river < 15.0 || terrain::height(x, z) < 0.3 {
        return [0.0; 4];
    }
    let (column, _) = terrain::field_cell(x, z);
    let f = terrain::field_space(x, z);
    // Headlands: a strip of grass left around each field where the plough turned.
    let along = (f.x / terrain::FIELD_WIDTH).fract() * terrain::FIELD_WIDTH;
    let offset = terrain::hash(5, column, 0.0) * terrain::FIELD_LENGTH;
    let down = ((f.y + offset) / terrain::FIELD_LENGTH).fract() * terrain::FIELD_LENGTH;
    let edge = along.min(terrain::FIELD_WIDTH - along).min(down).min(terrain::FIELD_LENGTH - down);
    let headland = edge < 1.8;

    let mut c = match terrain::crop(x, z) {
        _ if headland => [0.9, 0.0, 0.25, 0.8],
        Crop::Wheat => [0.05, 0.95, 0.9, 0.5],
        Crop::Barley => [0.05, 0.9, 1.0, 0.5],
        Crop::Oats => [0.3, 0.6, 0.6, 0.6],
        Crop::Meadow => [0.95, 0.0, 0.12, 1.0],
        Crop::Pasture => [0.9, 0.0, 0.25, 0.35],
        Crop::Ploughed => [0.03, 0.0, 0.5, 0.2],
        Crop::Stubble => [0.7, 0.0, 1.0, 0.12],
    };
    let village = (Vec2::new(x, z) - crate::scene::VILLAGE).length();
    let lush = terrain::smoothstep(90.0, 50.0, river).max(terrain::smoothstep(150.0, 110.0, village));
    if lush > 0.0 {
        let meadow = [0.95, 0.0, 0.1, 0.9];
        c = std::array::from_fn(|i| c[i] + (meadow[i] - c[i]) * lush);
    }
    if terrain::forest(x, z) > 0.0 {
        c = [0.25, 0.0, 0.3, 0.4];
    }
    // Verges beside the road, dusty and short.
    let verge = terrain::smoothstep(terrain::ROAD_HALF_WIDTH + 3.0, terrain::ROAD_HALF_WIDTH + 0.5, road);
    c[1] *= 1.0 - verge;
    c[0] = c[0].max(verge * 0.8);
    c[2] = c[2].max(verge * 0.5);
    c[3] *= 1.0 - verge * 0.6;

    // Trampled ground: cereal flattened away, grass short and bruised.
    c[1] *= 1.0 - trampled;
    c[0] = c[0].max(trampled * 0.7) * (1.0 - trampled * 0.3);
    c[3] *= 1.0 - trampled * 0.8;
    c[2] = c[2].max(trampled * 0.4);
    c
}

/// Uploads the terrain heights and the cover map, and builds the pipeline.
pub fn init(world: &mut World, ground: &Ground, trampled: &(dyn Fn(Vec2) -> f32 + Sync)) {
    let n = ground.samples;
    let cover_map: Vec<u8> = {
        let rows: Vec<Vec<u8>> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..n)
                .collect::<Vec<_>>()
                .chunks(n.div_ceil(16) as usize)
                .map(|rows| {
                    let rows = rows.to_vec();
                    s.spawn(move || {
                        let mut out = Vec::with_capacity(rows.len() * n as usize * 4);
                        for j in rows {
                            for i in 0..n {
                                let x = -DETAIL_EXTENT + i as f32 * DETAIL_STEP;
                                let z = -DETAIL_EXTENT + j as f32 * DETAIL_STEP;
                                let c = cover(x, z, trampled(Vec2::new(x, z)));
                                out.extend(c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8));
                            }
                        }
                        out
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        rows.concat()
    };

    let gpu = world.resource::<Gpu>();
    let device = &gpu.device;
    let size = wgpu::Extent3d {
        width: n,
        height: n,
        depth_or_array_layers: 1,
    };
    let heights = device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("grass heights"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        bytemuck::cast_slice(&ground.heights),
    );
    let cover = device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("grass cover"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        &cover_map,
    );
    let wind = Vec2::new(1.0, -0.45).normalize();
    let params = Params {
        grid: [TILE, BLADES_PER_TILE as f32, DETAIL_STEP, n as f32],
        extent: [-DETAIL_EXTENT, -DETAIL_EXTENT, FADE_START, FADE_END],
        wind: [wind.x, wind.y, 0.8, 0.0],
    };
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("grass params"),
        contents: bytemuck::bytes_of(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let tiles = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grass tiles"),
        size: (MAX_TILES * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty,
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grass layout"),
        entries: &[
            entry(0, wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
            }),
            entry(1, wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            }),
            entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            entry(3, wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            }),
            entry(4, wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            }),
        ],
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("grass cover sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grass"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&heights.create_view(&Default::default())),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&cover.create_view(&Default::default())),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: tiles.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params.as_entire_binding(),
            },
        ],
    });

    let view = world.resource::<ViewBinding>();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("grass shader"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", gpu.pbr_wgsl(), include_str!("grass.wgsl")).into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grass pipeline layout"),
        bind_group_layouts: &[&view.layout, &layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("grass pipeline"),
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
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(main_depth_state(true)),
        multisample: main_multisample(false),
        multiview: None,
        cache: None,
    });
    world.insert_resource(GrassRenderer {
        pipeline,
        bind_group,
        tiles,
        draws: Vec::new(),
    });
}

/// Picks the tiles in view near the camera, thinning blades with distance.
fn prepare(gpu: Res<Gpu>, frame: Res<RenderFrame>, grass: Option<ResMut<GrassRenderer>>) {
    let Some(mut grass) = grass else {
        return;
    };
    let camera = frame.camera_position;
    let planes = frustum_planes(frame.view_proj);
    let reach = (FADE_END / TILE).ceil() as i32 + 1;
    let origin = (Vec2::new(camera.x, camera.z) / TILE).floor();
    let mut tiles: Vec<(f32, [f32; 4])> = Vec::new();
    for j in -reach..=reach {
        for i in -reach..=reach {
            let corner = (origin + Vec2::new(i as f32, j as f32)) * TILE;
            if corner.abs().max_element() > DETAIL_EXTENT - TILE {
                continue;
            }
            let center2 = corner + TILE * 0.5;
            let distance = (center2 - Vec2::new(camera.x, camera.z)).length() - TILE * 0.7;
            if distance > FADE_END {
                continue;
            }
            let center = Vec3::new(center2.x, terrain::height(center2.x, center2.y) + 0.5, center2.y);
            let radius = TILE * 0.75 + 1.0;
            if planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius * p.truncate().length()) {
                continue;
            }
            let d = distance.max(0.0);
            let lod = ((FULL_DENSITY / d.max(FULL_DENSITY)).powi(2)).max(0.05);
            let widen = (1.0 / lod.sqrt()).min(3.5);
            tiles.push((d, [corner.x, corner.y, lod, widen]));
        }
    }
    tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    tiles.truncate(MAX_TILES);
    let data: Vec<[f32; 4]> = tiles.iter().map(|t| t.1).collect();
    if !data.is_empty() {
        gpu.queue.write_buffer(&grass.tiles, 0, bytemuck::cast_slice(&data));
    }
    grass.draws = tiles
        .iter()
        .enumerate()
        .map(|(slot, t)| (slot as u32, (t.1[2] * BLADES_PER_TILE as f32).ceil() as u32))
        .collect();
}

fn frustum_planes(view_proj: Mat4) -> [Vec4; 5] {
    let (x, y, w) = (view_proj.row(0), view_proj.row(1), view_proj.row(3));
    let z = view_proj.row(2);
    // Reversed Z: the near plane is z <= w.
    [w + x, w - x, w + y, w - y, w - z]
}

fn draw(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    let Some(grass) = world.get_resource::<GrassRenderer>() else {
        return;
    };
    if grass.draws.is_empty() {
        return;
    }
    pass.set_pipeline(&grass.pipeline);
    pass.set_bind_group(1, &grass.bind_group, &[]);
    for (slot, blades) in &grass.draws {
        let first = slot * BLADES_PER_TILE;
        pass.draw(0..9, first..first + blades);
    }
}
