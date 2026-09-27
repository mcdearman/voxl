//! Photographic materials for the square: Poly Haven scans (CC0), turned into the engine's
//! textures.

use std::path::{Path, PathBuf};

use anyhow::Context;
use voxl::{prelude::*, render::Image};

pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/res/paris");

pub fn asset(path: &str) -> PathBuf {
    Path::new(ASSETS).join(path)
}

/// One scanned surface: its colour, normal, and packed occlusion/roughness, all RGBA8.
struct Scan {
    albedo: Image,
    normal: Image,
    /// R: ambient occlusion, G: roughness, B: metalness, as glTF packs them.
    orm: Image,
    height: Option<Image>,
}

/// Scans come at 4K; 2K is sharper than the screen can show at any sensible distance here, and a
/// quarter of the memory.
const MAX_SIZE: u32 = 2048;

fn load(path: PathBuf, srgb: bool) -> anyhow::Result<Image> {
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut image = Image::from_bytes(&bytes, srgb)?;
    while image.width > MAX_SIZE {
        image = halve(&image);
    }
    Ok(image)
}

/// Half the size, each pixel the average of four.
fn halve(image: &Image) -> Image {
    let (w, h) = (image.width as usize, image.height as usize);
    let (nw, nh) = (w / 2, h / 2);
    let mut data = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let at = |dx: usize, dy: usize| image.data[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as u32;
                data[(y * nw + x) * 4 + c] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
            }
        }
    }
    Image::from_rgba(nw as u32, nh as u32, data, image.srgb)
}

fn scan(name: &str, with_height: bool) -> anyhow::Result<Scan> {
    let file = |kind: &str, ext: &str| asset(&format!("textures/{name}_{kind}.{ext}"));
    let albedo = load(file("diff", "jpg"), true)?;
    let normal = load(file("nor_gl", "jpg"), false)?;
    let rough = load(file("rough", "jpg"), false)?;
    let ao = load(file("ao", "jpg"), false).ok();
    let mut orm = rough.clone();
    for (i, p) in orm.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let r = rough.data[i * 4];
        p[0] = ao.as_ref().map_or(255, |ao| ao.data[i * 4]);
        p[1] = r;
        p[2] = 0;
        p[3] = 255;
    }
    let height = if with_height {
        Some(load(file("disp", "png"), false)?)
    } else {
        None
    };
    Ok(Scan {
        albedo,
        normal,
        orm,
        height,
    })
}



/// Rounds each sett into a dome. The scan's stones are nearly flat-topped; street setts are
/// worn into rounded crowns between deep joints. The joints are found in the height map, each
/// texel's distance to the nearest joint is measured, the height becomes a rounded profile over
/// that distance (blurred smooth), keeping a little of the scan's own relief for texture; and
/// the normal map gets the dome's slope added, so light rolls over the stones too.
fn dome(scan: &mut Scan, tile: f32, height_scale: f32) {
    let Some(height) = &mut scan.height else {
        return;
    };
    let (w, h) = (height.width as usize, height.height as usize);
    // Joints are the lowest third of the height field.
    let mut sorted: Vec<u8> = (0..w * h).map(|i| height.data[i * 4]).collect();
    sorted.sort_unstable();
    let joint = sorted[sorted.len() / 3] as f32 / 255.0;
    let original: Vec<f32> = (0..w * h).map(|i| height.data[i * 4] as f32 / 255.0).collect();

    // Chamfer distance to the nearest joint, in texels, wrapping round (the texture tiles).
    let far = 1e6f32;
    let mut d: Vec<f32> = original.iter().map(|&v| if v <= joint { 0.0 } else { far }).collect();
    let idx = |x: isize, y: isize| (y.rem_euclid(h as isize) as usize) * w + x.rem_euclid(w as isize) as usize;
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for _ in 0..2 {
        for y in 0..h as isize {
            for x in 0..w as isize {
                let v = d[idx(x, y)].min(d[idx(x - 1, y)] + a).min(d[idx(x, y - 1)] + a).min(d[idx(x - 1, y - 1)] + b).min(d[idx(x + 1, y - 1)] + b);
                d[idx(x, y)] = v;
            }
        }
        for y in (0..h as isize).rev() {
            for x in (0..w as isize).rev() {
                let v = d[idx(x, y)].min(d[idx(x + 1, y)] + a).min(d[idx(x, y + 1)] + a).min(d[idx(x + 1, y + 1)] + b).min(d[idx(x - 1, y + 1)] + b);
                d[idx(x, y)] = v;
            }
        }
    }
    // A gentle rounded crown, steepest at the joint and easing flat on top.
    let radius = w as f32 / 26.0;
    let mut smooth: Vec<f32> = d
        .iter()
        .map(|&dist| {
            let t = (dist / radius).min(1.0);
            t * (2.0 - t)
        })
        .collect();
    // The distance field has creases along each stone's middle, where the distances from
    // opposite joints meet; blur them away so the crowns are smooth.
    let blur = (radius * 0.35) as isize;
    for _ in 0..3 {
        let mut across = vec![0.0f32; w * h];
        for y in 0..h as isize {
            let mut sum: f32 = (-blur..=blur).map(|k| smooth[idx(k, y)]).sum();
            for x in 0..w as isize {
                across[idx(x, y)] = sum / (2 * blur + 1) as f32;
                sum += smooth[idx(x + blur + 1, y)] - smooth[idx(x - blur, y)];
            }
        }
        for x in 0..w as isize {
            let mut sum: f32 = (-blur..=blur).map(|k| across[idx(x, k)]).sum();
            for y in 0..h as isize {
                smooth[idx(x, y)] = sum / (2 * blur + 1) as f32;
                sum += across[idx(x, y + blur + 1)] - across[idx(x, y - blur)];
            }
        }
    }
    let domed: Vec<f32> = smooth.iter().zip(&original).map(|(&c, &o)| (c * 0.8 + o * 0.2).clamp(0.0, 1.0)).collect();
    for (i, v) in domed.iter().enumerate() {
        let byte = (v * 255.0 + 0.5) as u8;
        height.data[i * 4..i * 4 + 3].copy_from_slice(&[byte, byte, byte]);
    }

    // Add the dome's slope to the normal map (same size as the height map after loading).
    let normal = &mut scan.normal;
    if normal.width as usize != w || normal.height as usize != h {
        return;
    }
    let slope = height_scale / (tile / w as f32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let dx = (domed[idx(x as isize + 1, y as isize)] - domed[idx(x as isize - 1, y as isize)]) * 0.5 * slope;
            let dy = (domed[idx(x as isize, y as isize + 1)] - domed[idx(x as isize, y as isize - 1)]) * 0.5 * slope;
            let px = &mut normal.data[i * 4..i * 4 + 3];
            // Stored as OpenGL maps are: red along +u, green up the image (against rows).
            let nx = px[0] as f32 / 127.5 - 1.0 - dx;
            let ny = px[1] as f32 / 127.5 - 1.0 + dy;
            let nz = (px[2] as f32 / 127.5 - 1.0).max(0.2);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            for (c, v) in px.iter_mut().zip([nx, ny, nz]) {
                *c = ((v / len * 0.5 + 0.5) * 255.0 + 0.5) as u8;
            }
        }
    }
}

/// Wet after rain, as the old streets are more often than not. Water fills the stone's pores:
/// it goes darker and deeper in colour, and its surface turns nearly smooth, so each crown
/// carries a sharp highlight and mirrors the houses and the sky. The joints hold standing water.
fn wet(scan: &mut Scan) {
    let Some(height) = &scan.height else {
        return;
    };
    for i in 0..(scan.albedo.width * scan.albedo.height) as usize {
        let h = height.data[i * 4] as f32 / 255.0;
        // 1 in the joints, falling to a damp gloss on the crowns.
        let water = 1.0 - ((h - 0.15) / 0.35).clamp(0.0, 1.0);
        for c in 0..3 {
            let v = scan.albedo.data[i * 4 + c] as f32 / 255.0;
            // Darker and more saturated: squaring deepens the colour as a film of water does.
            let wet = (v * v * 0.9).max(v * 0.45) * (1.0 - 0.35 * water);
            scan.albedo.data[i * 4 + c] = (wet.clamp(0.0, 1.0) * 255.0) as u8;
        }
        let rough = scan.orm.data[i * 4 + 1] as f32 / 255.0;
        let glossy = (rough * 0.22 + 0.06) * (1.0 - water) + 0.02 * water;
        scan.orm.data[i * 4 + 1] = (glossy.clamp(0.0, 1.0) * 255.0) as u8;
    }
}

/// Every surface in the square.
#[derive(Clone, Copy)]
pub struct Palette {
    pub cobbles: Material,
    pub ashlar: Material,
    pub plaster: Material,
    pub wood: Material,
    pub iron: Material,
    pub slate: Material,
    pub brick: Material,
    pub canvas: Material,
    pub glass: Material,
    pub interior: Material,
    pub water: Material,
    pub stone_trim: Material,
    pub decals: crate::decals::Decals,
}

/// Metres covered by one repeat of each texture, from the scans' own records.
/// Setts about a fifth of a metre across, as the old streets were paved.
pub const COBBLE_TILE: f32 = 3.4;
pub const ASHLAR_TILE: f32 = 3.0;
pub const PLASTER_TILE: f32 = 2.0;
pub const WOOD_TILE: f32 = 1.0;
pub const IRON_TILE: f32 = 1.0;
pub const SLATE_TILE: f32 = 3.0;
pub const BRICK_TILE: f32 = 1.5;

pub fn load_all(images: &mut Assets<Image>) -> anyhow::Result<Palette> {
    let names = [
        ("cobblestone_floor_08", true),
        ("sandstone_blocks_08", true),
        ("beige_wall_001", true),
        ("wood_shutter", false),
        ("rusty_painted_metal", false),
        ("roof_slates_02", true),
        ("red_brick_03", false),
        ("rough_linen", false),
    ];
    // Decoding 4K scans is the slow part; do them side by side.
    let mut scans: Vec<Scan> = std::thread::scope(|s| {
        let jobs: Vec<_> = names.iter().map(|(n, h)| s.spawn(move || scan(n, *h))).collect();
        jobs.into_iter()
            .map(|j| j.join().expect("texture loader panicked"))
            .collect::<anyhow::Result<_>>()
    })?;
    // A dry, bright afternoon: the cobbles are left as they are, only warmed to the sandy
    // stone of the painting.

    let mut textured = |scan: Scan, height_scale: f32| {
        let has_height = scan.height.is_some();
        Material {
            base_color_texture: Some(images.add(scan.albedo)),
            normal_texture: Some(images.add(scan.normal)),
            metallic_roughness_texture: Some(images.add(scan.orm)),
            height_texture: scan.height.map(|h| images.add(h)),
            height_scale: if has_height { height_scale } else { 0.0 },
            roughness: 1.0,
            ..Default::default()
        }
    };
    dome(&mut scans[0], COBBLE_TILE, 0.05);
    wet(&mut scans[0]);
    let mut scans = scans.drain(..);
    // Years of soot, rain and street mud on everything, some surfaces more than others.
    let weathered = |m: Material, amount: f32| Material { weathering: amount, ..m };
    // Setts stand proud of deep joints; after the morning's rain, water still lies in the hollows.
    let cobbles = Material { puddles: 0.8, ..weathered(textured(scans.next().unwrap(), 0.05), 0.9) };
    let ashlar = weathered(textured(scans.next().unwrap(), 0.04), 1.4);
    let plaster = weathered(textured(scans.next().unwrap(), 0.012), 1.6);
    let wood = weathered(textured(scans.next().unwrap(), 0.0), 0.5);
    let iron = weathered(textured(scans.next().unwrap(), 0.0), 0.3);
    let slate = weathered(textured(scans.next().unwrap(), 0.03), 0.6);
    let brick = weathered(textured(scans.next().unwrap(), 0.0), 1.2);
    // Canvas for awnings: linen, faded and sooty.
    let canvas = Material { roughness: 1.0, double_sided: true, ..weathered(textured(scans.next().unwrap(), 0.0), 1.3) };
    let cobbles = Material { color: Color::rgb(1.08, 1.0, 0.9), ..cobbles };
    let decals = crate::decals::make(images);
    Ok(Palette {
        decals,
        cobbles,
        stone_trim: Material {
            color: Color::rgb(1.08, 1.04, 0.98),
            height_scale: 0.0,
            height_texture: None,
            ..ashlar
        },
        ashlar,
        plaster: Material {
            color: Color::rgb(0.95, 0.86, 0.72),
            ..plaster
        },
        wood,
        iron: Material {
            color: Color::rgb(0.35, 0.35, 0.36),
            metallic: 1.0,
            roughness: 0.7,
            ..iron
        },
        brick,
        canvas,
        // Blue-grey slate, darkened by weather.
        slate: Material { color: Color::rgb(0.36, 0.38, 0.43), ..slate },
        // Old crown glass: nearly black straight on, a mirror at a glance.
        glass: Material {
            color: Color::rgb(0.012, 0.014, 0.016),
            roughness: 0.04,
            ..Default::default()
        },
        interior: Material {
            color: Color::rgb(0.03, 0.022, 0.016),
            roughness: 0.9,
            ..Default::default()
        },
        water: Material {
            color: Color::rgb(0.012, 0.011, 0.01),
            roughness: 0.02,
            waves: 0.4,
            ..Default::default()
        },
    })
}
