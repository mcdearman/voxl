//! Cascaded shadow maps for the sun.
//!
//! The view is cut into slices by distance, and each slice gets its own shadow map covering a
//! sphere around it: detailed shadows near the camera, coarser ones further off. Spheres don't
//! change size as the camera turns, and their centres snap to whole shadow texels, so shadow
//! edges stay still instead of crawling.

use glam::{Mat4, Vec3, Vec4Swizzles};

use super::{gpu::Gpu, DEPTH_FORMAT};

pub const CASCADES: usize = 4;
/// Uniform buffer offsets for dynamic bindings must be 256-byte aligned.
const SLOT: u64 = 256;

#[derive(Clone, Copy, Debug)]
pub struct ShadowSettings {
    pub enabled: bool,
    /// Width and height of each cascade's map.
    pub resolution: u32,
    /// Shadows end (fading out) at this distance from the camera.
    pub max_distance: f32,
    /// Where the first cascade ends; later splits grow geometrically to `max_distance`.
    pub first_split: f32,
    /// How far toward the sun beyond each cascade's sphere casters are still caught.
    pub caster_reach: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            resolution: 2048,
            max_distance: 300.0,
            first_split: 10.0,
            caster_reach: 400.0,
        }
    }
}

/// Where each cascade's shadow map sits in the world, for one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct CascadeData {
    pub matrices: [Mat4; CASCADES],
    pub splits: [f32; 4],
    pub texel_sizes: [f32; 4],
}

impl CascadeData {
    /// `camera` is the camera's world transform; `tan_half_fov` is the tangent of half the
    /// vertical field of view. `to_sun` is the direction toward the sun.
    pub fn compute(
        settings: &ShadowSettings,
        camera: Mat4,
        tan_half_fov: f32,
        aspect: f32,
        near: f32,
        to_sun: Vec3,
    ) -> Self {
        let to_sun = to_sun.normalize();
        let position = camera.w_axis.xyz();
        let forward = -camera.z_axis.xyz().normalize();
        // Lateral extent per unit of depth, out to the frustum's corners.
        let spread = (tan_half_fov * tan_half_fov * (1.0 + aspect * aspect)).sqrt();
        let up = if to_sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let light_view = Mat4::look_at_rh(Vec3::ZERO, -to_sun, up);

        let mut data = Self::default();
        let ratio = (settings.max_distance / settings.first_split).powf(1.0 / (CASCADES - 1) as f32);
        let mut slice_near = near;
        for i in 0..CASCADES {
            let slice_far = settings.first_split * ratio.powi(i as i32);
            // The smallest sphere around the slice depends only on its depth range and the
            // field of view, never on where the camera looks.
            let s2 = spread * spread;
            let center_depth = ((slice_far + slice_near) * 0.5 * (1.0 + s2)).min(slice_far);
            let radius = ((slice_far - center_depth).powi(2) + (slice_far * spread).powi(2)).sqrt();
            let radius = (radius * 16.0).ceil() / 16.0;
            let center = position + forward * center_depth;

            let texel = radius * 2.0 / settings.resolution as f32;
            let mut c = light_view.transform_point3(center);
            c.x = (c.x / texel).floor() * texel;
            c.y = (c.y / texel).floor() * texel;
            // Light space looks down -Z: nearer the sun is larger z.
            let projection = Mat4::orthographic_rh(
                c.x - radius,
                c.x + radius,
                c.y - radius,
                c.y + radius,
                -c.z - radius - settings.caster_reach,
                -c.z + radius,
            );
            data.matrices[i] = projection * light_view;
            data.splits[i] = slice_far;
            data.texel_sizes[i] = texel;
            slice_near = slice_far;
        }
        data
    }
}

pub struct ShadowMaps {
    pub resolution: u32,
    pub layer_views: Vec<wgpu::TextureView>,
    pub array_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    buffer: wgpu::Buffer,
    /// Group 0 for shadow pipelines: the cascade's light matrix, with a dynamic offset.
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl ShadowMaps {
    pub fn new(gpu: &Gpu, resolution: u32) -> Self {
        let device = &gpu.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: CASCADES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layer_views = (0..CASCADES as u32)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow cascades"),
            size: SLOT * CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow view"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });
        Self {
            resolution,
            layer_views,
            array_view,
            sampler,
            buffer,
            layout,
            bind_group,
        }
    }

    pub fn write(&self, gpu: &Gpu, cascades: &CascadeData) {
        for (i, m) in cascades.matrices.iter().enumerate() {
            gpu.queue
                .write_buffer(&self.buffer, SLOT * i as u64, bytemuck::bytes_of(m));
        }
    }

    pub fn offset(cascade: usize) -> u32 {
        (SLOT * cascade as u64) as u32
    }
}

/// Depth-stencil state for a shadow pipeline. The slope-scaled bias keeps grazing surfaces from
/// shadowing themselves.
pub fn shadow_depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: true,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: Default::default(),
        bias: wgpu::DepthBiasState {
            constant: 1,
            slope_scale: 1.5,
            clamp: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascades_cover_what_the_camera_sees_and_ignore_where_it_looks() {
        let settings = ShadowSettings::default();
        let sun = Vec3::new(-0.5, 0.7, 0.4);
        let a = CascadeData::compute(&settings, Mat4::IDENTITY, 0.5, 1.6, 0.1, sun);
        let turned = Mat4::from_rotation_y(1.0);
        let b = CascadeData::compute(&settings, turned, 0.5, 1.6, 0.1, sun);
        for i in 0..CASCADES {
            assert!((a.texel_sizes[i] - b.texel_sizes[i]).abs() < 1e-5, "cascade {i} changed size");
        }
        assert!((a.splits[3] - settings.max_distance).abs() < 1e-3);
        // A point in front of the camera, inside the first slice, lands inside cascade 0.
        let p = Vec3::new(0.5, -0.3, -5.0);
        let clip = a.matrices[0] * p.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z), "{ndc}");
    }
}
