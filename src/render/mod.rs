mod gpu;
mod mesh;
mod renderer;

use glam::{Mat4, Vec3};

pub use gpu::Gpu;
pub use mesh::{Mesh, Mesh3d, Vertex};
pub use renderer::MeshRenderer;

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
    /// (mesh id, model matrix, color)
    pub objects: Vec<(u32, Mat4, Color)>,
    pub has_camera: bool,
}

fn init_gpu(world: &mut World) {
    let window = world.resource::<Window>().handle().clone();
    let vsync = world
        .get_resource::<WindowSettings>()
        .is_none_or(|s| s.vsync);
    let gpu = pollster::block_on(Gpu::new(window, vsync)).expect("failed to initialize the GPU");
    let renderer = MeshRenderer::new(&gpu);
    world.insert_resource(gpu);
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
    cameras: Query<(&Camera, &GlobalTransform)>,
    lights: Query<(&DirectionalLight, &GlobalTransform)>,
    objects: Query<(&Mesh3d, &GlobalTransform, Option<&Material>)>,
) {
    let frame = &mut *frame;
    frame.clear_color = clear.0;
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

fn draw(
    gpu: Res<Gpu>,
    mut renderer: ResMut<MeshRenderer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut frame: ResMut<RenderFrame>,
) {
    renderer.sync_meshes(&gpu, &mut meshes);
    if frame.has_camera {
        renderer.render(&gpu, &mut frame);
    }
}

fn reconfigure_lost_surface(world: &mut World) {
    let lost = world
        .get_resource_mut::<MeshRenderer>()
        .is_some_and(|r| std::mem::take(&mut r.surface_lost));
    if lost {
        world.resource_mut::<Gpu>().reconfigure();
    }
}

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<ClearColor>()
            .init_resource::<AmbientLight>()
            .init_resource::<RenderFrame>()
            .add_systems(Stage::PreStartup, init_gpu)
            .add_systems(Stage::PreUpdate, resize)
            .add_systems(Stage::Render, (extract, draw, reconfigure_lost_surface));
    }
}
