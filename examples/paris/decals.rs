//! Decals for the square, painted procedurally: stains and grime on the paving, soot streaks
//! running down from window sills, cracks in plaster, bills pasted on the walls, and the
//! dung and straw of a city that runs on horses. Each is a colour texture whose alpha says how
//! much of it covers the surface beneath.

use voxl::{glam::Vec3, prelude::*, render::Image, voxel::value_noise};

const SIZE: usize = 512;

/// Every kind of decal, as a material ready to lay on a surface.
#[derive(Clone, Copy)]
pub struct Decals {
    pub stain: Material,
    pub grime: Material,
    pub streak: Material,
    pub crack: Material,
    pub posters: [Material; 3],
    pub dung: Material,
    pub straw: Material,
}

/// Fractal noise, 0 to 1, over a texture's unit square, tiling nowhere (decals don't tile).
fn fbm(seed: u32, x: f32, y: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise(seed + o, Vec3::new(x * freq, y * freq, o as f32 * 7.1));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Paints a texture from `f(u, v) -> (linear rgb, alpha)`, u and v from 0 to 1.
fn paint(f: impl Fn(f32, f32) -> ([f32; 3], f32) + Sync) -> Image {
    let rows: Vec<Vec<u8>> = std::thread::scope(|s| {
        let f = &f;
        let jobs: Vec<_> = (0..8)
            .map(|part| {
                s.spawn(move || {
                    let mut out = Vec::with_capacity(SIZE * SIZE / 8 * 4);
                    for y in part * SIZE / 8..(part + 1) * SIZE / 8 {
                        for x in 0..SIZE {
                            let (c, a) = f((x as f32 + 0.5) / SIZE as f32, (y as f32 + 0.5) / SIZE as f32);
                            for v in c {
                                out.push((v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8);
                            }
                            out.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                        }
                    }
                    out
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    Image::from_rgba(SIZE as u32, SIZE as u32, rows.concat(), true)
}

/// Fades to nothing toward the edges of the square, so no decal shows a straight border.
fn vignette(u: f32, v: f32) -> f32 {
    let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
    1.0 - smoothstep(0.55, 1.0, d)
}

pub fn make(images: &mut Assets<Image>) -> Decals {
    let decal = |images: &mut Assets<Image>, image: Image, roughness: f32| Material {
        base_color_texture: Some(images.add(image)),
        roughness,
        decal: true,
        double_sided: true,
        ..Default::default()
    };

    // A spill or a patch of trodden dirt: a ragged blotch, darkest in the middle.
    let stain = paint(|u, v| {
        let n = fbm(3, u * 4.0, v * 4.0, 5);
        let a = smoothstep(0.34, 0.72, n * vignette(u, v) * 1.4) * 0.6;
        ([0.06, 0.045, 0.03], a)
    });
    // Grime at the foot of a wall: dense along the bottom edge, thinning upward, blotchy.
    let grime = paint(|u, v| {
        let n = fbm(7, u * 6.0, v * 3.0, 5);
        let height = 1.0 - v;
        let edge = smoothstep(0.0, 0.12, u) * smoothstep(0.0, 0.12, 1.0 - u);
        let a = (smoothstep(1.0, 0.1, height * 1.6 - n * 0.6) * 0.75 * edge).clamp(0.0, 1.0);
        ([0.06, 0.05, 0.04], a)
    });
    // Soot and rain run down from a sill: streaks, heavy at the top and fading downward.
    let streak = paint(|u, v| {
        let lines = fbm(11, u * 22.0, v * 1.5, 4);
        let side = smoothstep(0.0, 0.25, u) * smoothstep(0.0, 0.25, 1.0 - u);
        let fall = (1.0 - v).powf(1.6);
        let a = smoothstep(0.35, 0.75, lines) * side * (1.0 - v).max(0.0) * 0.8 + fall * side * 0.15;
        ([0.04, 0.035, 0.03], a.min(0.85))
    });
    // Cracks: dark branching hairlines where the plaster has given way.
    let crack = paint(|u, v| {
        let mut darkest: f32 = 0.0;
        for branch in 0..3 {
            let b = branch as f32;
            // Each crack wanders down the square, pushed about by noise.
            let x = 0.5 + (fbm(19 + branch, v * 3.0, b, 4) - 0.5) * 0.9 + (b - 1.0) * 0.12 * v;
            let width = 0.004 + 0.004 * (1.0 - v);
            let d = (u - x).abs();
            darkest = darkest.max((1.0 - smoothstep(width * 0.3, width, d)) * smoothstep(0.0, 0.1, v) * (1.0 - smoothstep(0.7 + b * 0.1, 1.0, v)));
        }
        ([0.03, 0.028, 0.025], darkest * 0.9)
    });
    // Bills pasted on walls: paper, a heading, rows of print, torn and weathered edges.
    let poster = |seed: u32, paper: [f32; 3], ink: [f32; 3]| {
        paint(move |u, v| {
            let torn = fbm(seed, u * 9.0, v * 9.0, 4);
            let margin = 0.06 + torn * 0.07;
            let inside = u > margin && u < 1.0 - margin && v > margin && v < 1.0 - margin * 0.6;
            if !inside {
                return ([0.0; 3], 0.0);
            }
            let aged = 0.8 + 0.2 * fbm(seed + 5, u * 5.0, v * 5.0, 3);
            let mut c = paper.map(|p| p * aged);
            // A big heading, then lines of smaller type, then a line at the foot.
            let heading = v > 0.14 && v < 0.27 && u > 0.16 && u < 0.84;
            let row = ((v - 0.34) / 0.045).floor();
            let in_row = v > 0.34 && v < 0.8 && ((v - 0.34) / 0.045).fract() < 0.45;
            let row_len = 0.62 + 0.2 * value_noise(seed + 9, Vec3::new(row, 0.0, 0.0));
            let print = in_row && u > 0.14 && u < 0.14 + row_len * 0.72;
            let letters = value_noise(seed + 3, Vec3::new(u * 80.0, row, 0.0)) > 0.3;
            let heading_letters = value_noise(seed + 4, Vec3::new(u * 26.0, 0.0, 0.0)) > 0.25;
            if (heading && heading_letters) || (print && letters) {
                c = ink;
            }
            let wear = smoothstep(0.55, 0.8, fbm(seed + 7, u * 7.0, v * 7.0, 4));
            ([c[0] * (1.0 - wear * 0.3), c[1] * (1.0 - wear * 0.3), c[2] * (1.0 - wear * 0.3)], 1.0 - wear * 0.6)
        })
    };
    // Horse dung: a few dark lumps in a trodden smear.
    let dung = paint(|u, v| {
        let mut lump: f32 = 0.0;
        for k in 0..6 {
            let cx = 0.35 + 0.3 * value_noise(41, Vec3::new(k as f32, 0.0, 0.0));
            let cy = 0.35 + 0.3 * value_noise(43, Vec3::new(k as f32, 0.0, 0.0));
            let r = 0.06 + 0.04 * value_noise(47, Vec3::new(k as f32, 0.0, 0.0));
            let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
            lump = lump.max(1.0 - smoothstep(r * 0.7, r, d));
        }
        let smear = smoothstep(0.45, 0.65, fbm(51, u * 5.0, v * 5.0, 4) * vignette(u, v) * 1.3) * 0.5;
        let c = if lump > 0.5 { [0.07, 0.055, 0.03] } else { [0.09, 0.075, 0.045] };
        (c, lump.max(smear))
    });
    // Loose straw: thin golden strokes lying every which way.
    let straw = paint(|u, v| {
        let mut a: f32 = 0.0;
        let mut shade = 0.0;
        for k in 0..60 {
            let r = |j: u32| value_noise(61 + j, Vec3::new(k as f32 * 1.7, j as f32, 0.0));
            let (cx, cy) = (0.15 + 0.7 * r(0), 0.15 + 0.7 * r(1));
            let angle = r(2) * std::f32::consts::TAU;
            let len = 0.05 + 0.12 * r(3);
            let (dx, dy) = (u - cx, v - cy);
            let along = dx * angle.cos() + dy * angle.sin();
            let across = -dx * angle.sin() + dy * angle.cos();
            if along.abs() < len && across.abs() < 0.004 {
                a = 1.0;
                shade = r(4);
            }
        }
        ([0.26 + 0.12 * shade, 0.2 + 0.08 * shade, 0.08], a * vignette(u, v).sqrt() * 0.85)
    });

    Decals {
        stain: decal(images, stain, 0.7),
        grime: decal(images, grime, 0.9),
        streak: decal(images, streak, 0.9),
        crack: decal(images, crack, 0.9),
        posters: [
            decal(images, poster(71, [0.8, 0.75, 0.6], [0.05, 0.04, 0.04]), 0.9),
            decal(images, poster(83, [0.75, 0.62, 0.45], [0.35, 0.05, 0.03]), 0.9),
            decal(images, poster(97, [0.62, 0.66, 0.72], [0.04, 0.05, 0.12]), 0.9),
        ],
        dung: decal(images, dung, 0.5),
        straw: decal(images, straw, 0.95),
    }
}
