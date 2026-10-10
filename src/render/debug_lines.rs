//! Lines drawn over the frame for looking into a game: the shapes physics collides with,
//! where something is headed, the edges of a region. Anything may add lines during a frame;
//! they are drawn once, over the finished frame, faint where the scene hides them, and
//! forgotten.

use glam::{Mat4, Quat, Vec3};

use super::{gpu::Gpu, Color, RenderFrame};
use crate::ecs::World;

/// Must match the instance inputs of `debug_lines.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Line {
    from: [f32; 3],
    width: f32,
    to: [f32; 3],
    _pad: f32,
    color: [f32; 4],
}

/// The lines to draw this frame. They last one frame: add them every frame they should show.
///
/// ```ignore
/// fn show_reach(mut lines: ResMut<DebugLines>, guards: Query<(&GlobalTransform, &Guard)>) {
///     for (at, guard) in &guards {
///         lines.circle(at.translation(), Vec3::Y, guard.reach, Color::rgb(1.0, 0.8, 0.1));
///     }
/// }
/// ```
pub struct DebugLines {
    lines: Vec<Line>,
    /// How wide lines added from now on are, in pixels.
    pub width: f32,
    /// Nothing is drawn while this is false; lines are still forgotten each frame.
    pub shown: bool,
    /// How strongly a line shows where something in the scene is in front of it: 0 hides it
    /// there, 1 draws it as if nothing were in the way.
    pub through: f32,
}

impl Default for DebugLines {
    fn default() -> Self {
        Self { lines: Vec::new(), width: 1.5, shown: true, through: 0.15 }
    }
}

/// How many segments a full circle is drawn with.
const AROUND: usize = 32;

impl DebugLines {
    pub fn line(&mut self, from: Vec3, to: Vec3, color: Color) {
        self.lines.push(Line {
            from: from.to_array(),
            width: self.width,
            to: to.to_array(),
            _pad: 0.0,
            color: [color.r, color.g, color.b, color.a],
        });
    }

    /// A line from `from` as far as `along` reaches.
    pub fn ray(&mut self, from: Vec3, along: Vec3, color: Color) {
        self.line(from, from + along, color);
    }

    /// A line with a head at `to`.
    pub fn arrow(&mut self, from: Vec3, to: Vec3, color: Color) {
        self.line(from, to, color);
        let along = to - from;
        let length = along.length();
        if length < 1e-6 {
            return;
        }
        let forward = along / length;
        let side = forward.any_orthonormal_vector();
        let head = (length * 0.25).min(0.3);
        for side in [side, -side, forward.cross(side), -forward.cross(side)] {
            self.line(to, to - forward * head + side * head * 0.4, color);
        }
    }

    /// Lines through each point in turn; `closed` joins the last to the first.
    pub fn path(&mut self, points: &[Vec3], closed: bool, color: Color) {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1], color);
        }
        if let (true, [first, .., last]) = (closed, points) {
            self.line(*last, *first, color);
        }
    }

    /// The three axes of `transform`, `size` long: x red, y green, z blue.
    pub fn axes(&mut self, transform: Mat4, size: f32) {
        let origin = transform.transform_point3(Vec3::ZERO);
        for (axis, color) in [
            (Vec3::X, Color::rgb(0.9, 0.2, 0.2)),
            (Vec3::Y, Color::rgb(0.3, 0.8, 0.2)),
            (Vec3::Z, Color::rgb(0.2, 0.4, 0.95)),
        ] {
            self.line(origin, origin + transform.transform_vector3(axis).normalize_or_zero() * size, color);
        }
    }

    /// The edges of a box `half` to each side of the origin of `transform`.
    pub fn cuboid(&mut self, transform: Mat4, half: Vec3, color: Color) {
        let corner = |i: usize| {
            let sign = |bit: usize| if i & bit == 0 { -1.0 } else { 1.0 };
            transform.transform_point3(half * Vec3::new(sign(1), sign(2), sign(4)))
        };
        for i in 0..8 {
            for bit in [1, 2, 4] {
                if i & bit == 0 {
                    self.line(corner(i), corner(i | bit), color);
                }
            }
        }
    }

    /// The edges of the box from `min` to `max`, along the world's axes.
    pub fn bounds(&mut self, min: Vec3, max: Vec3, color: Color) {
        self.cuboid(Mat4::from_translation((min + max) * 0.5), (max - min) * 0.5, color);
    }

    /// A circle about `centre`, lying across `normal`.
    pub fn circle(&mut self, centre: Vec3, normal: Vec3, radius: f32, color: Color) {
        self.arc(centre, normal, radius, AROUND, AROUND, None, color);
    }

    /// Part of a circle: `steps` of `AROUND` segments, starting from `start` (any direction
    /// across `normal`; one is chosen when none is given).
    #[allow(clippy::too_many_arguments)]
    fn arc(&mut self, centre: Vec3, normal: Vec3, radius: f32, steps: usize, of: usize, start: Option<Vec3>, color: Color) {
        let normal = normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        let u = start.map_or_else(|| normal.any_orthonormal_vector(), |start| start.normalize_or_zero());
        let v = normal.cross(u);
        let at = |i: usize| {
            let angle = i as f32 / of as f32 * std::f32::consts::TAU;
            centre + (u * angle.cos() + v * angle.sin()) * radius
        };
        for i in 0..steps {
            self.line(at(i), at(i + 1), color);
        }
    }

    /// A ball as three circles, one across each axis of `rotation`.
    pub fn sphere(&mut self, centre: Vec3, rotation: Quat, radius: f32, color: Color) {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.circle(centre, rotation * axis, radius, color);
        }
    }

    /// A capsule along the local y of `rotation`: a segment `half_height` either side of
    /// `centre`, swept by `radius`.
    pub fn capsule(&mut self, centre: Vec3, rotation: Quat, half_height: f32, radius: f32, color: Color) {
        let (x, up, z) = (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z);
        let (top, bottom) = (centre + up * half_height, centre - up * half_height);
        self.circle(top, up, radius, color);
        self.circle(bottom, up, radius, color);
        for side in [x, -x, z, -z] {
            self.line(top + side * radius, bottom + side * radius, color);
        }
        // The caps: a half circle over each end, in both upright planes.
        for (across, start) in [(z, x), (x, -z)] {
            self.arc(top, across, radius, AROUND / 2, AROUND, Some(start), color);
            self.arc(bottom, across, radius, AROUND / 2, AROUND, Some(-start), color);
        }
    }

    /// A square grid of lines on the plane through `centre` across `normal`, `cells` squares
    /// of `cell` to each side.
    pub fn grid(&mut self, centre: Vec3, normal: Vec3, cell: f32, cells: u32, color: Color) {
        let normal = normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        let u = normal.any_orthonormal_vector();
        let v = normal.cross(u);
        let reach = cell * cells as f32;
        for i in 0..=cells * 2 {
            let across = i as f32 * cell - reach;
            self.line(centre + u * across - v * reach, centre + u * across + v * reach, color);
            self.line(centre + v * across - u * reach, centre + v * across + u * reach, color);
        }
    }

    /// How many lines are waiting to be drawn.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Forgets the lines added so far this frame.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Both ends of every line waiting, for tests and tools.
    pub fn segments(&self) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
        self.lines.iter().map(|line| (Vec3::from(line.from), Vec3::from(line.to)))
    }
}

/// Must match `View` in `debug_lines.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
    size: [f32; 4],
}

pub(crate) struct LineRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// How many lines `instances` has room for.
    room: usize,
}

impl LineRenderer {
    fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = crate::shader!("debug_lines.wgsl").module(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug lines layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: true,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Depth,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug lines"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Line>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines view"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let room = 1024;
        Self { pipeline, layout, uniform, instances: Self::buffer(device, room), room }
    }

    fn buffer(device: &wgpu::Device, room: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines"),
            size: (room * std::mem::size_of::<Line>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

/// The overlay that draws the frame's lines and forgets them.
pub(crate) fn draw(world: &mut World, target: &wgpu::TextureView) {
    let (lines, shown, through) = {
        let lines = world.resource_mut::<DebugLines>();
        (std::mem::take(&mut lines.lines), lines.shown, lines.through)
    };
    if lines.is_empty() || !shown {
        return;
    }
    if !world.contains_resource::<LineRenderer>() {
        let renderer = LineRenderer::new(world.resource::<Gpu>());
        world.insert_resource(renderer);
    }
    let view_proj = world.resource::<RenderFrame>().unjittered_view_proj;
    world.resource_scope(|world, renderer: &mut LineRenderer| {
        let gpu = world.resource::<Gpu>();
        if lines.len() > renderer.room {
            renderer.room = lines.len().next_power_of_two();
            renderer.instances = LineRenderer::buffer(&gpu.device, renderer.room);
        }
        gpu.queue.write_buffer(&renderer.instances, 0, bytemuck::cast_slice(&lines));
        let size = [gpu.config.width as f32, gpu.config.height as f32, through, 0.0];
        gpu.queue.write_buffer(
            &renderer.uniform,
            0,
            bytemuck::bytes_of(&ViewUniform { view_proj: view_proj.to_cols_array_2d(), size }),
        );
        // The depth buffer is made again whenever the frame changes size, so it is bound anew.
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug lines"),
            layout: &renderer.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: renderer.uniform.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&gpu.targets.depth),
                },
            ],
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("debug lines") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debug lines"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&renderer.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, renderer.instances.slice(..));
            pass.draw(0..6, 0..lines.len() as u32);
        }
        gpu.queue.submit([encoder.finish()]);
    });
    // Hand the list's memory back for the next frame.
    let mut lines = lines;
    lines.clear();
    let kept = world.resource_mut::<DebugLines>();
    if kept.lines.is_empty() {
        kept.lines = lines;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_has_twelve_edges_each_as_long_as_its_side() {
        let mut lines = DebugLines::default();
        lines.cuboid(Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)), Vec3::new(1.0, 2.0, 3.0), Color::WHITE);
        assert_eq!(lines.len(), 12);
        let mut lengths: Vec<f32> = lines.segments().map(|(a, b)| a.distance(b)).collect();
        lengths.sort_by(f32::total_cmp);
        assert_eq!(lengths, [2.0, 2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0, 6.0, 6.0, 6.0, 6.0]);
        assert!(lines.segments().all(|(a, b)| (a.x - 3.0).abs() <= 1.0 && (b.x - 3.0).abs() <= 1.0));
    }

    #[test]
    fn a_circle_closes_and_stays_at_its_radius_across_its_normal() {
        let mut lines = DebugLines::default();
        let centre = Vec3::new(1.0, 2.0, 3.0);
        lines.circle(centre, Vec3::Y, 2.5, Color::WHITE);
        let segments: Vec<_> = lines.segments().collect();
        assert_eq!(segments.len(), AROUND);
        for (i, (a, b)) in segments.iter().enumerate() {
            assert!((a.distance(centre) - 2.5).abs() < 1e-4);
            assert!((a.y - 2.0).abs() < 1e-4);
            let next = segments[(i + 1) % segments.len()].0;
            assert!(b.distance(next) < 1e-4, "segment {i} does not meet the next");
        }
    }

    #[test]
    fn a_capsule_reaches_its_ends_and_no_further() {
        let mut lines = DebugLines::default();
        lines.capsule(Vec3::ZERO, Quat::IDENTITY, 1.0, 0.5, Color::WHITE);
        let highest = lines.segments().map(|(a, b)| a.y.max(b.y)).fold(f32::MIN, f32::max);
        let lowest = lines.segments().map(|(a, b)| a.y.min(b.y)).fold(f32::MAX, f32::min);
        assert!((highest - 1.5).abs() < 1e-4, "top at {highest}");
        assert!((lowest + 1.5).abs() < 1e-4, "bottom at {lowest}");
        let widest = lines.segments().map(|(a, _)| a.x.abs().max(a.z.abs())).fold(0.0, f32::max);
        assert!((widest - 0.5).abs() < 1e-4);
    }
}
