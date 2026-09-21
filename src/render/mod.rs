mod gpu;
mod mesh;
mod renderer;
mod screenshot;
mod texture;
mod view;

use glam::{Mat4, Vec3};

pub use gpu::{Gpu, DEPTH_FORMAT};
pub use mesh::{Mesh, Mesh3d, Vertex};
pub use renderer::MeshRenderer;
pub use screenshot::Screenshot;
pub use texture::TextureArray;
pub use view::ViewBinding;

use crate::{
    app::{App, Plugin, Stage},
    assets::Assets,
    ecs::{Component, EventReader, Query, Res, ResMut, World},
    transform::GlobalTransform,
    window::{Window, WindowResized, WindowSettings},
};

/// Linear RGBA color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Converts from sRGB, the space color pickers and hex codes use.
    pub fn srgb(r: f32, g: f32, b: f32) -> Self {
        fn linear(c: f32) -> f32 {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        Self::rgb(linear(r), linear(g), linear(b))
    }

    pub fn hex(rgb: u32) -> Self {
        let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.0;
        Self::srgb(channel(16), channel(8), channel(0))
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// Per-entity surface color.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub color: Color,
}

impl Component for Material {}

impl Material {
    pub fn color(color: Color) -> Self {
        Self { color }
    }
}

/// A perspective camera. The first active camera found is used.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    pub active: bool,
}

impl Component for Camera {}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 1000.0,
            active: true,
        }
    }
}

impl Camera {
    pub fn projection(&self, aspect_ratio: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect_ratio, self.near, self.far)
    }
}

/// A sun-like light shining along its entity's forward direction.
#[derive(Clone, Copy, Debug)]
pub struct DirectionalLight {
    pub color: Color,
    pub intensity: f32,
}

impl Component for DirectionalLight {}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AmbientLight {
    pub color: Color,
    pub intensity: f32,
}

impl Default for AmbientLight {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 0.15,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ClearColor(pub Color);

impl Default for ClearColor {
    fn default() -> Self {
        Self(Color::hex(0x87a9c9))
    }
}

/// Blends distant surfaces into the clear color between `start` and `end` world units.
/// Off by default (the range is far beyond any camera's far plane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub start: f32,
    pub end: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            start: 1e9,
            end: 2e9,
        }
    }
}

/// What the renderer needs from the world for one frame, collected by `extract`.
/// The draw code only ever sees this, never the ECS.
#[derive(Default)]
pub struct RenderFrame {
    pub view_proj: Mat4,
    pub camera_position: Vec3,
    pub light_direction: Vec3,
    pub light_color: Vec3,
    pub ambient_color: Vec3,
    pub clear_color: Color,
    pub fog: Fog,
    /// (mesh id, model matrix, color)
    pub objects: Vec<(u32, Mat4, Color)>,
    pub has_camera: bool,
}

/// A function that records draw calls into the main pass. The view is already bound at group 0.
pub type DrawFn = fn(&World, &mut wgpu::RenderPass<'_>);

/// The draw functions run every frame, in order. Plugins push their own to add a pipeline.
#[derive(Default)]
pub struct DrawFunctions(pub Vec<DrawFn>);

fn init_gpu(world: &mut World) {
    let window = world.resource::<Window>().handle().clone();
    let vsync = world
        .get_resource::<WindowSettings>()
        .is_none_or(|s| s.vsync);
    let gpu = pollster::block_on(Gpu::new(window, vsync)).expect("failed to initialize the GPU");
    let view = ViewBinding::new(&gpu);
    let renderer = MeshRenderer::new(&gpu, &view);
    world.insert_resource(gpu);
    world.insert_resource(view);
    world.insert_resource(renderer);
}

fn resize(mut gpu: ResMut<Gpu>, mut events: EventReader<WindowResized>) {
    if let Some(size) = events.read().last() {
        gpu.resize(size.width, size.height);
    }
}

#[allow(clippy::too_many_arguments)]
fn extract(
    mut frame: ResMut<RenderFrame>,
    gpu: Res<Gpu>,
    clear: Res<ClearColor>,
    ambient: Res<AmbientLight>,
    fog: Res<Fog>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    lights: Query<(&DirectionalLight, &GlobalTransform)>,
    objects: Query<(&Mesh3d, &GlobalTransform, Option<&Material>)>,
) {
    let frame = &mut *frame;
    frame.clear_color = clear.0;
    frame.fog = *fog;
    frame.ambient_color = Vec3::from_slice(&ambient.color.to_array()) * ambient.intensity;

    let camera = cameras.iter().find(|(camera, _)| camera.active);
    frame.has_camera = camera.is_some();
    if let Some((camera, transform)) = camera {
        let view = transform.0.inverse();
        frame.view_proj = camera.projection(gpu.aspect_ratio()) * view;
        frame.camera_position = transform.translation();
    }

    match lights.iter().next() {
        Some((light, transform)) => {
            frame.light_direction = transform.forward();
            frame.light_color = Vec3::from_slice(&light.color.to_array()) * light.intensity;
        }
        None => {
            frame.light_direction = Vec3::NEG_Y;
            frame.light_color = Vec3::ZERO;
        }
    }

    frame.objects.clear();
    frame
        .objects
        .extend(objects.iter().map(|(mesh, transform, material)| {
            let color = material.map_or(Color::WHITE, |m| m.color);
            (mesh.0.id(), transform.0, color)
        }));
}

fn prepare_view(gpu: Res<Gpu>, view: Res<ViewBinding>, frame: Res<RenderFrame>) {
    view.write(&gpu, &frame);
}

fn prepare_meshes(
    gpu: Res<Gpu>,
    mut renderer: ResMut<MeshRenderer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut frame: ResMut<RenderFrame>,
) {
    renderer.sync_meshes(&gpu, &mut meshes);
    renderer.build_batches(&gpu, &mut frame);
}

fn draw_meshes(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    world.resource::<MeshRenderer>().draw(pass);
}

/// Records one pass containing every registered draw function, then presents.
fn render(world: &mut World) {
    let frame = world.resource::<RenderFrame>();
    if !frame.has_camera {
        return;
    }
    let clear = frame.clear_color;
    let gpu = world.resource::<Gpu>();
    let output = match gpu.surface.get_current_texture() {
        Ok(output) => output,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            world.resource_mut::<Gpu>().reconfigure();
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => return,
        Err(err) => {
            log::error!("failed to acquire a frame: {err}");
            return;
        }
    };
    let target = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear.r as f64,
                        g: clear.g as f64,
                        b: clear.b as f64,
                        a: clear.a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &gpu.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, &world.resource::<ViewBinding>().bind_group, &[]);
        for draw in &world.resource::<DrawFunctions>().0 {
            draw(world, &mut pass);
        }
    }
    let path = world.resource::<Screenshot>().path.clone();
    let readback = path
        .as_ref()
        .and_then(|_| screenshot::copy_to_buffer(gpu, &mut encoder, &output.texture));
    gpu.queue.submit([encoder.finish()]);
    if let (Some(path), Some(readback)) = (&path, readback) {
        match screenshot::save(gpu, readback, path) {
            Ok(()) => log::info!("saved screenshot to {}", path.display()),
            Err(err) => log::error!("failed to save screenshot: {err:#}"),
        }
    }
    output.present();
    if path.is_some() {
        world.resource_mut::<Screenshot>().path = None;
    }
}

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<ClearColor>()
            .init_resource::<AmbientLight>()
            .init_resource::<Fog>()
            .init_resource::<Screenshot>()
            .init_resource::<RenderFrame>()
            .add_systems(Stage::PreStartup, init_gpu)
            .add_systems(Stage::PreUpdate, resize)
            .add_systems(Stage::Extract, extract)
            .add_systems(Stage::Prepare, (prepare_view, prepare_meshes))
            .add_systems(Stage::Render, render);
        app.world.init_resource::<DrawFunctions>();
        app.world
            .resource_mut::<DrawFunctions>()
            .0
            .push(draw_meshes);
    }
}
