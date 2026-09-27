//! Powder smoke, dust and woodsmoke as soft, lit, semi-transparent billboards; muzzle flashes
//! as brief additive ones.

use voxl::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::{
        main_depth_state, main_multisample, Gpu, Image, RenderFrame, TextureArray,
        TransparentDrawFunctions, ViewBinding, HDR_FORMAT,
    },
    voxel::fbm,
};

/// The breeze, which carries smoke north-east.
pub const WIND: Vec3 = Vec3::new(1.1, 0.0, -0.5);
const PUFF_SIZE: u32 = 256;
const PUFF_LAYERS: u32 = 4;
const MAX_PARTICLES: usize = 8192;

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub life: f32,
    /// Radius when born, and how many times bigger it spreads to.
    pub size: f32,
    pub spread: f32,
    pub color: Vec3,
    pub opacity: f32,
    /// Upward drift, metres per second: hot smoke climbs, cold powder smoke hangs.
    pub rise: f32,
    pub drag: f32,
    pub rotation: f32,
    pub spin: f32,
    pub layer: u32,
    pub flash: bool,
}

#[derive(Default)]
pub struct Smoke {
    particles: Vec<Particle>,
    seed: u32,
}

impl Smoke {
    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn emit(&mut self, particle: Particle) {
        if self.particles.len() < MAX_PARTICLES {
            self.particles.push(particle);
        }
    }

    fn base(&mut self, position: Vec3, velocity: Vec3) -> Particle {
        let rotation = self.random() * std::f32::consts::TAU;
        let layer = (self.random() * PUFF_LAYERS as f32) as u32;
        Particle {
            position,
            velocity,
            age: 0.0,
            life: 10.0,
            size: 0.5,
            spread: 4.0,
            color: Vec3::splat(0.8),
            opacity: 0.9,
            rise: 0.2,
            drag: 1.2,
            rotation,
            spin: (self.random() - 0.5) * 0.2,
            layer: layer.min(PUFF_LAYERS - 1),
            flash: false,
        }
    }

    /// The cloud a musket throws out: a jet from the muzzle and a puff from the pan.
    pub fn musket(&mut self, muzzle: Vec3, forward: Vec3) {
        for i in 0..5 {
            let r = self.random();
            let speed = 6.0 + i as f32 * 2.5 + r * 3.0;
            let jitter = Vec3::new(self.random() - 0.5, self.random() * 0.6, self.random() - 0.5) * 1.2;
            let mut p = self.base(muzzle + forward * (i as f32 * 0.15), forward * speed + jitter);
            p.size = 0.25 + r * 0.15;
            p.spread = 5.0 + r * 3.0;
            p.life = 14.0 + r * 8.0;
            p.color = Vec3::splat(0.78 + r * 0.08);
            p.opacity = 0.75;
            p.drag = 1.6;
            self.emit(p);
        }
        let pan = muzzle - forward * 1.3 + Vec3::new(0.0, 0.05, 0.0);
        let mut p = self.base(pan, Vec3::Y * 0.6);
        p.size = 0.15;
        p.spread = 3.0;
        p.life = 5.0;
        self.emit(p);
        let mut f = self.base(muzzle + forward * 0.25, forward * 2.0);
        f.flash = true;
        f.size = 0.45;
        f.spread = 0.3;
        f.life = 0.07;
        f.color = Vec3::new(60.0, 32.0, 9.0);
        self.emit(f);
    }

    /// A field gun's discharge: a great bank of white smoke rolling out ahead.
    pub fn cannon(&mut self, muzzle: Vec3, forward: Vec3) {
        for i in 0..44 {
            let r = self.random();
            let speed = 4.0 + (i % 22) as f32 * 1.1 + r * 6.0;
            let jitter = Vec3::new(self.random() - 0.5, self.random() * 0.8, self.random() - 0.5) * 2.5;
            let mut p = self.base(muzzle + forward * ((i % 22) as f32 * 0.2), forward * speed + jitter);
            p.size = 0.4 + r * 0.35;
            p.spread = 3.5 + r * 3.0;
            p.life = 20.0 + r * 12.0;
            p.color = Vec3::splat(0.76 + r * 0.1);
            p.opacity = 0.95;
            p.drag = 1.1;
            p.rise = 0.3;
            self.emit(p);
        }
        // The flash: a short tongue of flame out of the muzzle, brightest and widest at the
        // mouth, gone in a few frames.
        for k in 0..4 {
            let along = k as f32 * 0.45;
            let mut f = self.base(muzzle + forward * (0.35 + along), forward * 8.0);
            f.flash = true;
            f.size = 0.55 - k as f32 * 0.1;
            f.spread = 0.6;
            f.life = 0.06 + self.random() * 0.03;
            f.color = Vec3::new(60.0, 28.0, 7.0) * (1.0 - k as f32 * 0.2);
            self.emit(f);
        }
    }

    /// Earth thrown up where a roundshot strikes.
    pub fn dust(&mut self, at: Vec3) {
        for _ in 0..10 {
            let r = self.random();
            let v = Vec3::new(self.random() - 0.5, 2.0 + r * 5.0, self.random() - 0.5) * 2.0;
            let mut p = self.base(at, v);
            p.size = 0.5 + r * 0.4;
            p.spread = 4.0;
            p.life = 6.0 + r * 4.0;
            p.color = Vec3::new(0.42, 0.35, 0.26);
            p.opacity = 0.9;
            p.rise = -0.3;
            self.emit(p);
        }
    }

    /// A breath of woodsmoke from a campfire or chimney.
    pub fn woodsmoke(&mut self, at: Vec3) {
        let r = self.random();
        let mut p = self.base(at, Vec3::new(r - 0.5, 0.8, 0.5 - r) * 0.4);
        p.size = 0.25;
        p.spread = 7.0;
        p.life = 16.0;
        p.color = Vec3::new(0.55, 0.55, 0.57);
        p.opacity = 0.35;
        p.rise = 0.9;
        p.drag = 0.5;
        self.emit(p);
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    position: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

pub struct SmokeRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    count: u32,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Smoke>()
        .add_systems(Stage::PostUpdate, simulate)
        .add_systems(Stage::Prepare, prepare);
    app.world.resource_mut::<TransparentDrawFunctions>().0.push(draw);
}

/// Soft, lumpy puffs of smoke, each a little different.
fn puff_images() -> Vec<Image> {
    (0..PUFF_LAYERS)
        .map(|layer| {
            let n = PUFF_SIZE;
            let mut data = Vec::with_capacity((n * n * 4) as usize);
            for y in 0..n {
                for x in 0..n {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                    let q = Vec3::new(p.x * 2.2, p.y * 2.2, layer as f32 * 7.3);
                    let billow = fbm(71 + layer, q, 5);
                    let edge = 1.0 - (p.length() / (0.62 + 0.45 * billow)).powf(2.5);
                    let alpha = edge.clamp(0.0, 1.0) * (0.55 + 0.6 * billow).min(1.0);
                    let shade = (0.8 + 0.35 * fbm(83 + layer, q * 1.7, 3)).min(1.0);
                    let c = (shade * 255.0) as u8;
                    data.extend([c, c, c, (alpha * 255.0) as u8]);
                }
            }
            Image::from_rgba(n, n, data, false)
        })
        .collect()
}

pub fn init(world: &mut World) {
    let gpu = world.resource::<Gpu>();
    let device = &gpu.device;
    let puffs = TextureArray::from_images(gpu, &puff_images(), false);
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("smoke layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("smoke"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&puffs.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&puffs.sampler),
            },
        ],
    });
    let view = world.resource::<ViewBinding>();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("smoke shader"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", gpu.pbr_wgsl(), include_str!("smoke.wgsl")).into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("smoke pipeline layout"),
        bind_group_layouts: &[&view.layout, &layout],
        push_constant_ranges: &[],
    });
    let premultiplied = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("smoke pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: premultiplied,
                    alpha: premultiplied,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: Some(main_depth_state(false)),
        multisample: main_multisample(false),
        multiview: None,
        cache: None,
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke instances"),
        size: (MAX_PARTICLES * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    world.insert_resource(SmokeRenderer {
        pipeline,
        bind_group,
        buffer,
        count: 0,
    });
}

/// Smoke billows out, slows, rises or hangs, drifts downwind and thins away.
fn simulate(time: Res<Time>, mut smoke: ResMut<Smoke>) {
    let dt = time.delta_secs();
    smoke.particles.retain_mut(|p| {
        p.age += dt;
        if p.age >= p.life {
            return false;
        }
        p.velocity *= (-p.drag * dt).exp();
        // Smoke picks up the breeze as it slows.
        let carried = 1.0 - (-p.age * 0.6).exp();
        p.position += (p.velocity + WIND * carried + Vec3::Y * p.rise) * dt;
        p.rotation += p.spin * dt;
        true
    });
}

fn prepare(gpu: Res<Gpu>, frame: Res<RenderFrame>, smoke: Res<Smoke>, renderer: Option<ResMut<SmokeRenderer>>) {
    let Some(mut renderer) = renderer else {
        return;
    };
    let camera = frame.camera_position;
    let mut instances: Vec<(f32, Instance)> = smoke
        .particles
        .iter()
        .map(|p| {
            let t = p.age / p.life;
            // Quick to billow, slow to spread; thins away over its life.
            let grow = 1.0 - (-p.age * 1.2).exp();
            let radius = p.size * (1.0 + (p.spread - 1.0) * (grow * 0.7 + t * 0.3));
            let fade_in = (p.age / 0.15).min(1.0);
            let opacity = p.opacity * fade_in * (1.0 - t).powf(1.5) / (1.0 + t * 2.0);
            let instance = Instance {
                position: [p.position.x, p.position.y, p.position.z, radius],
                color: [p.color.x, p.color.y, p.color.z, opacity],
                params: [p.rotation, p.layer as f32, if p.flash { 1.0 } else { 0.0 }, 0.6],
            };
            (p.position.distance_squared(camera), instance)
        })
        .collect();
    // Back to front, so nearer smoke covers farther.
    instances.sort_by(|a, b| b.0.total_cmp(&a.0));
    let data: Vec<Instance> = instances.into_iter().map(|(_, i)| i).collect();
    if !data.is_empty() {
        gpu.queue.write_buffer(&renderer.buffer, 0, bytemuck::cast_slice(&data));
    }
    renderer.count = data.len() as u32;
}

fn draw(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    let Some(r) = world.get_resource::<SmokeRenderer>() else {
        return;
    };
    if r.count == 0 {
        return;
    }
    pass.set_pipeline(&r.pipeline);
    pass.set_bind_group(1, &r.bind_group, &[]);
    pass.set_vertex_buffer(0, r.buffer.slice(..));
    pass.draw(0..4, 0..r.count);
}
