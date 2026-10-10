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
    /// For a built-in sky, where the sun it was made for is.
    follows: Option<Vec3>,
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

/// The angle from its middle within which the built-in sky draws its sun, in radians.
const SUN_DISK: f32 = 0.02;

/// How much of each colour the clear air overhead scatters out of a beam (optical depths
/// of red, green and blue light), and the same for the dust and droplets in it.
const AIR: Vec3 = Vec3::new(0.046, 0.108, 0.265);
const DUST: f32 = 0.025;

/// Light scattered more than once fills the sky in; this stands in for it.
const SCATTERED_AGAIN: f32 = 2.2;

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// How many thicknesses of air a ray crosses at this height above the horizon (the sine of
/// the angle): one straight up, about 38 along the ground. Kasten and Young's fit.
fn air_mass(up: f32) -> f32 {
    let up = up.clamp(0.0, 1.0);
    let from_zenith = up.acos().to_degrees();
    1.0 / (up + 0.505_72 * (96.079_95 - from_zenith).powf(-1.636_4))
}

/// The light above the air, set so that a sun half way up the sky delivers 3 at the ground.
fn sun_above_the_air() -> Vec3 {
    let through = (-(AIR + DUST) * air_mass(std::f32::consts::FRAC_1_SQRT_2)).exp();
    Vec3::splat(3.0 / luminance(through))
}

/// The sun's light on a surface facing it at the ground: less, and redder, the lower it is.
fn sunlight(sun: Vec3) -> Vec3 {
    // A sun below the horizon still lights the air above for a while.
    let fade = ((sun.y + 0.1) / 0.1).clamp(0.0, 1.0);
    sun_above_the_air() * (-(AIR + DUST) * air_mass(sun.y)).exp() * fade
}

/// The light of a clear sky from direction `d` (at or above the horizon): sunlight scattered
/// once on its way down, by the air evenly and by dust mostly forward.
fn sky_radiance(d: Vec3, sun: Vec3) -> Vec3 {
    let toward = d.dot(sun).clamp(-1.0, 1.0);
    let by_air = 3.0 / (16.0 * PI) * (1.0 + toward * toward);
    let g = 0.76f32;
    let by_dust = (1.0 - g * g) / (4.0 * PI * (1.0 + g * g - 2.0 * g * toward).powf(1.5));
    let depth = AIR + DUST;
    let scattered = (AIR * by_air + Vec3::splat(DUST * by_dust)) / depth;
    let reached = Vec3::ONE - (-depth * air_mass(d.y)).exp();
    let mut day = sunlight(sun) * scattered * reached * SCATTERED_AGAIN;
    // Toward the horizon the light has been scattered many times over: brighter than once
    // would make it, and nearly white.
    let low = (1.0 - d.y.clamp(0.0, 1.0)).powi(5);
    day = day.lerp(Vec3::splat(luminance(day)) * Vec3::new(0.95, 1.0, 1.08), low * 0.6) * (1.0 + low * 0.9);
    // What is left when the sun has gone.
    day + Vec3::new(0.002, 0.003, 0.006)
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

    /// The built-in sky, for when no sky image is given: a clear day with the sun in this
    /// direction. See [`Environment::clear_sky`].
    pub fn gradient(sun_direction: Vec3) -> Self {
        Self::clear_sky(sun_direction)
    }

    /// A clear sky worked out from where the sun is: deep blue overhead, pale toward the
    /// horizon, bright and warm about the sun, red when it is low, and a sun whose light is
    /// the colour the air has left it. Its strength suits a `DirectionalLight` of about 3.
    ///
    /// A sky made this way follows the sun: while it is the world's sky and the scene's
    /// directional light turns, it is made again to match.
    pub fn clear_sky(sun_direction: Vec3) -> Self {
        let (width, height) = (256, 128);
        let sun = sun_direction.normalize_or(Vec3::Y);
        let sunlight = sunlight(sun);
        let disk = sunlight / (PI * SUN_DISK * SUN_DISK);
        // The ground far off, lit by that sun and sky; nearer the horizon it is lost in haze.
        let ground = Vec3::new(0.2, 0.19, 0.17) * (luminance(sunlight) * sun.y.max(0.0) + 0.5) / PI;
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let uv = Vec2::new((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
                let d = uv_to_direction(uv);
                let mut c = if d.y >= 0.0 {
                    sky_radiance(d, sun)
                } else {
                    let level = Vec3::new(d.x, 0.0, d.z).normalize_or(Vec3::X);
                    let below = (-d.y / 0.3).clamp(0.0, 1.0);
                    sky_radiance(level, sun).lerp(ground, below * below * (3.0 - 2.0 * below))
                };
                if d.dot(sun) > SUN_DISK.cos() {
                    c = disk;
                }
                pixels.push(c);
            }
        }
        let mut sky = Self::from_pixels(width, height, pixels, Orientation::Rotation(0.0));
        // The picture is too coarse to measure so small a sun from; what it was drawn
        // from is known exactly.
        sky.sun_direction = sun;
        sky.sun_illuminance = sunlight;
        sky.follows = Some(sun);
        sky
    }

    /// The direction of the sun a built-in sky was made for; none for a sky from a picture.
    pub fn follows(&self) -> Option<Vec3> {
        self.follows
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
            follows: None,
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
    fn a_clear_sky_is_blue_overhead_and_its_sun_reddens_as_it_sinks() {
        let noon = Vec3::new(1.0, 1.0, 0.0).normalize();
        let overhead = sky_radiance(Vec3::Y, noon);
        assert!(overhead.z > overhead.y && overhead.y > overhead.x, "overhead is {overhead}");
        // Paler and brighter toward the horizon, as far from the sun.
        let level = sky_radiance(Vec3::new(1.0, 0.02, 0.0).normalize(), noon);
        assert!(luminance(level) > luminance(overhead));
        assert!(level.x / level.z > overhead.x / overhead.z);
        // The sun half way up delivers 3; lower, less of it and less blue in it.
        let half = sunlight(Vec3::new(1.0, 1.0, 0.0).normalize());
        assert!((luminance(half) - 3.0).abs() < 0.01);
        let low = sunlight(Vec3::new(1.0, 0.08, 0.0).normalize());
        assert!(luminance(low) < luminance(half) * 0.5);
        assert!(low.z / low.x < half.z / half.x * 0.5);
        assert_eq!(sunlight(Vec3::new(1.0, -0.3, 0.0).normalize()), Vec3::ZERO);
    }

    #[test]
    fn a_built_in_sky_remembers_the_sun_it_was_made_for() {
        let sun = Vec3::new(0.3, 0.5, -0.4).normalize();
        assert_eq!(Environment::clear_sky(sun).follows(), Some(sun));
    }

    #[test]
    fn gradient_sky_finds_its_sun() {
        let sun = Vec3::new(-0.5, 0.6, 0.4).normalize();
        let env = Environment::gradient(sun);
        assert!(env.sun_direction.dot(sun) > 0.999);
        assert!(env.sun_illuminance.x > 0.0);
        // The light from above is bluer than the light from the ground below.
        assert!(env.sh[1].z > env.sh[1].x);
    }
}
