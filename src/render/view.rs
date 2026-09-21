use super::{gpu::Gpu, RenderFrame};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
    camera_position: [f32; 4],
    light_direction: [f32; 4],
    light_color: [f32; 4],
    ambient_color: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
}

/// Camera and lighting data shared by every pipeline. Always bound at group 0; the matching
/// WGSL struct is `View` in the shaders.
pub struct ViewBinding {
    buffer: wgpu::Buffer,
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl ViewBinding {
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view uniform"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view bind group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            layout,
            bind_group,
        }
    }

    pub fn write(&self, gpu: &Gpu, frame: &RenderFrame) {
        let view = ViewUniform {
            view_proj: frame.view_proj.to_cols_array_2d(),
            camera_position: frame.camera_position.extend(1.0).into(),
            light_direction: frame.light_direction.extend(0.0).into(),
            light_color: frame.light_color.extend(1.0).into(),
            ambient_color: frame.ambient_color.extend(1.0).into(),
            fog_color: frame.clear_color.to_array(),
            fog: [frame.fog.start, frame.fog.end, 0.0, 0.0],
        };
        gpu.queue
            .write_buffer(&self.buffer, 0, bytemuck::bytes_of(&view));
    }
}
