//! Motion vectors for what moves. Temporal anti-aliasing finds last frame's colour for a
//! pixel from the depth there and how the camera moved, which is right for everything that
//! stands still. A thing that moved was somewhere else; this pass draws those things again,
//! writing how far each has come across the picture, and the resolve looks back along that.
//!
//! Only meshes whose place changed since the last frame are drawn, so a still scene pays
//! nothing. Meshes bent by a skeleton move as their entity does and no more.

use glam::Mat4;

use super::{
    gpu::{Gpu, DEPTH_COMPARE, DEPTH_FORMAT, MOTION_CLEAR, MOTION_FORMAT, MSAA_SAMPLES},
    mesh::Vertex,
    renderer::MeshRenderer,
    RenderFrame,
};

/// A mesh that is not where it was a frame ago.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Moved {
    pub mesh: u32,
    pub model: Mat4,
    pub was: Mat4,
}

/// Whether a thing has moved enough to be worth the drawing.
pub(crate) fn has_moved(model: &Mat4, was: &Mat4) -> bool {
    !model.abs_diff_eq(*was, 1e-5)
}

/// Must match the instance inputs of `motion.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    model: [[f32; 4]; 4],
    was: [[f32; 4]; 4],
}

/// Must match `Motion` in `motion.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MotionUniform {
    view_proj: [[f32; 4]; 4],
    now: [[f32; 4]; 4],
    before: [[f32; 4]; 4],
}

pub(crate) struct MotionRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    room: usize,
}

impl MotionRenderer {
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = crate::shader!("motion.wgsl").module(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("motion"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("motion"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(Vertex::layout()),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
                            7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4,
                        ],
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MOTION_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            // Against the depth the main pass left, without changing it; pulled a little
            // toward the eye (depth is reversed) so a surface passes against itself.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(DEPTH_COMPARE),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 8, slope_scale: 1.5, clamp: 0.0 },
            }),
            multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("motion view"),
            size: std::mem::size_of::<MotionUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let room = 256;
        Self { pipeline, bind_group, uniform, instances: Self::buffer(device, room), room }
    }

    fn buffer(device: &wgpu::Device, room: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("motion instances"),
            size: (room * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Draws how far each moved mesh has come. Returns whether anything was drawn: if not,
    /// the target was left alone and holds nothing to read.
    pub(crate) fn render(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, meshes: &MeshRenderer, frame: &RenderFrame) -> bool {
        if frame.moved.is_empty() {
            return false;
        }
        if frame.moved.len() > self.room {
            self.room = frame.moved.len().next_power_of_two();
            self.instances = Self::buffer(&gpu.device, self.room);
        }
        let instances: Vec<Instance> = frame
            .moved
            .iter()
            .map(|moved| Instance { model: moved.model.to_cols_array_2d(), was: moved.was.to_cols_array_2d() })
            .collect();
        gpu.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        gpu.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&MotionUniform {
                view_proj: frame.view_proj.to_cols_array_2d(),
                now: frame.unjittered_view_proj.to_cols_array_2d(),
                before: frame.previous_view_proj.to_cols_array_2d(),
            }),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gpu.targets.motion,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(MOTION_CLEAR), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &gpu.targets.depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for (at, moved) in frame.moved.iter().enumerate() {
            let Some((vertices, indices, count)) = meshes.buffers(moved.mesh) else {
                continue;
            };
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..count, 0, at as u32..at as u32 + 1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn a_thing_has_moved_when_its_place_or_turn_has_changed() {
        let here = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert!(!has_moved(&here, &here));
        assert!(has_moved(&Mat4::from_translation(Vec3::new(1.0, 2.001, 3.0)), &here));
        assert!(has_moved(&(here * Mat4::from_rotation_y(0.01)), &here));
    }
}
