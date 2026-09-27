//! Image-based lighting from an equirectangular HDR sky.
//!
//! The sky picture is used three ways: drawn as the background, sampled at blurrier mip levels
//! for glossy reflections and haze, and projected onto spherical harmonics for the soft light
//! coming from every direction. The sun is found in the image and cut out of it, then handed
//! to a `DirectionalLight` so it can cast shadows, with its measured strength.

use std::{
    f32::consts::{PI, TAU},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Context;
use glam::{Quat, Vec2, Vec3};
use half::f16;

/// Cut-out radius around the sun, in radians. The visible disk is 0.27°; the extra takes the
/// glare around it, which in a real camera is part of what lights the scene.
const SUN_RADIUS: f32 = 0.06;

/// Tells skies apart, so the renderer notices when the resource is replaced.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// An HDR sky, prepared for rendering. Insert it as a resource to replace the default sky.
pub struct Environment {
    /// Linear RGB radiance, full resolution first, each level half the last.
    pub(crate) mips: Vec<(u32, u32, Vec<[f16; 4]>)>,
    /// Irradiance divided by π as order-2 spherical harmonics, so diffuse light is
    /// `albedo * Σ sh[i] * Y_i(normal)`. The sun is excluded.
    pub sh: [Vec3; 9],
    /// Direction toward the sun.
    pub sun_direction: Vec3,
    /// The sun's irradiance on a surface facing it, linear RGB.
    pub sun_illuminance: Vec3,
    /// Turns the whole sky around the vertical axis, in radians.
    pub rotation: f32,
    /// Multiplies the sky's brightness, background and lighting alike.
    pub intensity: f32,
    pub(crate) generation: u64,
}

pub fn direction_to_uv(d: Vec3) -> Vec2 {
    Vec2::new(0.5 + d.x.atan2(-d.z) / TAU, d.y.clamp(-1.0, 1.0).acos() / PI)
}

pub fn uv_to_direction(uv: Vec2) -> Vec3 {
    let phi = (uv.x - 0.5) * TAU;
    let theta = uv.y * PI;
    Vec3::new(
        theta.sin() * phi.sin(),
        theta.cos(),
        -theta.sin() * phi.cos(),
    )
}

fn sh_basis(d: Vec3) -> [f32; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

impl Environment {
    /// Loads a Radiance `.hdr` sky. `rotation` turns it about the vertical axis so its sun can
    /// be placed where the scene wants it.
    pub fn from_hdr(bytes: &[u8], rotation: f32) -> anyhow::Result<Self> {
        Self::decode(bytes, Orientation::Rotation(rotation))
    }

    /// Loads a Radiance `.hdr` sky turned so its sun lies in the horizontal direction
    /// `sun_toward`, whatever direction it was photographed in.
    pub fn from_hdr_with_sun(bytes: &[u8], sun_toward: Vec3) -> anyhow::Result<Self> {
        Self::decode(bytes, Orientation::SunToward(sun_toward))
    }

    fn decode(bytes: &[u8], orientation: Orientation) -> anyhow::Result<Self> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Hdr)
            .context("not a Radiance HDR image")?
            .to_rgb32f();
        let (width, height) = image.dimensions();
        let pixels: Vec<Vec3> = image
            .pixels()
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();
        Ok(Self::from_pixels(width, height, pixels, orientation))
    }

    /// A plain blue-to-haze gradient with a sun, for when no sky image is given.
    pub fn gradient(sun_direction: Vec3) -> Self {
        let (width, height) = (256, 128);
        let sun = sun_direction.normalize();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let uv = Vec2::new((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
                let d = uv_to_direction(uv);
                let up = d.y.max(0.0);
                let horizon = Vec3::new(0.75, 0.85, 1.0);
                let zenith = Vec3::new(0.25, 0.45, 0.95);
                let ground = Vec3::new(0.25, 0.23, 0.2);
                let mut c = if d.y >= 0.0 {
                    horizon.lerp(zenith, up.powf(0.5))
                } else {
                    horizon.lerp(ground, (-d.y * 4.0).min(1.0))
                };
                c += Vec3::new(1.0, 0.9, 0.7) * d.dot(sun).max(0.0).powf(16.0) * 2.0;
                if d.dot(sun) > (0.02f32).cos() {
                    c = Vec3::splat(8_000.0);
                }
                pixels.push(c * 0.35);
            }
        }
        Self::from_pixels(width, height, pixels, Orientation::Rotation(0.0))
    }

    fn from_pixels(width: u32, height: u32, mut pixels: Vec<Vec3>, orientation: Orientation) -> Self {
        let pixel_direction = |x: u32, y: u32| {
            uv_to_direction(Vec2::new(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ))
        };
        let solid_angle = |y: u32| {
            let theta = (y as f32 + 0.5) / height as f32 * PI;
            (TAU / width as f32) * (PI / height as f32) * theta.sin()
        };
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));

        // The sun is the brightest thing in the sky.
        let brightest = (0..pixels.len())
            .max_by(|a, b| luminance(pixels[*a]).total_cmp(&luminance(pixels[*b])))
            .unwrap_or(0) as u32;
        let sun = pixel_direction(brightest % width, brightest / width);
        // Rotation about Y adds to a direction's angle atan2(x, z).
        let rotation = match orientation {
            Orientation::Rotation(r) => r,
            Orientation::SunToward(t) => t.x.atan2(t.z) - sun.x.atan2(sun.z),
        };

        // Measure the light the sun region delivers above its surroundings, then replace it
        // with the surrounding sky so lighting doesn't count it twice.
        let (mut ring, mut ring_count) = (Vec3::ZERO, 0.0);
        for y in 0..height {
            for x in 0..width {
                let angle = pixel_direction(x, y).dot(sun).clamp(-1.0, 1.0).acos();
                if (SUN_RADIUS..SUN_RADIUS * 1.5).contains(&angle) {
                    ring += pixels[(y * width + x) as usize];
                    ring_count += 1.0;
                }
            }
        }
        let ring = ring / f32::max(ring_count, 1.0);
        let mut sun_illuminance = Vec3::ZERO;
        for y in 0..height {
            for x in 0..width {
                let d = pixel_direction(x, y);
                let cos = d.dot(sun);
                if cos.clamp(-1.0, 1.0).acos() < SUN_RADIUS {
                    let p = &mut pixels[(y * width + x) as usize];
                    sun_illuminance += (*p - ring).max(Vec3::ZERO) * solid_angle(y) * cos;
                    *p = p.min(ring.max(Vec3::splat(0.0)) * 1.5);
                }
            }
        }

        let mips = build_mips(width, height, &pixels);

        // Project a small copy onto spherical harmonics, convolved with the cosine lobe.
        let (sw, sh_h, small) = mips
            .iter()
            .find(|(w, _, _)| *w <= 256)
            .map(|(w, h, p)| (*w, *h, p))
            .unwrap_or_else(|| {
                let last = mips.last().unwrap();
                (last.0, last.1, &last.2)
            });
        let turn = Quat::from_rotation_y(rotation);
        let mut sh = [Vec3::ZERO; 9];
        for y in 0..sh_h {
            let theta = (y as f32 + 0.5) / sh_h as f32 * PI;
            let weight = (TAU / sw as f32) * (PI / sh_h as f32) * theta.sin();
            for x in 0..sw {
                let d = turn
                    * uv_to_direction(Vec2::new(
                        (x as f32 + 0.5) / sw as f32,
                        (y as f32 + 0.5) / sh_h as f32,
                    ));
                let p = small[(y * sw + x) as usize];
                let c = Vec3::new(p[0].to_f32(), p[1].to_f32(), p[2].to_f32());
                for (i, basis) in sh_basis(d).iter().enumerate() {
                    sh[i] += c * *basis * weight;
                }
            }
        }
        // Cosine convolution (π, 2π/3, π/4 per band), then divide by π for Lambert.
        for (i, c) in sh.iter_mut().enumerate() {
            let band = match i {
                0 => PI,
                1..=3 => TAU / 3.0,
                _ => PI / 4.0,
            };
            *c *= band / PI;
        }

        let sun_direction = turn * sun;
        log::info!(
            "sky: sun {:.0}° up, illuminance {:.1}, sky irradiance {:.1}",
            sun_direction.y.asin().to_degrees(),
            luminance(sun_illuminance),
            luminance(sh[0] * 0.282_095 + sh[1] * 0.488_603) * PI,
        );
        Self {
            mips,
            sh,
            sun_direction,
            sun_illuminance,
            rotation,
            intensity: 1.0,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn width(&self) -> u32 {
        self.mips[0].0
    }
}

fn build_mips(width: u32, height: u32, pixels: &[Vec3]) -> Vec<(u32, u32, Vec<[f16; 4]>)> {
    let to_half = |c: Vec3| {
        let c = c.min(Vec3::splat(60_000.0));
        [f16::from_f32(c.x), f16::from_f32(c.y), f16::from_f32(c.z), f16::ONE]
    };
    let mut mips = vec![(width, height, pixels.iter().map(|c| to_half(*c)).collect())];
    let mut current: Vec<Vec3> = pixels.to_vec();
    let (mut w, mut h) = (width, height);
    while w > 8 && h > 4 {
        let (nw, nh) = (w / 2, h / 2);
        let mut next = Vec::with_capacity((nw * nh) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let at = |dx: u32, dy: u32| current[((y * 2 + dy) * w + x * 2 + dx) as usize];
                next.push((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) * 0.25);
            }
        }
        mips.push((nw, nh, next.iter().map(|c| to_half(*c)).collect()));
        current = next;
        (w, h) = (nw, nh);
    }
    mips
}

/// The sky on the GPU.
pub(crate) struct GpuEnvironment {
    pub view: wgpu::TextureView,
    pub mip_count: u32,
    pub generation: u64,
}

impl GpuEnvironment {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, env: &Environment) -> Self {
        let (width, height, _) = &env.mips[0];
        let mip_count = env.mips.len() as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment"),
            size: wgpu::Extent3d {
                width: *width,
                height: *height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, data)) in env.mips.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 8),
                    rows_per_image: Some(*h),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
            );
        }
        Self {
            view: texture.create_view(&Default::default()),
            mip_count,
            generation: env.generation,
        }
    }
}

enum Orientation {
    Rotation(f32),
    SunToward(Vec3),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_round_trips() {
        for d in [Vec3::X, Vec3::NEG_Z, Vec3::new(0.3, 0.5, 0.8).normalize(), Vec3::new(-0.6, -0.2, 0.1).normalize()] {
            let back = uv_to_direction(direction_to_uv(d));
            assert!((back - d).length() < 1e-4, "{d} came back as {back}");
        }
    }

    #[test]
    fn gradient_sky_finds_its_sun() {
        let sun = Vec3::new(-0.5, 0.6, 0.4).normalize();
        let env = Environment::gradient(sun);
        assert!(env.sun_direction.dot(sun) > 0.999);
        assert!(env.sun_illuminance.x > 0.0);
        // More light from above than below.
        assert!(env.sh[1].y > 0.0);
    }
}
