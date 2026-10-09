mod animation;
mod environment;
pub mod gait;
pub mod reach;
mod gltf_scene;
mod gpu;
mod image;
mod mesh;
mod post;
mod probes;
mod raytrace;
mod renderer;
mod screenshot;
mod shaders;
mod shadow;
mod skin;
mod taa;
mod texture;
mod view;
mod volumetric;

use glam::{Mat4, Vec2, Vec3, Vec4};

pub use environment::{direction_to_uv, uv_to_direction, Environment};
pub use gltf_scene::{GltfPart, GltfScene};
pub use gpu::{
    main_depth_state, main_multisample, Gpu, Targets, DEPTH_CLEAR, DEPTH_COMPARE, DEPTH_FORMAT,
    HDR_FORMAT, MSAA_SAMPLES,
};
pub use image::Image;
pub use mesh::{Mesh, Mesh3d, Vertex};
pub use post::PostProcess;
pub use shaders::{Install, Rebuild, Shader, ShaderReload};
pub use raytrace::{GeometryId, HitMaterial, RayTracing, RayTracingSettings};
pub use animation::{AnimationClip, Animator, Palette, SkinWeights, Skeleton, Skinned};
pub use gait::{Gait, Leg, Pattern};
pub use reach::{Limb, Reach};
pub use probes::ProbeGrid;
pub use volumetric::VolumetricLight;
pub use renderer::MeshRenderer;
pub use screenshot::Screenshot;
pub use shadow::{shadow_depth_state, CascadeData, ShadowMaps, ShadowSettings, CASCADES};
pub use texture::TextureArray;
pub use view::ViewBinding;
use probes::ProbeBaker;

use crate::{
    app::{App, Plugin, Stage},
    asset_server::AssetServer,
    assets::{Assets, Handle},
    ecs::{Component, EventReader, Query, Res, ResMut, World},
    reflect::Reflect,
    time::Time,
    transform::{GlobalTransform, Transform},
    window::{Window, WindowResized, WindowSettings},
};
use environment::GpuEnvironment;
use post::PostRenderer;
use renderer::{BatchKey, RenderObject, CASTS_SHADOW, VISIBLE};

/// Physically based lighting shared by every main-pass pipeline: the `View` uniform, sun and
/// shadows, sky light and haze, at group 0. Prepend it to a custom shader's source.
pub const PBR_WGSL: &str = concat!(include_str!("pbr.wgsl"), include_str!("rt_off.wgsl"));
/// [`PBR_WGSL`] with ray-traced shadows, occlusion and reflections; see [`Gpu::pbr_wgsl`].
pub const PBR_RT_WGSL: &str = concat!(include_str!("pbr.wgsl"), include_str!("rt_on.wgsl"));

/// Linear RGBA color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
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

    pub fn to_vec3(self) -> Vec3 {
        Vec3::new(self.r, self.g, self.b)
    }
}

/// How a surface looks: a metallic-roughness PBR material, as in glTF.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.Material", default)]
pub struct Material {
    /// Multiplies the base color texture and the mesh's vertex colors.
    pub color: Color,
    pub roughness: f32,
    pub metallic: f32,
    /// Light the surface gives off itself, linear RGB, in the same units as the sun.
    pub emissive: Color,
    /// How much light passes through, for leaves and grass lit from behind.
    pub translucency: f32,
    pub base_color_texture: Option<Handle<Image>>,
    pub normal_texture: Option<Handle<Image>>,
    pub normal_strength: f32,
    /// Green is roughness and blue metalness, multiplying the factors; red is ambient
    /// occlusion.
    pub metallic_roughness_texture: Option<Handle<Image>>,
    /// Greyscale height (white is high), for parallax: surfaces seem to have real relief.
    pub height_texture: Option<Handle<Image>>,
    /// How far, in metres, white stands above black in the height texture.
    pub height_scale: f32,
    /// Light scattering beneath the surface, 0 to 1: skin, wax, marble.
    pub subsurface: f32,
    /// Soot, rain streaks and splashed mud, worked out from world position, 0 to 1.
    pub weathering: f32,
    /// Pixels with less alpha than this are cut away, as for leaves.
    pub alpha_cutoff: Option<f32>,
    pub double_sided: bool,
    /// A decal: a stain, a crack or a poster laid over another surface. It blends over what is
    /// beneath by its alpha, casts no shadow, and stays out of the traced scene. Lay it a few
    /// millimetres off the surface.
    pub decal: bool,
    /// Puddles on level ground, 0 to 1: water gathers in the low parts of the height map.
    pub puddles: f32,
    /// Moving water, 0 for still: ripples drifting with `flow` (metres a second across level
    /// water; falling water streams down).
    pub waves: f32,
    pub flow: Vec2,
}

impl Component for Material {}

impl Default for Material {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            roughness: 0.8,
            metallic: 0.0,
            emissive: Color::BLACK,
            translucency: 0.0,
            base_color_texture: None,
            normal_texture: None,
            normal_strength: 1.0,
            metallic_roughness_texture: None,
            alpha_cutoff: None,
            double_sided: false,
            height_texture: None,
            height_scale: 0.0,
            subsurface: 0.0,
            weathering: 0.0,
            decal: false,
            puddles: 0.0,
            waves: 0.0,
            flow: Vec2::ZERO,
        }
    }
}

impl Material {
    pub fn color(color: Color) -> Self {
        Self {
            color,
            ..Default::default()
        }
    }

    /// Something that glows, like a flame, and isn't lit by anything else.
    pub fn emissive(color: Color) -> Self {
        Self {
            color: Color::BLACK,
            emissive: color,
            ..Default::default()
        }
    }
}

/// One level of detail: drawn while the camera is nearer than `max_distance`.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.LodLevel")]
pub struct LodLevel {
    pub max_distance: f32,
    pub mesh: Handle<Mesh>,
    pub material: Material,
}

/// Swaps in simpler meshes as the camera moves away, instead of `Mesh3d`. Levels are ordered
/// nearest first; beyond the last one the entity isn't drawn at all.
#[derive(Clone, Debug, Default, Reflect)]
#[reflect(name = "mira.Lods")]
pub struct Lods(pub Vec<LodLevel>);

impl Component for Lods {}

impl Lods {
    pub fn pick(&self, distance: f32) -> Option<&LodLevel> {
        self.0.iter().find(|level| distance < level.max_distance)
    }
}

/// Keeps a mesh out of the shadow cascades, for things too thin or diffuse to cast.
#[derive(Clone, Copy, Debug, Default, Reflect)]
#[reflect(name = "mira.NotShadowCaster")]
pub struct NotShadowCaster;

impl Component for NotShadowCaster {}

/// A perspective camera. The first active camera found is used. The projection has no far
/// plane: everything in front of `near` is drawn.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.Camera", default)]
pub struct Camera {
    pub fov_y: f32,
    pub near: f32,
    pub active: bool,
}

impl Component for Camera {}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y: 60f32.to_radians(),
            near: 0.1,
            active: true,
        }
    }
}

impl Camera {
    /// Reversed-Z, infinitely far: depth runs from 1 at `near` to 0 at the horizon.
    pub fn projection(&self, aspect_ratio: f32) -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(self.fov_y, aspect_ratio, self.near)
    }
}

/// The sun, shining along its entity's forward direction.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(name = "mira.DirectionalLight", default)]
pub struct DirectionalLight {
    pub color: Color,
    /// Illuminance on a surface facing the light, in the same units as the sky's radiance.
    pub intensity: f32,
    pub shadows: bool,
}

impl Component for DirectionalLight {}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 3.0,
            shadows: true,
        }
    }
}

impl Environment {
    /// A light matching the sun found in the sky image, pointed the way its light travels.
    pub fn sun_light(&self) -> (Transform, DirectionalLight) {
        let illuminance = self.sun_illuminance * self.intensity;
        let intensity = illuminance.max_element().max(1e-6);
        let color = illuminance / intensity;
        (
            Transform::IDENTITY.looking_at(-self.sun_direction, Vec3::Y),
            DirectionalLight {
                color: Color::rgb(color.x, color.y, color.z),
                intensity,
                shadows: true,
            },
        )
    }
}

/// Extra flat light from every direction, on top of the sky's.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(name = "mira.AmbientLight", default)]
pub struct AmbientLight {
    pub color: Color,
    pub intensity: f32,
}

impl Default for AmbientLight {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 0.0,
        }
    }
}

/// Haze that thickens with distance and thins with height, lit by the sky. Off by default.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(name = "mira.Fog", default)]
pub struct Fog {
    /// Extinction per metre at `base_height`.
    pub density: f32,
    /// How quickly the haze thins with height, per metre.
    pub height_falloff: f32,
    pub base_height: f32,
    /// Metres from the camera before the haze begins. Zero is ordinary atmosphere, thickening
    /// from the eye outward; a large value keeps everything nearby clear and hazes only the
    /// distance, for hiding the edge of a world.
    pub start: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            density: 0.0,
            height_falloff: 0.01,
            base_height: 0.0,
            start: 0.0,
        }
    }
}

/// What the renderer needs from the world for one frame, collected by `extract`.
/// The draw code only ever sees this, never the ECS.
#[derive(Default)]
pub struct RenderFrame {
    /// The camera, nudged by this frame's sub-pixel jitter when TAA is on.
    pub view_proj: Mat4,
    /// The camera without the jitter.
    pub unjittered_view_proj: Mat4,
    /// Counts frames; noisy effects vary their pattern with it.
    pub frame_index: u32,
    /// This frame's jitter, in pixels.
    pub jitter: Vec2,
    /// Whether frames are blended over time (TAA).
    pub temporal: bool,
    pub camera_position: Vec3,
    pub camera_forward: Vec3,
    pub time: f32,
    pub exposure: f32,
    /// Toward the sun.
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    pub shadows: bool,
    pub ambient_color: Vec3,
    pub sky_rotation: f32,
    pub sky_intensity: f32,
    pub sh: [Vec3; 9],
    pub fog: Fog,
    pub cascades: CascadeData,
    pub(crate) objects: Vec<RenderObject>,
    /// Skinned meshes to pose this frame.
    pub(crate) skinned: Vec<skin::SkinJob>,
    pub has_camera: bool,
}

/// A function that records draw calls. In the main pass the view is bound at group 0; in the
/// shadow pass group 0 is the cascade's light matrix.
pub type DrawFn = fn(&World, &mut wgpu::RenderPass<'_>);

/// Opaque draws in the main pass, run in order. Plugins push their own to add a pipeline.
#[derive(Default)]
pub struct DrawFunctions(pub Vec<DrawFn>);

/// Blended draws, after everything opaque and the sky.
#[derive(Default)]
pub struct TransparentDrawFunctions(pub Vec<DrawFn>);

/// Draws into each shadow cascade. Pipelines here use `ShadowMaps::layout` at group 0 and
/// `shadow_depth_state()`.
#[derive(Default)]
pub struct ShadowDrawFunctions(pub Vec<DrawFn>);

struct SkyRenderer {
    pipeline: wgpu::RenderPipeline,
    gpu_environment: GpuEnvironment,
}

fn init_gpu(world: &mut World) {
    let window = world.resource::<Window>().handle().clone();
    let vsync = world
        .get_resource::<WindowSettings>()
        .is_none_or(|s| s.vsync);
    let rt_settings = world.get_resource::<RayTracingSettings>().copied().unwrap_or_default();
    let gpu = pollster::block_on(Gpu::new(window, vsync, rt_settings.enabled)).expect("failed to initialize the GPU");
    let rt = gpu.ray_tracing.then(|| {
        let white = Image::solid([255, 255, 255, 255], true).upload(&gpu.device, &gpu.queue);
        RayTracing::new(&gpu, rt_settings, white)
    });
    if !world.contains_resource::<Environment>() {
        world.insert_resource(Environment::gradient(Vec3::new(-0.5, 0.6, 0.4)));
    }
    let resolution = world.resource::<ShadowSettings>().resolution;
    let shadows = ShadowMaps::new(&gpu, resolution);
    let gpu_environment = GpuEnvironment::new(&gpu.device, &gpu.queue, world.resource::<Environment>());
    let view = ViewBinding::new(&gpu, &shadows, &gpu_environment, rt.as_ref());
    let renderer = MeshRenderer::new(&gpu, &view, &shadows);
    if rt.is_some() {
        world.insert_resource(ProbeBaker::new(&gpu, &view.layout));
    }
    let sky = SkyRenderer {
        pipeline: sky_pipeline(&gpu, &view),
        gpu_environment,
    };
    world.resource_mut::<ShaderReload>().0.push(rebuild_pipelines);
    world.insert_resource(PostRenderer::new(&gpu));
    world.insert_resource(taa::Taa::new(&gpu));
    world.insert_resource(skin::Skinner::new(&gpu));
    world.insert_resource(volumetric::Shafts::new(&gpu, &view.layout));
    world.insert_resource(sky);
    world.insert_resource(gpu);
    world.insert_resource(shadows);
    world.insert_resource(view);
    world.insert_resource(renderer);
    if let Some(rt) = rt {
        world.insert_resource(rt);
    }
}

/// Rebuilds everything the engine's own renderers make from shaders.
fn rebuild_pipelines(world: &World) -> Option<shaders::Install> {
    let gpu = world.resource::<Gpu>();
    let view = world.resource::<ViewBinding>();
    let meshes = MeshRenderer::new(gpu, view, world.resource::<ShadowMaps>());
    let sky = sky_pipeline(gpu, view);
    let post = PostRenderer::new(gpu);
    let taa = taa::Taa::new(gpu);
    let skinner = skin::Skinner::new(gpu);
    let shafts = volumetric::Shafts::new(gpu, &view.layout);
    Some(Box::new(move |world: &mut World| {
        world.resource_mut::<MeshRenderer>().adopt_pipelines(meshes);
        world.resource_mut::<SkyRenderer>().pipeline = sky;
        world.resource_mut::<skin::Skinner>().adopt_pipeline(skinner);
        // These keep nothing that can't be made again; the frame history just starts over.
        world.insert_resource(post);
        world.insert_resource(taa);
        world.insert_resource(shafts);
    }))
}

fn sky_pipeline(gpu: &Gpu, view: &ViewBinding) -> wgpu::RenderPipeline {
    let device = &gpu.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky shader"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", gpu.pbr_wgsl(), crate::shader!("sky.wgsl").source()).into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky layout"),
        bind_group_layouts: &[&view.layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky pipeline"),
        layout: Some(&layout),
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
        primitive: Default::default(),
        depth_stencil: Some(main_depth_state(false)),
        multisample: main_multisample(false),
        multiview: None,
        cache: None,
    })
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
    time: Res<Time>,
    ambient: Res<AmbientLight>,
    fog: Res<Fog>,
    environment: Res<Environment>,
    shadow_settings: Res<ShadowSettings>,
    post: Res<PostProcess>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    lights: Query<(&DirectionalLight, &GlobalTransform)>,
    renderer: Res<MeshRenderer>,
    objects: Query<Drawable>,
) {
    let frame = &mut *frame;
    frame.time = time.elapsed_secs();
    frame.exposure = post.exposure;
    frame.fog = *fog;
    frame.ambient_color = ambient.color.to_vec3() * ambient.intensity;
    frame.sh = environment.sh;
    frame.sky_rotation = environment.rotation;
    frame.sky_intensity = environment.intensity;

    match lights.iter().next() {
        Some((light, transform)) => {
            frame.sun_direction = -transform.forward();
            frame.sun_color = light.color.to_vec3() * light.intensity;
            frame.shadows = light.shadows && shadow_settings.enabled;
        }
        None => {
            frame.sun_direction = Vec3::Y;
            frame.sun_color = Vec3::ZERO;
            frame.shadows = false;
        }
    }

    let camera = cameras.iter().find(|(camera, _)| camera.active);
    frame.has_camera = camera.is_some();
    if let Some((camera, transform)) = camera {
        let view = transform.0.inverse();
        let projection = camera.projection(gpu.aspect_ratio());
        frame.unjittered_view_proj = projection * view;
        frame.temporal = post.taa;
        frame.frame_index = frame.frame_index.wrapping_add(1);
        frame.jitter = if post.taa { taa::jitter(frame.frame_index) } else { Vec2::ZERO };
        // A shift of the whole image by the jitter, in clip space.
        let size = Vec2::new(gpu.config.width as f32, gpu.config.height as f32);
        let shift = frame.jitter * 2.0 / size;
        frame.view_proj = Mat4::from_translation(Vec3::new(shift.x, -shift.y, 0.0)) * projection * view;
        frame.camera_position = transform.translation();
        frame.camera_forward = transform.forward();
        frame.cascades = CascadeData::compute(
            &shadow_settings,
            transform.0,
            (camera.fov_y * 0.5).tan(),
            gpu.aspect_ratio(),
            camera.near,
            frame.sun_direction,
        );
    }

    frame.objects.clear();
    let planes = frustum_planes(frame.view_proj);
    let camera = frame.camera_position;
    // Casters beyond this can't shadow anything the cascades cover.
    let shadow_reach = shadow_settings.max_distance * 1.2 + 40.0;
    let shadows = frame.shadows;
    for (mesh, lods, transform, material, no_shadow) in &objects {
        let distance = camera.distance(transform.translation());
        let (mesh, material) = match (lods, mesh) {
            (Some(lods), _) => match lods.pick(distance) {
                Some(level) => (level.mesh, level.material),
                None => continue,
            },
            (None, Some(mesh)) => (mesh.0, material.copied().unwrap_or_default()),
            (None, None) => continue,
        };
        let casts = no_shadow.is_none() && shadows && !material.decal;
        let mut flags = VISIBLE | if casts { CASTS_SHADOW } else { 0 };
        if let Some((center, radius)) = renderer.bounds(mesh.id()) {
            let center = transform.0.transform_point3(center);
            let scale = transform.0.x_axis.truncate().length()
                .max(transform.0.y_axis.truncate().length())
                .max(transform.0.z_axis.truncate().length());
            let radius = radius * scale;
            if planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius * p.truncate().length()) {
                flags &= !VISIBLE;
            }
            if center.distance(camera) - radius > shadow_reach {
                flags &= !CASTS_SHADOW;
            }
        }
        if flags != 0 {
            frame.objects.push(RenderObject {
                key: BatchKey::new(mesh.id(), &material),
                model: transform.0,
                material,
                flags,
            });
        }
    }
}

/// What `extract` reads from each entity that might be drawn.
type Drawable = (
    Option<&'static Mesh3d>,
    Option<&'static Lods>,
    &'static GlobalTransform,
    Option<&'static Material>,
    Option<&'static NotShadowCaster>,
);

/// Planes `(normal, d)` bounding the view, inward-facing. With reversed, infinite Z there is
/// no far plane; the near plane is `z <= w`.
fn frustum_planes(view_proj: Mat4) -> [Vec4; 5] {
    let (x, y, z, w) = (
        view_proj.row(0),
        view_proj.row(1),
        view_proj.row(2),
        view_proj.row(3),
    );
    [w + x, w - x, w + y, w - y, w - z]
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    gpu: Res<Gpu>,
    mut view: ResMut<ViewBinding>,
    mut sky: ResMut<SkyRenderer>,
    environment: Res<Environment>,
    shadows: Res<ShadowMaps>,
    mut renderer: ResMut<MeshRenderer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut frame: ResMut<RenderFrame>,
    rt: Option<ResMut<RayTracing>>,
    mut skinner: ResMut<skin::Skinner>,
) {
    let mut rt = rt;
    if sky.gpu_environment.generation != environment.generation {
        sky.gpu_environment = GpuEnvironment::new(&gpu.device, &gpu.queue, &environment);
        view.rebind(&gpu, &shadows, &sky.gpu_environment, rt.as_deref());
    }
    let probes = rt.as_deref().map_or([[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 0.0]], |rt| rt.probes.uniform());
    view.write(&gpu, &frame, sky.gpu_environment.mip_count, &shadows, probes);
    shadows.write(&gpu, &frame.cascades);
    renderer.sync_meshes(&gpu, &mut meshes);
    renderer.sync_images(&gpu, &mut images);
    renderer.build_batches(&gpu, &mut frame);
    skinner.prepare(&gpu, &renderer, &meshes, &frame.skinned);

    // Everything opaque near enough to matter goes into the traced scene, seen or not: what's
    // behind the camera still casts shadows and shows in reflections.
    if let Some(rt) = rt.as_deref_mut() {
        rt.begin_frame();
        let reach = rt.settings.max_distance;
        for object in &frame.objects {
            // Moving water stays out too: its traced copy would be frozen flat while the surface
            // ripples through it.
            if object.material.alpha_cutoff.is_some() || object.material.decal || object.material.waves > 0.0 {
                continue;
            }
            let at = object.model.w_axis.truncate();
            if at.distance(frame.camera_position) > reach {
                continue;
            }
            let id = object.key.mesh();
            let geometry = if frame.skinned.iter().any(|job| job.mesh == id) {
                match (meshes.get_by_id(id), renderer.vertex_buffer(id), renderer.index_buffer(id)) {
                    (Some(mesh), Some(vertices), Some(indices)) => Some(rt.skinned_geometry(&gpu, id, mesh, vertices, indices)),
                    _ => None,
                }
            } else {
                rt.mesh_geometry(&gpu, &meshes, id)
            };
            if let Some(geometry) = geometry {
                let m = &object.material;
                let texture = m
                    .base_color_texture
                    .map_or(0, |t| rt.texture_slot(t.id(), renderer.image_view(t.id())));
                let material = raytrace::HitMaterial {
                    color: m.color.to_array(),
                    emissive: [m.emissive.r, m.emissive.g, m.emissive.b, 0.0],
                    texture,
                    roughness: m.roughness,
                    metallic: m.metallic,
                    ..raytrace::HitMaterial::plain(m.color)
                };
                rt.push(geometry, object.model, material);
            }
        }
        rt.upload(&gpu);
        if rt.generation != view.rt_generation {
            view.rt_generation = rt.generation;
            view.rebind(&gpu, &shadows, &sky.gpu_environment, Some(rt));
        }
    }
}

/// Collects the skinned meshes to pose this frame.
fn extract_skins(mut frame: ResMut<RenderFrame>, skinned: Query<(&Mesh3d, &Skinned)>) {
    frame.skinned.clear();
    for (mesh, skin) in &skinned {
        let palette = skin.palette.lock().unwrap_or_else(|e| e.into_inner()).clone();
        frame.skinned.push(skin::SkinJob {
            mesh: mesh.0.id(),
            source: skin.source.id(),
            weights: skin.weights.clone(),
            palette,
        });
    }
}

/// Advances every animator and recomputes its palette.
fn animate(time: Res<Time>, mut animators: Query<&mut Animator>) {
    let dt = time.delta_secs();
    for mut animator in &mut animators {
        animator.update(dt);
    }
}

fn draw_meshes(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    world.resource::<MeshRenderer>().draw(pass);
}

fn draw_mesh_shadows(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    world.resource::<MeshRenderer>().draw_shadows(pass);
}

fn draw_sky(world: &World, pass: &mut wgpu::RenderPass<'_>) {
    pass.set_pipeline(&world.resource::<SkyRenderer>().pipeline);
    pass.draw(0..3, 0..1);
}

/// Records the shadow cascades, the main HDR pass and post-processing, then presents.
fn render(world: &mut World) {
    let frame = world.resource::<RenderFrame>();
    if !frame.has_camera {
        return;
    }
    let cast_shadows = frame.shadows;
    let gpu = world.resource::<Gpu>();
    // Without a frame to draw in (the window hidden, or the screen locked), draw off screen
    // instead, so the world still renders and screenshots can still be taken.
    let output = match gpu.surface.get_current_texture() {
        Ok(output) => Some(output),
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            world.resource_mut::<Gpu>().reconfigure();
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => return,
        Err(err) => {
            if !world.contains_resource::<Offscreen>() {
                log::warn!("no frame from the window ({err}); drawing off screen");
            }
            None
        }
    };
    if output.is_none() {
        let (w, h) = (gpu.config.width, gpu.config.height);
        let stale = world.get_resource::<Offscreen>().is_none_or(|o| o.0.width() != w || o.0.height() != h);
        if stale {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            world.insert_resource(Offscreen(texture));
        }
    }
    let gpu = world.resource::<Gpu>();
    let frame_texture = match &output {
        Some(output) => output.texture.clone(),
        None => world.resource::<Offscreen>().0.clone(),
    };
    let target = frame_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
    world.resource::<skin::Skinner>().dispatch(&mut encoder);
    if world.contains_resource::<RayTracing>() {
        world.resource_scope(|world, rt: &mut RayTracing| {
            rt.build(&mut encoder);
            let scene = rt.scene_key();
            if rt.ready() && rt.probes.wants_bake(scene) {
                let view = &world.resource::<ViewBinding>().bind_group;
                world
                    .resource::<ProbeBaker>()
                    .bake(world.resource::<Gpu>(), &mut encoder, view, &mut rt.probes);
            }
        });
    }
    let gpu = world.resource::<Gpu>();

    if cast_shadows {
        let shadows = world.resource::<ShadowMaps>();
        for (cascade, layer) in shadows.layer_views.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: layer,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &shadows.bind_group, &[ShadowMaps::offset(cascade)]);
            for draw in &world.resource::<ShadowDrawFunctions>().0 {
                draw(world, &mut pass);
            }
        }
    }

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gpu.targets.hdr_msaa,
                resolve_target: Some(&gpu.targets.hdr),
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &gpu.targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                    // Kept for the light shafts and TAA, which read it afterwards.
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
        for draw in &world.resource::<TransparentDrawFunctions>().0 {
            draw(world, &mut pass);
        }
    }

    // Sunlight scattered in the air, with the shadows' shafts through it.
    let air = *world.resource::<VolumetricLight>();
    if air.enabled && cast_shadows {
        world.resource_scope(|world, shafts: &mut volumetric::Shafts| {
            shafts.render(world.resource::<Gpu>(), &mut encoder, &world.resource::<ViewBinding>().bind_group, &air);
        });
    }

    let settings = *world.resource::<PostProcess>();
    let (unjittered, time) = {
        let frame = world.resource::<RenderFrame>();
        (frame.unjittered_view_proj, frame.time)
    };
    let scene = world.resource_scope(|world, taa: &mut taa::Taa| {
        let gpu = world.resource::<Gpu>();
        if settings.taa {
            taa.resolve(gpu, &mut encoder, unjittered).clone()
        } else {
            taa.reset();
            gpu.targets.hdr.clone()
        }
    });
    world.resource_scope(|world, post: &mut PostRenderer| {
        post.render(world.resource::<Gpu>(), &mut encoder, &scene, &target, &settings, time);
    });

    let gpu = world.resource::<Gpu>();
    let path = world.resource::<Screenshot>().path.clone();
    let readback = path
        .as_ref()
        .and_then(|_| screenshot::copy_to_buffer(gpu, &mut encoder, &frame_texture));
    gpu.queue.submit([encoder.finish()]);
    if let (Some(path), Some(readback)) = (&path, readback) {
        match screenshot::save(gpu, readback, path) {
            Ok(()) => log::info!("saved screenshot to {}", path.display()),
            Err(err) => log::error!("failed to save screenshot: {err:#}"),
        }
    }
    if let Some(output) = output {
        output.present();
    }
    if path.is_some() {
        world.resource_mut::<Screenshot>().path = None;
    }
}

/// Where frames are drawn while the window can't give us one.
struct Offscreen(wgpu::Texture);

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Camera>()
            .register_type::<DirectionalLight>()
            .register_type::<Mesh3d>()
            .register_type::<Material>()
            .register_type::<Lods>()
            .register_type::<NotShadowCaster>()
            .register_resource_type::<AmbientLight>()
            .register_resource_type::<Fog>()
            .register_resource_type::<PostProcess>()
            .register_resource_type::<ShadowSettings>()
            .register_resource_type::<VolumetricLight>();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<AmbientLight>()
            .init_resource::<Fog>()
            .init_resource::<ShadowSettings>()
            .init_resource::<PostProcess>()
            .init_resource::<Screenshot>()
            .init_resource::<ShaderReload>()
            .init_resource::<AssetServer>()
            .add_systems(
                Stage::First,
                (shaders::reload_changed, crate::asset_server::update_asset_server),
            )
            .init_resource::<RenderFrame>()
            .init_resource::<VolumetricLight>()
            .add_systems(Stage::PreStartup, init_gpu)
            .add_systems(Stage::PreUpdate, resize)
            .add_systems(Stage::PostUpdate, (animate, gait::walk, reach::reach))
            .add_systems(Stage::Extract, (extract, extract_skins))
            .add_systems(Stage::Prepare, prepare)
            .add_systems(Stage::Render, render);
        app.world.init_resource::<DrawFunctions>();
        app.world.init_resource::<TransparentDrawFunctions>();
        app.world.init_resource::<ShadowDrawFunctions>();
        app.world
            .resource_mut::<DrawFunctions>()
            .0
            .extend([draw_meshes as DrawFn, draw_sky]);
        app.world
            .resource_mut::<ShadowDrawFunctions>()
            .0
            .push(draw_mesh_shadows);
    }
}
