//! The photographic materials everything in the world is textured with.
//!
//! Most are Poly Haven scans (CC0). Two are made here: straw, recoloured from the grass scan,
//! and window glass.

use std::path::{Path, PathBuf};

use anyhow::Context;
use voxl::render::Image;

pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/res/napoleonic");

pub fn asset(path: &str) -> PathBuf {
    Path::new(ASSETS).join(path)
}

/// Texture array layers. The first six are the ground layers terrain vertices blend.
pub const GRASS: u32 = 0;
pub const STRAW: u32 = 1;
pub const SOIL: u32 = 2;
pub const ROAD: u32 = 3;
pub const FOREST_FLOOR: u32 = 4;
pub const MUD: u32 = 5;
pub const STONE: u32 = 6;
pub const PLASTER: u32 = 7;
pub const SLATE: u32 = 8;
pub const TILES: u32 = 9;
pub const WOOD: u32 = 10;
pub const GLASS: u32 = 11;
pub const CANVAS: u32 = 12;

struct Layer {
    scan: Option<&'static str>,
    /// Metres covered by one repeat of the texture.
    scale: f32,
    roughness: f32,
    normal_strength: f32,
    hide_tiling: bool,
}

const LAYERS: [Layer; 13] = [
    Layer { scan: Some("aerial_grass_rock"), scale: 3.5, roughness: 1.0, normal_strength: 1.0, hide_tiling: true },
    Layer { scan: None, scale: 3.5, roughness: 1.0, normal_strength: 0.8, hide_tiling: true },
    Layer { scan: Some("farm_soil"), scale: 4.0, roughness: 1.0, normal_strength: 1.2, hide_tiling: true },
    Layer { scan: Some("grass_path_2"), scale: 4.0, roughness: 1.0, normal_strength: 1.0, hide_tiling: true },
    Layer { scan: Some("forest_leaves_02"), scale: 3.0, roughness: 1.0, normal_strength: 1.0, hide_tiling: true },
    Layer { scan: Some("brown_mud"), scale: 3.0, roughness: 0.8, normal_strength: 1.0, hide_tiling: true },
    Layer { scan: Some("old_stone_wall"), scale: 3.0, roughness: 1.0, normal_strength: 1.2, hide_tiling: false },
    Layer { scan: Some("plaster_stone_wall_01"), scale: 3.0, roughness: 1.0, normal_strength: 1.0, hide_tiling: false },
    Layer { scan: Some("roof_slates_02"), scale: 2.5, roughness: 0.9, normal_strength: 1.2, hide_tiling: false },
    Layer { scan: Some("roof_tiles"), scale: 2.5, roughness: 1.0, normal_strength: 1.2, hide_tiling: false },
    Layer { scan: Some("oak_wood_planks"), scale: 2.0, roughness: 1.0, normal_strength: 1.0, hide_tiling: false },
    Layer { scan: None, scale: 1.0, roughness: 1.0, normal_strength: 0.0, hide_tiling: false },
    Layer { scan: None, scale: 0.5, roughness: 1.0, normal_strength: 0.6, hide_tiling: false },
];

pub const SIZE: u32 = 2048;

pub struct Materials {
    pub albedo: Vec<Image>,
    /// rgb: OpenGL-convention normal, a: roughness.
    pub normal_roughness: Vec<Image>,
    /// Per layer: metres per repeat, roughness scale, normal strength, tiling flag.
    pub params: [[f32; 4]; 16],
}

fn load(path: PathBuf, srgb: bool) -> anyhow::Result<Image> {
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let image = Image::from_bytes(&bytes, srgb)?;
    anyhow::ensure!(
        image.width == SIZE && image.height == SIZE,
        "{} is not {SIZE}×{SIZE}",
        path.display()
    );
    Ok(image)
}

/// The scan's colour, and its normal map with roughness packed into alpha.
fn scan(name: &str) -> anyhow::Result<(Image, Image)> {
    let file = |map: &str| asset(&format!("textures/{name}_{map}.jpg"));
    let albedo = load(file("diff"), true)?;
    let mut normal = load(file("nor_gl"), false)?;
    let rough = load(file("rough"), false)?;
    for (n, r) in normal.data.as_chunks_mut::<4>().0.iter_mut().zip(rough.data.as_chunks::<4>().0) {
        n[3] = r[0];
    }
    Ok((albedo, normal))
}

/// Dry cereal and stubble, recoloured from the grass scan so it has the same fine detail.
fn straw(grass: &Image) -> Image {
    let lum = |p: &[u8]| (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722) / 255.0;
    let average = grass.data.as_chunks::<4>().0.iter().map(|p| lum(p)).sum::<f32>() / (SIZE * SIZE) as f32;
    // Linear albedo: ripe straw reflects about a third of the light.
    let gold = [0.3, 0.19, 0.06];
    let pale = [0.42, 0.31, 0.14];
    let data = grass
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let t = (lum(p) / average).powf(1.3);
            let mix = (t - 0.6).clamp(0.0, 1.0);
            let c: [f32; 3] = std::array::from_fn(|i| (gold[i] + (pale[i] - gold[i]) * mix) * t.min(1.6));
            // Stored sRGB, like the scan it came from.
            let encode = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            [encode(c[0]), encode(c[1]), encode(c[2]), 255]
        })
        .collect();
    Image::from_rgba(SIZE, SIZE, data, true)
}

pub fn load_all() -> anyhow::Result<Materials> {
    // Decoding a dozen 2K JPEG sets is the slow part of startup; do them side by side.
    let scans: Vec<anyhow::Result<Option<(Image, Image)>>> = std::thread::scope(|s| {
        let jobs: Vec<_> = LAYERS
            .iter()
            .map(|layer| s.spawn(move || layer.scan.map(scan).transpose()))
            .collect();
        jobs.into_iter().map(|j| j.join().expect("texture loader panicked")).collect()
    });
    let mut albedo = Vec::new();
    let mut normal_roughness = Vec::new();
    for (i, loaded) in scans.into_iter().enumerate() {
        match (loaded?, i as u32) {
            (Some((a, n)), _) => {
                albedo.push(a);
                normal_roughness.push(n);
            }
            (None, STRAW) => {
                albedo.push(straw(&albedo[GRASS as usize]));
                let mut n = normal_roughness[GRASS as usize].clone();
                for p in n.data.as_chunks_mut::<4>().0 {
                    p[3] = 235;
                }
                normal_roughness.push(n);
            }
            (None, CANVAS) => {
                let (a, n) = canvas();
                albedo.push(a);
                normal_roughness.push(n);
            }
            (None, GLASS) => {
                let pixels = (SIZE * SIZE) as usize;
                albedo.push(Image::from_rgba(SIZE, SIZE, [14, 17, 19, 255].repeat(pixels), true));
                normal_roughness.push(Image::from_rgba(SIZE, SIZE, [128, 128, 255, 12].repeat(pixels), false));
            }
            (None, other) => anyhow::bail!("layer {other} has no source"),
        }
    }
    let mut params = [[1.0, 1.0, 1.0, 0.0]; 16];
    for (p, layer) in params.iter_mut().zip(&LAYERS) {
        *p = [layer.scale, layer.roughness, layer.normal_strength, layer.hide_tiling as u8 as f32];
    }
    Ok(Materials {
        albedo,
        normal_roughness,
        params,
    })
}

/// Weathered tent canvas: a plain weave, a little grubby, with slubs in the thread.
fn canvas() -> (Image, Image) {
    let n = SIZE as usize;
    let mut albedo = Vec::with_capacity(n * n * 4);
    let mut normal = Vec::with_capacity(n * n * 4);
    let noise = |x: f32, y: f32, s: f32| voxl::voxel::fbm(91, voxl::glam::Vec3::new(x * s, y * s, 0.0), 4);
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32, y as f32);
            // 48 threads each way over the texture.
            let warp = (fx / n as f32 * 48.0 * std::f32::consts::TAU).sin();
            let weft = (fy / n as f32 * 48.0 * std::f32::consts::TAU).sin();
            let over = if (x * 48 / n + y * 48 / n).is_multiple_of(2) { warp } else { weft };
            let slub = noise(fx, fy, 0.03);
            let grime = noise(fx, fy, 0.004);
            let shade = 0.9 + 0.08 * over + 0.08 * (slub - 0.5) - 0.18 * (grime - 0.4).max(0.0);
            let base = [0.5, 0.47, 0.39];
            let encode = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            albedo.extend([encode(base[0] * shade), encode(base[1] * shade), encode(base[2] * shade), 255]);
            let nx = (warp * 0.25 * 127.0 + 128.0) as u8;
            let ny = (weft * 0.25 * 127.0 + 128.0) as u8;
            normal.extend([nx, ny, 240, 235]);
        }
    }
    (Image::from_rgba(SIZE, SIZE, albedo, true), Image::from_rgba(SIZE, SIZE, normal, false))
}
