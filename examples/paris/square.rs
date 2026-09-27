//! The Place du Châtelet in about 1810, after the painting by Étienne Bouhot: the new Fontaine
//! du Palmier (1808) in the middle, its palm-trunk column bound with gilded bands and crowned
//! by a golden Victory; tall old houses down the left in shadow; a sunlit block of ochre stone
//! with red shopfronts on the right; and across the Seine, over the Pont au Change, the clock
//! tower of the Palais de Justice and the conical towers of the Conciergerie.
//!
//! +X is to the right looking from the square toward the river, which lies to -Z. One unit is
//! one metre; the square is level at y = 0.

use std::f32::consts::{FRAC_PI_4, PI, TAU};

use voxl::{
    glam::{Vec2, Vec3},
    prelude::*,
    render::Vertex,
};

use crate::{
    buildings::{bracket_lantern, hash, oriented, puddle, borne, Builder, GroundFloor, House},
    materials::{Palette, ASHLAR_TILE, COBBLE_TILE, IRON_TILE, PLASTER_TILE, SLATE_TILE, WOOD_TILE},
};

/// The foot of the fountain's column.
pub const FOUNTAIN: Vec3 = Vec3::new(6.0, 0.0, -12.0);
/// The fountain's outer step: nobody walks through it.
pub const FOUNTAIN_RADIUS: f32 = 6.6;
/// Where people may walk: x from the left houses to the right ones, z from the quay back.
pub const WALK_MIN: Vec2 = Vec2::new(-19.2, -57.0);
pub const WALK_MAX: Vec2 = Vec2::new(29.2, 44.0);
/// The river's surface, well below the quays.
const RIVER: f32 = -5.5;
/// The quay edges, north (the square's) and south (the island's).
const QUAY_N: f32 = -58.0;
const QUAY_S: f32 = -101.0;
/// The Pont au Change's roadway, between these x.
const BRIDGE: (f32, f32) = (-9.0, 5.0);

pub fn fountain_center() -> Vec2 {
    Vec2::new(FOUNTAIN.x, FOUNTAIN.z)
}

/// Trees along the quay, either side of the bridge.
const TREES: [(f32, f32); 8] = [(9.0, -54.5), (15.0, -54.5), (21.0, -54.5), (27.0, -54.5), (-24.0, -50.0), (-30.0, -50.0), (-36.0, -50.0), (-42.0, -50.0)];
/// Street lamps on cast-iron posts.
const LAMPS: [(f32, f32); 7] = [(-4.0, 6.0), (16.0, 6.0), (-10.0, -24.0), (25.0, -38.0), (-10.0, -56.0), (6.0, -56.0), (4.0, 26.0)];

/// What people walk round, as circles on the ground: the fountain, the stalls, the carts,
/// the coach and their horses, the toll house, the trees and the lamp posts.
pub fn obstacles() -> Vec<(Vec2, f32)> {
    let posts = TREES.iter().map(|(x, z)| (Vec2::new(*x, *z), 0.7)).chain(LAMPS.iter().map(|(x, z)| (Vec2::new(*x, *z), 0.45)));
    posts.chain([
        (fountain_center(), FOUNTAIN_RADIUS),
        (Vec2::new(26.8, -3.5), 1.7),
        (Vec2::new(26.8, 2.0), 1.7),
        (Vec2::new(26.8, 7.5), 1.7),
        (Vec2::new(22.0, -24.0), 2.6),
        (Vec2::new(20.8, -20.0), 1.9),
        (Vec2::new(-13.8, -47.5), 3.9),
    ])
    .collect()
}

/// A street lamp: a fluted cast-iron post on a stone base, the lantern on a bracket at its top.
fn lamp_post(b: &mut Builder, pal: &Palette, at: Vec3) {
    let iron = Material { color: Color::rgb(0.05, 0.055, 0.06), roughness: 0.5, metallic: 1.0, ..pal.iron };
    b.add("trim", pal.stone_trim, ASHLAR_TILE, &Mesh::frustum(0.32, 0.26, 0.4, 16), Mat4::from_translation(at + Vec3::Y * 0.2));
    b.add("lamp_iron", iron, IRON_TILE, &Mesh::frustum(0.12, 0.07, 3.4, 12), Mat4::from_translation(at + Vec3::Y * 2.1));
    for (y, r) in [(0.55, 0.16), (1.3, 0.1), (3.75, 0.1)] {
        b.add("lamp_iron", iron, IRON_TILE, &Mesh::cylinder(r, 0.06, 12), Mat4::from_translation(at + Vec3::Y * y));
    }
    crate::buildings::post_lantern(b, pal, at + Vec3::Y * 4.15);
}

/// A figure to place: which model, where its feet are, and the direction it faces.
pub struct Placement {
    pub model: &'static str,
    pub at: Vec3,
    pub facing: Vec2,
    pub scale: f32,
}

pub struct Square {
    pub parts: Vec<(Material, Mesh)>,
    pub people: Vec<Placement>,
    /// The four statues round the column and the Victory on top.
    pub statues: Vec<Placement>,
    pub horses: Vec<(&'static str, Vec3, Vec2)>,
    pub vehicles: Vec<Vehicle>,
    /// Animals about the square: which, where they keep to, and how far they stray.
    pub animals: Vec<(&'static str, Vec2, f32)>,
    /// Trees along the quay: where, turned how far, and how big.
    pub trees: Vec<(Vec3, f32, f32)>,
    /// Chimney pots with a fire lit below.
    pub chimneys: Vec<Vec3>,
}

/// A ring: the wall of a round basin, from `inner` to `outer` radius, `y0` to `y1`.
fn ring(inner: f32, outer: f32, y0: f32, y1: f32, segments: u32) -> Mesh {
    let mut mesh = Mesh::default();
    let at = |r: f32, a: f32, y: f32| Vec3::new(a.cos() * r, y, a.sin() * r);
    let mut quad = |p: [Vec3; 4], n: [Vec3; 4]| {
        let base = mesh.vertices.len() as u32;
        for (p, n) in p.iter().zip(n) {
            mesh.vertices.push(Vertex::new(*p, n, Vec2::ZERO));
        }
        mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    for i in 0..segments {
        let (a0, a1) = (TAU * i as f32 / segments as f32, TAU * (i + 1) as f32 / segments as f32);
        let (n0, n1) = (Vec3::new(a0.cos(), 0.0, a0.sin()), Vec3::new(a1.cos(), 0.0, a1.sin()));
        // Outer wall, facing out; inner wall, facing in; the top.
        quad([at(outer, a0, y0), at(outer, a0, y1), at(outer, a1, y1), at(outer, a1, y0)], [n0, n0, n1, n1]);
        quad([at(inner, a1, y0), at(inner, a1, y1), at(inner, a0, y1), at(inner, a0, y0)], [-n1, -n1, -n0, -n0]);
        quad([at(inner, a0, y1), at(inner, a1, y1), at(outer, a1, y1), at(outer, a0, y1)], [Vec3::Y; 4]);
    }
    mesh
}

/// A flat rectangle of paving at height `y`, facing up, in cells of `cell` metres.
fn paving(b: &mut Builder, name: &str, material: Material, tile: f32, min: Vec2, max: Vec2, y: f32) {
    let cell = 6.0;
    let (nx, nz) = (((max.x - min.x) / cell).ceil() as u32, ((max.y - min.y) / cell).ceil() as u32);
    let mesh = b.mesh(name, material, tile);
    let base = mesh.vertices.len() as u32;
    for j in 0..=nz {
        for i in 0..=nx {
            let x = min.x + (max.x - min.x) * i as f32 / nx as f32;
            let z = min.y + (max.y - min.y) * j as f32 / nz as f32;
            mesh.vertices.push(Vertex::new(Vec3::new(x, y, z), Vec3::Y, Vec2::new(x, z) / tile));
        }
    }
    let row = nx + 1;
    for j in 0..nz {
        for i in 0..nx {
            let a = base + j * row + i;
            let (bb, c, d) = (a + 1, a + row, a + row + 1);
            mesh.indices.extend([a, c, bb, bb, c, d]);
        }
    }
}

/// A spoked cart wheel standing in the plane across `axle`.
fn wheel(b: &mut Builder, wood: Material, iron: Material, center: Vec3, axle: Vec3, radius: f32) {
    let axle = axle.normalize();
    let u = axle.cross(Vec3::Y).normalize();
    let segments = 20;
    let rim = |a: f32| center + (u * a.cos() + Vec3::Y * a.sin()) * radius;
    for k in 0..segments {
        let (a0, a1) = (TAU * k as f32 / segments as f32, TAU * (k + 1) as f32 / segments as f32);
        b.rod("cart_iron", iron, IRON_TILE, rim(a0), rim(a1), 0.07);
    }
    for k in 0..12 {
        let a = TAU * k as f32 / 12.0;
        b.rod("cart_wood", wood, WOOD_TILE, center, center + (u * a.cos() + Vec3::Y * a.sin()) * (radius - 0.03), 0.04);
    }
    b.add("cart_wood", wood, WOOD_TILE, &Mesh::cylinder(0.1, 0.3, 16), Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Y, axle), center));
}

/// A two-wheeled tumbril or a four-wheeled wagon behind a horse: `at` is the middle of the
/// bed, `heading` the way the horse faces.
/// With `spin`, the wheels are left off and returned (centre and radius) to be made as parts
/// that turn.
fn cart(b: &mut Builder, at: Vec3, heading: Vec2, four_wheels: bool, load: Option<Color>, spin: bool) -> Vec<(Vec3, f32)> {
    let mut wheels = Vec::new();
    let forward = Vec3::new(heading.x, 0.0, heading.y).normalize();
    let side = forward.cross(Vec3::Y).normalize();
    let wood = Material { color: Color::rgb(0.24, 0.16, 0.09), roughness: 0.8, ..Default::default() };
    let iron = Material { color: Color::rgb(0.08, 0.08, 0.08), roughness: 0.55, metallic: 1.0, ..Default::default() };
    let length = if four_wheels { 3.4 } else { 2.2 };
    let bed = at + Vec3::Y * 1.0;
    let rotation = oriented(side, forward);
    b.rotated("cart_wood", wood, WOOD_TILE, bed, Vec3::new(1.4, 0.08, length), rotation);
    for s in [-1.0f32, 1.0] {
        b.rotated("cart_wood", wood, WOOD_TILE, bed + side * s * 0.68 + Vec3::Y * 0.3, Vec3::new(0.05, 0.55, length), rotation);
    }
    b.rotated("cart_wood", wood, WOOD_TILE, bed - forward * length * 0.5 + Vec3::Y * 0.3, Vec3::new(1.4, 0.55, 0.05), rotation);
    // Shafts running forward to the horse.
    for s in [-1.0f32, 1.0] {
        let from = bed + side * s * 0.45 + forward * length * 0.3;
        b.rod("cart_wood", wood, WOOD_TILE, from, from + forward * 2.6 + Vec3::Y * 0.1, 0.07);
    }
    let axles: &[f32] = if four_wheels { &[-1.1, 1.0] } else { &[0.0] };
    for &a in axles {
        let r = if four_wheels && a > 0.0 { 0.55 } else { 0.8 };
        for s in [-1.0f32, 1.0] {
            let center = at + forward * a + side * s * 0.82 + Vec3::Y * r;
            if spin {
                wheels.push((center, r));
            } else {
                wheel(b, wood, iron, center, side, r);
            }
        }
    }
    if let Some(color) = load {
        // A load of hay or sacks under a canvas cover, bellied over hoops.
        let canvas = Material { color, roughness: 0.95, ..Default::default() };
        let cover = Mesh::uv_sphere(1.0, 24, 12);
        b.add("canvas", canvas, 1.0, &cover, Mat4::from_scale_rotation_translation(Vec3::new(0.8, 0.8, length * 0.52), rotation, bed + Vec3::Y * 0.45));
    }
    wheels
}

/// A cart wheel on its own, centred on the origin with its axle along X, to turn as it rolls.
pub fn wheel_parts(radius: f32) -> Vec<(Material, Mesh)> {
    let mut b = Builder::new();
    let wood = Material { color: Color::rgb(0.24, 0.16, 0.09), roughness: 0.8, ..Default::default() };
    let iron = Material { color: Color::rgb(0.08, 0.08, 0.08), roughness: 0.55, metallic: 1.0, ..Default::default() };
    wheel(&mut b, wood, iron, Vec3::ZERO, Vec3::X, radius);
    b.finish().0
}

/// A cart that drives about the square behind a walking horse. Its parts are built facing +Z
/// about its own origin.
pub struct Vehicle {
    pub parts: Vec<(Material, Mesh)>,
    pub wheels: Vec<(Vec3, f32)>,
    /// Where the horse stands, and which.
    pub horse: (Vec3, &'static str),
    /// The waypoint of `ROUTE` it starts at.
    pub start: usize,
}

/// The way carts go round the square, clear of the fountain, the stalls and the lamps.
pub const ROUTE: [(f32, f32); 8] = [(-4.0, -44.0), (-8.0, -20.0), (-6.0, 10.0), (6.0, 18.0), (13.0, 12.0), (15.0, -20.0), (14.0, -40.0), (2.0, -46.0)];

fn vehicles() -> Vec<Vehicle> {
    let mut out = Vec::new();
    for (four_wheels, load, horse, start) in [
        (false, None, (Vec3::new(0.0, 0.0, 3.6), "horse_bay"), 0),
        (true, Some(Color::rgb(0.8, 0.77, 0.68)), (Vec3::new(0.0, 0.0, 4.6), "horse_chestnut"), 4),
    ] {
        let mut b = Builder::new();
        let wheels = cart(&mut b, Vec3::ZERO, Vec2::new(0.0, 1.0), four_wheels, load, true);
        out.push(Vehicle { parts: b.finish().0, wheels, horse, start });
    }
    out
}

/// A closed carriage, yellow bodied with a black top, on four wheels.
fn coach(b: &mut Builder, at: Vec3, heading: Vec2) {
    let forward = Vec3::new(heading.x, 0.0, heading.y).normalize();
    let side = forward.cross(Vec3::Y).normalize();
    let rotation = oriented(side, forward);
    let body = Material { color: Color::rgb(0.62, 0.42, 0.08), roughness: 0.25, ..Default::default() };
    let black = Material { color: Color::rgb(0.02, 0.02, 0.02), roughness: 0.3, ..Default::default() };
    let wood = Material { color: Color::rgb(0.3, 0.12, 0.05), roughness: 0.5, ..Default::default() };
    let iron = Material { color: Color::rgb(0.06, 0.06, 0.06), roughness: 0.5, metallic: 1.0, ..Default::default() };
    let center = at + Vec3::Y * 1.55;
    b.rotated("coach_body", body, 1.0, center, Vec3::new(1.5, 1.2, 2.1), rotation);
    b.rotated("coach_black", black, 1.0, center + Vec3::Y * 0.7, Vec3::new(1.55, 0.2, 2.2), rotation);
    for s in [-1.0f32, 1.0] {
        b.rotated("glass", Material { color: Color::rgb(0.015, 0.017, 0.02), roughness: 0.03, ..Default::default() }, 1.0,
            center + side * s * 0.76 + Vec3::Y * 0.2, Vec3::new(0.01, 0.5, 0.7), rotation);
    }
    // The driver's box in front.
    b.rotated("coach_black", black, 1.0, center + forward * 1.4 + Vec3::Y * 0.2, Vec3::new(1.2, 0.6, 0.7), rotation);
    for (a, r) in [(-0.9, 0.75), (1.0, 0.55)] {
        for s in [-1.0f32, 1.0] {
            wheel(b, wood, iron, at + forward * a + side * s * 0.85 + Vec3::Y * r, side, r);
        }
    }
    for s in [-1.0f32, 1.0] {
        let from = center - Vec3::Y * 0.6 + forward * 1.2 + side * s * 0.4;
        b.rod("cart_wood", wood, WOOD_TILE, from, from + forward * 2.8, 0.07);
    }
}


/// A market stall: a trestle table under a canvas canopy, with baskets of produce.
fn stall(b: &mut Builder, pal: &Palette, at: Vec3, facing: Vec2, canopy: Color, seed: f32) {
    let forward = Vec3::new(facing.x, 0.0, facing.y).normalize();
    let side = forward.cross(Vec3::Y).normalize();
    let rotation = oriented(side, forward);
    let wood = Material { color: Color::rgb(0.3, 0.2, 0.12), ..pal.wood };
    b.rotated("stall_wood", wood, WOOD_TILE, at + Vec3::Y * 0.85, Vec3::new(2.2, 0.06, 0.9), rotation);
    for (s, f) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let foot = at + side * s * 1.0 + forward * f * 0.38;
        b.rod("stall_wood", wood, WOOD_TILE, foot, foot + Vec3::Y * 0.85, 0.06);
        // Poles holding the canopy up at the back, taller than the front.
        let pole_top = foot + Vec3::Y * if f < 0.0 { 2.5 } else { 2.2 };
        b.rod("stall_wood", wood, WOOD_TILE, foot, pole_top, 0.05);
    }
    let canvas = Material { color: canopy, roughness: 0.9, double_sided: true, ..Default::default() };
    let tilt = Quat::from_axis_angle(side, -0.25);
    b.rotated(&format!("stall_canvas_{:.2}", canopy.r), canvas, 1.0, at + Vec3::Y * 2.38, Vec3::new(2.4, 0.02, 1.2), tilt * rotation);
    // Baskets heaped with apples, cabbages, onions and pears.
    let produce = [Color::rgb(0.5, 0.06, 0.04), Color::rgb(0.25, 0.4, 0.12), Color::rgb(0.55, 0.38, 0.14), Color::rgb(0.55, 0.5, 0.15)];
    let wicker = Material { color: Color::rgb(0.45, 0.32, 0.16), roughness: 0.9, ..Default::default() };
    for k in 0..4 {
        let p = at + Vec3::Y * 0.88 + side * (-0.75 + k as f32 * 0.5);
        b.add("wicker", wicker, 1.0, &Mesh::frustum(0.17, 0.2, 0.14, 16), Mat4::from_translation(p + Vec3::Y * 0.07));
        let fruit = Material { color: produce[(k + (seed * 4.0) as usize) % produce.len()], roughness: 0.45, ..Default::default() };
        for j in 0..9 {
            let a = j as f32 * 2.4 + seed;
            let r = if j == 0 { 0.0 } else { 0.1 };
            let q = p + Vec3::new(a.cos() * r, 0.17 + if j == 0 { 0.05 } else { 0.0 }, a.sin() * r);
            b.add(&format!("produce_{}", (k + (seed * 4.0) as usize) % produce.len()), fruit, 1.0, &Mesh::uv_sphere(0.055, 10, 6), Mat4::from_translation(q));
        }
    }
    // Crates and a barrel beside it.
    b.rotated("stall_wood", wood, WOOD_TILE, at + side * 1.5 + Vec3::Y * 0.2, Vec3::new(0.5, 0.4, 0.4), rotation);
    b.rotated("stall_wood", wood, WOOD_TILE, at + side * 1.5 + Vec3::Y * 0.55 + forward * 0.05, Vec3::new(0.45, 0.3, 0.35), rotation);
    b.add("stall_wood", wood, WOOD_TILE, &Mesh::frustum(0.26, 0.26, 0.8, 16), Mat4::from_translation(at - side * 1.55 + Vec3::Y * 0.4));
}

/// The Fontaine du Palmier: a round basin on steps, a square pedestal with lion masks and the
/// imperial eagle in a laurel wreath, the column bound like a palm trunk with gilded bands,
/// a crown of palm leaves, and the globe the Victory stands on. Returns the height of the top
/// of the globe.
fn fountain(b: &mut Builder, pal: &Palette) -> f32 {
    let f = FOUNTAIN;
    let stone = Material { color: Color::rgb(1.08, 1.02, 0.9), weathering: 0.6, ..pal.plaster };
    let dressed = Material { color: Color::rgb(1.1, 1.05, 0.95), weathering: 0.5, ..pal.ashlar };
    let gilt = Material { color: Color::rgb(1.0, 0.74, 0.34), roughness: 0.28, metallic: 1.0, ..Default::default() };
    let bronze = Material { color: Color::rgb(0.3, 0.22, 0.12), roughness: 0.4, metallic: 1.0, ..Default::default() };
    let at = |y: f32| Mat4::from_translation(f + Vec3::Y * y);
    let disc = |r: f32, h: f32| Mesh::cylinder(r, h, 64);

    // Steps, the basin wall with its coping, the water.
    b.add("fountain_dressed", dressed, ASHLAR_TILE, &disc(6.4, 0.16), at(0.08));
    b.add("fountain_dressed", dressed, ASHLAR_TILE, &disc(6.0, 0.16), at(0.24));
    b.add("fountain_dressed", dressed, ASHLAR_TILE, &ring(4.75, 5.2, 0.32, 0.95, 64), at(0.0));
    b.add("fountain_stone", stone, PLASTER_TILE, &ring(4.65, 5.3, 0.95, 1.07, 64), at(0.0));
    // The water in the basin is simulated (see water.rs).

    // The pedestal: plinth, die, cornice and attic.
    let block = |b: &mut Builder, name: &str, m: Material, tile: f32, w: f32, y0: f32, y1: f32| {
        b.block(name, m, tile, f + Vec3::Y * (y0 + y1) * 0.5, Vec3::new(w, y1 - y0, w));
    };
    block(b, "fountain_dressed", dressed, ASHLAR_TILE, 4.6, 0.32, 1.15);
    block(b, "fountain_stone", stone, PLASTER_TILE, 4.8, 1.15, 1.3);
    block(b, "fountain_dressed", dressed, ASHLAR_TILE, 3.8, 1.3, 3.9);
    block(b, "fountain_stone", stone, PLASTER_TILE, 4.2, 3.9, 4.1);
    block(b, "fountain_stone", stone, PLASTER_TILE, 4.45, 4.1, 4.3);
    block(b, "fountain_stone", stone, PLASTER_TILE, 3.3, 4.3, 5.0);

    for k in 0..4 {
        let dir = Quat::from_rotation_y(k as f32 * PI / 2.0) * Vec3::Z;
        let side = dir.cross(Vec3::Y);
        let face = f + dir * 1.9;
        // A raised panel framing the eagle in its wreath.
        for (du, dv, w, h) in [(0.0, 1.35, 2.6, 0.1), (0.0, 3.55, 2.6, 0.1), (-1.25, 2.45, 0.1, 2.3), (1.25, 2.45, 0.1, 2.3)] {
            b.block("fountain_stone", stone, PLASTER_TILE, face + side * du + Vec3::Y * dv + dir * 0.03, side.abs() * w + Vec3::Y * h + dir.abs() * 0.08);
        }
        let wreath = face + Vec3::Y * 2.55 + dir * 0.06;
        for i in 0..22 {
            let a = TAU * i as f32 / 22.0;
            let p = wreath + (side * a.cos() + Vec3::Y * a.sin()) * 0.62;
            let tangent = -side * a.sin() + Vec3::Y * a.cos();
            let leaf = Mat4::from_scale_rotation_translation(Vec3::new(0.05, 0.02, 0.14), oriented(tangent.cross(dir), tangent), p);
            b.add("fountain_gilt", gilt, 1.0, &Mesh::uv_sphere(1.0, 10, 6), leaf);
        }
        // The eagle, wings spread: a body and two swept wings, all gilt bronze.
        b.add("fountain_gilt", gilt, 1.0, &Mesh::uv_sphere(1.0, 12, 8), Mat4::from_scale_rotation_translation(Vec3::new(0.12, 0.26, 0.06), Quat::IDENTITY, wreath));
        b.add("fountain_gilt", gilt, 1.0, &Mesh::uv_sphere(0.07, 10, 6), Mat4::from_translation(wreath + Vec3::Y * 0.3));
        for s in [-1.0f32, 1.0] {
            let wing = (side * s + Vec3::Y * 0.55).normalize();
            b.rotated("fountain_gilt", gilt, 1.0, wreath + wing * 0.25 + dir * 0.01, Vec3::new(0.03, 0.14, 0.45), oriented(dir, wing));
        }
        // A lion's mask low on each face, spouting into the basin (the water itself is
        // simulated: see `spouts`).
        let mask = face + Vec3::Y * 1.75 + dir * 0.02;
        b.add("fountain_bronze", bronze, 1.0, &Mesh::uv_sphere(1.0, 14, 10), Mat4::from_scale_rotation_translation(Vec3::new(0.22, 0.24, 0.12), Quat::IDENTITY, mask));
        // The mouth, a dark hollow the jet comes out of.
        b.add("fountain_bronze", Material { color: Color::rgb(0.02, 0.015, 0.01), ..bronze }, 1.0, &Mesh::uv_sphere(1.0, 10, 8), Mat4::from_scale_rotation_translation(Vec3::new(0.07, 0.045, 0.03), Quat::IDENTITY, mask + dir * 0.11 - Vec3::Y * 0.05));
    }

    // The column's base mouldings, then the shaft in palm-trunk drums, each flaring a little
    // toward its gilded band.
    b.add("fountain_stone", stone, PLASTER_TILE, &Mesh::cylinder(1.0, 0.22, 48), at(5.11));
    b.add("fountain_stone", stone, PLASTER_TILE, &Mesh::frustum(0.9, 0.72, 0.3, 48), at(5.37));
    let (base, drum, drums) = (5.52, 1.18, 11);
    for k in 0..drums {
        let y = base + k as f32 * drum;
        b.add("fountain_stone", stone, PLASTER_TILE, &Mesh::frustum(0.6, 0.69, drum - 0.1, 40), at(y + (drum - 0.1) * 0.5));
        b.add("fountain_gilt", gilt, 1.0, &Mesh::cylinder(0.72, 0.1, 40), at(y + drum - 0.05));
    }
    // The capital: a flared bell, and palm leaves curling out from under the abacus.
    let top = base + drums as f32 * drum;
    b.add("fountain_stone", stone, PLASTER_TILE, &Mesh::frustum(0.68, 1.0, 0.8, 40), at(top + 0.4));
    for k in 0..14 {
        let a = TAU * k as f32 / 14.0;
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let dir = (out + Vec3::Y * 0.35).normalize();
        let root = f + Vec3::Y * (top + 0.75) + out * 0.75;
        b.rotated("fountain_gilt", gilt, 1.0, root + dir * 0.35 - Vec3::Y * 0.12, Vec3::new(0.22, 0.03, 0.8), oriented(out.cross(Vec3::Y), dir));
    }
    b.add("fountain_stone", stone, PLASTER_TILE, &Mesh::cylinder(0.85, 0.16, 40), at(top + 0.88));
    b.add("fountain_gilt", gilt, 1.0, &Mesh::frustum(0.35, 0.25, 0.3, 24), at(top + 1.1));
    let globe = top + 1.25 + 0.55;
    b.add("fountain_gilt", gilt, 1.0, &Mesh::uv_sphere(0.55, 32, 16), at(globe));

    // A ring of stone bornes round the fountain.
    for k in 0..16 {
        let a = TAU * (k as f32 + 0.5) / 16.0;
        borne(b, pal, f + Vec3::new(a.cos(), 0.0, a.sin()) * 7.6, 1.2);
    }
    // Water spilt round the basin where the carriers fill their buckets.
    for k in 0..6 {
        let a = TAU * k as f32 / 6.0 + 0.4;
        let p = Vec2::new(f.x, f.z) + Vec2::new(a.cos(), a.sin()) * (6.7 + hash(k as f32, 3.0));
        puddle(b, pal, COBBLE_TILE, p, Vec2::new(0.5, 0.35) * (0.8 + hash(k as f32, 5.0)), 0.012, k as f32 * 3.0);
    }
    globe + 0.55
}

/// The Tour de l'Horloge: a tall square medieval tower with a steep slate pyramid, a clock
/// dial on its face, and a small lantern turret.
fn clock_tower(b: &mut Builder, pal: &Palette, at: Vec3) {
    let (w, h) = (10.0, 32.0);
    let stone = Material { color: Color::rgb(0.95, 0.9, 0.8), ..pal.ashlar };
    b.block("tower", stone, ASHLAR_TILE, at + Vec3::Y * h * 0.5, Vec3::new(w, h, w));
    // Corner buttresses and a machicolated parapet course.
    for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        b.block("tower", stone, ASHLAR_TILE, at + Vec3::new(sx * w * 0.5, h * 0.45, sz * w * 0.5), Vec3::new(1.4, h * 0.9, 1.4));
    }
    b.block("tower", stone, ASHLAR_TILE, at + Vec3::Y * (h - 0.5), Vec3::new(w + 1.0, 1.0, w + 1.0));
    // Tall narrow windows, and the clock on the face toward the square.
    for k in 0..3 {
        let y = 9.0 + k as f32 * 7.0;
        for dx in [-2.5, 2.5] {
            b.block("glass", pal.glass, 1.0, at + Vec3::new(dx, y, w * 0.5 + 0.02), Vec3::new(1.0, 3.2, 0.04));
        }
    }
    let dial = Material { color: Color::rgb(0.05, 0.1, 0.3), roughness: 0.4, ..Default::default() };
    let gilt = Material { color: Color::rgb(1.0, 0.74, 0.34), roughness: 0.3, metallic: 1.0, ..Default::default() };
    let face = at + Vec3::new(0.0, 25.0, w * 0.5 + 0.05);
    b.add("clock", dial, 1.0, &Mesh::cylinder(2.0, 0.1, 40), Mat4::from_rotation_translation(Quat::from_rotation_x(PI / 2.0), face));
    b.add("gilt", gilt, 1.0, &ring(1.9, 2.15, -0.06, 0.06, 40), Mat4::from_rotation_translation(Quat::from_rotation_x(PI / 2.0), face));
    b.rod("gilt", gilt, 1.0, face + Vec3::Z * 0.08, face + Vec3::new(0.0, 1.5, 0.08), 0.1);
    b.rod("gilt", gilt, 1.0, face + Vec3::Z * 0.08, face + Vec3::new(1.0, 0.3, 0.08), 0.1);
    // The pyramid roof and its turret.
    b.add("slate", pal.slate, SLATE_TILE, &Mesh::cone(w * 0.72, 11.0, 4), Mat4::from_rotation_translation(Quat::from_rotation_y(FRAC_PI_4), at + Vec3::Y * (h + 5.5)));
    b.add("tower_lead", pal.iron, IRON_TILE, &Mesh::cylinder(0.8, 2.0, 12), Mat4::from_translation(at + Vec3::Y * (h + 7.0)));
    b.add("tower_lead", pal.iron, IRON_TILE, &Mesh::cone(1.0, 3.5, 12), Mat4::from_translation(at + Vec3::Y * (h + 9.7)));
}

/// A round medieval tower with a pointed slate cone.
fn round_tower(b: &mut Builder, pal: &Palette, at: Vec3, radius: f32, height: f32) {
    let stone = Material { color: Color::rgb(0.92, 0.88, 0.8), ..pal.ashlar };
    b.add("tower", stone, ASHLAR_TILE, &Mesh::cylinder(radius, height, 40), Mat4::from_translation(at + Vec3::Y * height * 0.5));
    b.add("tower", stone, ASHLAR_TILE, &Mesh::cylinder(radius + 0.4, 0.8, 40), Mat4::from_translation(at + Vec3::Y * (height - 0.4)));
    b.add("slate", pal.slate, SLATE_TILE, &Mesh::cone(radius + 0.6, radius * 2.6, 40), Mat4::from_translation(at + Vec3::Y * (height + radius * 1.3)));
    for k in 0..3 {
        let a = -PI / 2.0 + (k as f32 - 1.0) * 0.5;
        let p = at + Vec3::new(a.cos() * radius, 7.0 + k as f32 * 4.0, -a.sin() * radius);
        b.block("glass", pal.glass, 1.0, p, Vec3::new(0.8, 1.8, 0.8));
    }
}

/// A row of houses along a line, one after another, each a little different.
#[allow(clippy::too_many_arguments)]
fn row(b: &mut Builder, pal: &Palette, origin: Vec3, along: Vec3, out: Vec3, length: f32, seed: f32, tall: bool, mut special: impl FnMut(&mut Builder, &House, usize)) {
    let paints = [
        Color::rgb(0.16, 0.26, 0.2),
        Color::rgb(0.42, 0.08, 0.06),
        Color::rgb(0.07, 0.08, 0.09),
        Color::rgb(0.22, 0.28, 0.33),
        Color::rgb(0.5, 0.42, 0.28),
    ];
    let mut u = 0.0;
    let mut i = 0;
    while u < length - 3.0 {
        let r = |k: f32| hash(u * 1.7 + seed * 31.0, k);
        let width = (7.5 + r(1.0) * 5.5).min(length - u);
        let house = House {
            origin: origin + along * u,
            along,
            out,
            width,
            floors: if tall { 4 + (r(3.0) * 2.5) as usize } else { 3 + (r(3.0) * 1.8) as usize },
            stone: r(4.0) < 0.45,
            ground_floor: if tall && i == 1 {
                GroundFloor::Gate
            } else if r(2.0) < 0.75 {
                GroundFloor::Shop
            } else {
                GroundFloor::Windows
            },
            paint: paints[(r(5.0) * paints.len() as f32) as usize % paints.len()],
            seed: r(6.0),
            mansard: r(7.0) < 0.6,
            depth: 10.0,
            ornate: r(9.0) < 0.4,
        };
        // No two houses are quite the same stone, nor the same plaster.
        let tint = |base: Material, k: f32, spread: f32| {
            let t = r(k);
            let shade = 0.88 + 0.2 * r(k + 0.5);
            Material {
                color: Color::rgb(
                    base.color.r * (1.0 + spread * (t - 0.4)) * shade,
                    base.color.g * (1.0 + spread * 0.6 * (t - 0.5)) * shade,
                    base.color.b * (1.0 - spread * (t - 0.3)) * shade,
                ),
                ..base
            }
        };
        let own = Palette {
            ashlar: tint(pal.ashlar, 7.0, 0.2),
            plaster: tint(pal.plaster, 8.0, 0.4),
            stone_trim: tint(pal.stone_trim, 7.0, 0.2),
            ..*pal
        };
        b.suffix = format!("_{seed}_{i}");
        special(b, &house, i);
        house.build(b, &own);
        b.suffix.clear();
        u += width;
        i += 1;
    }
}

pub fn build(pal: &Palette) -> Square {
    let mut b = Builder::new();

    // ---- The ground: the square's paving, the quays, the bridge and the island.
    paving(&mut b, "cobbles", pal.cobbles, COBBLE_TILE, Vec2::new(-60.0, QUAY_N), Vec2::new(80.0, 60.0), 0.0);
    paving(&mut b, "cobbles", pal.cobbles, COBBLE_TILE, Vec2::new(-120.0, -200.0), Vec2::new(160.0, QUAY_S), 0.0);
    paving(&mut b, "cobbles", pal.cobbles, COBBLE_TILE, Vec2::new(BRIDGE.0, QUAY_S), Vec2::new(BRIDGE.1, QUAY_N), 0.0);
    // The Seine, flowing west under the bridge.
    let river = Material { color: Color::rgb(0.02, 0.028, 0.026), waves: 1.2, flow: Vec2::new(-0.7, 0.05), ..pal.water };
    paving(&mut b, "river", river, 4.0, Vec2::new(-200.0, QUAY_S), Vec2::new(240.0, QUAY_N), RIVER);
    let quay = Material { color: Color::rgb(0.8, 0.76, 0.68), ..pal.ashlar };
    for (z, face) in [(QUAY_N, -1.0f32), (QUAY_S, 1.0)] {
        for (x0, x1) in [(-200.0, BRIDGE.0), (BRIDGE.1, 240.0)] {
            b.block("quay", quay, ASHLAR_TILE, Vec3::new((x0 + x1) * 0.5, (RIVER - 1.0) * 0.5, z - face * 0.5), Vec3::new(x1 - x0, -RIVER + 1.0, 1.0));
            // A low parapet along the quay's edge.
            b.block("quay", quay, ASHLAR_TILE, Vec3::new((x0 + x1) * 0.5, 0.5, z + face * 0.25), Vec3::new(x1 - x0, 1.0, 0.5));
            b.block("trim", pal.stone_trim, ASHLAR_TILE, Vec3::new((x0 + x1) * 0.5, 1.04, z + face * 0.25), Vec3::new(x1 - x0, 0.08, 0.62));
        }
    }
    // The bridge: its deck, parapets, and the piers standing in the river.
    let (bx0, bx1) = BRIDGE;
    b.block("quay", quay, ASHLAR_TILE, Vec3::new((bx0 + bx1) * 0.5, -0.9, (QUAY_N + QUAY_S) * 0.5), Vec3::new(bx1 - bx0 + 1.2, 1.8, QUAY_N - QUAY_S));
    for x in [bx0 - 0.25, bx1 + 0.25] {
        b.block("quay", quay, ASHLAR_TILE, Vec3::new(x, 0.55, (QUAY_N + QUAY_S) * 0.5), Vec3::new(0.5, 1.1, QUAY_N - QUAY_S));
        b.block("trim", pal.stone_trim, ASHLAR_TILE, Vec3::new(x, 1.14, (QUAY_N + QUAY_S) * 0.5), Vec3::new(0.62, 0.08, QUAY_N - QUAY_S));
    }
    for k in 0..3 {
        let z = QUAY_N - 11.0 - k as f32 * 10.5;
        b.block("quay", quay, ASHLAR_TILE, Vec3::new((bx0 + bx1) * 0.5, (RIVER - 1.8) * 0.5, z), Vec3::new(bx1 - bx0 + 2.5, -RIVER - 1.8 + 0.1, 3.0));
        for s in [-1.0f32, 1.0] {
            b.add("quay", quay, ASHLAR_TILE, &Mesh::cone(1.3, 2.4, 3), Mat4::from_rotation_translation(Quat::from_rotation_y(if s < 0.0 { 0.0 } else { PI }), Vec3::new(if s < 0.0 { bx0 - 2.0 } else { bx1 + 2.0 }, RIVER + 1.2, z)));
        }
    }

    // ---- The houses down the left side, tall and old, in the afternoon shadow.
    row(&mut b, pal, Vec3::new(-20.0, 0.0, 60.0), Vec3::NEG_Z, Vec3::X, 60.0 - QUAY_N + 2.0, 1.0, true, |b, house, i| {
        if i == 3 {
            bracket_lantern(b, pal, house.at(house.width * 0.5, 5.2, 0.0), house.out);
        }
    });

    // ---- Houses closing the near side of the square, behind the painter.
    row(&mut b, pal, Vec3::new(31.0, 0.0, WALK_MAX.y + 1.0), Vec3::NEG_X, Vec3::NEG_Z, 52.0, 3.0, false, |_, _, _| {});

    // ---- The right side: the sunlit ochre block with red shopfronts, awnings and signs.
    let ochre = Palette {
        ashlar: Material { color: Color::rgb(1.18, 1.0, 0.72), ..pal.ashlar },
        stone_trim: Material { color: Color::rgb(1.2, 1.05, 0.8), ..pal.stone_trim },
        plaster: Material { color: Color::rgb(1.15, 0.95, 0.66), ..pal.plaster },
        ..*pal
    };
    let signs = [Color::rgb(0.55, 0.06, 0.04), Color::rgb(0.08, 0.2, 0.45), Color::rgb(0.75, 0.55, 0.1), Color::rgb(0.1, 0.3, 0.15)];
    row(&mut b, pal, Vec3::new(30.0, 0.0, QUAY_N - 1.0), Vec3::Z, Vec3::NEG_X, 118.0, 2.0, false, |b, house, i| {
        let r = hash(i as f32 * 5.3, 2.0);
        if (house.origin.z + house.width * 0.5 - (-2.0)).abs() < 30.0 {
            house.awning(b, pal, 0.7, house.width - 0.7, if r < 0.5 { (Color::rgb(0.6, 0.08, 0.06), Color::rgb(0.8, 0.76, 0.66)) } else { (Color::rgb(0.12, 0.25, 0.45), Color::rgb(0.8, 0.76, 0.66)) });
            house.sign(b, &ochre, house.width - 0.3, signs[i % signs.len()]);
        }
    });
    // The corner building itself, drawn again in front of the row's plainer one.
    let corner = House {
        origin: Vec3::new(29.9, 0.0, -20.0),
        along: Vec3::Z,
        out: Vec3::NEG_X,
        width: 36.0,
        floors: 4,
        stone: true,
        ground_floor: GroundFloor::Shop,
        paint: Color::rgb(0.5, 0.07, 0.05),
        seed: 0.3,
        mansard: true,
        depth: 12.0,
        ornate: true,
    };
    b.suffix = "_corner".into();
    corner.build(&mut b, &ochre);
    corner.awning(&mut b, pal, 3.0, 11.0, (Color::rgb(0.62, 0.1, 0.07), Color::rgb(0.85, 0.8, 0.7)));
    corner.awning(&mut b, pal, 22.0, 33.0, (Color::rgb(0.12, 0.28, 0.5), Color::rgb(0.85, 0.8, 0.7)));
    for (k, u) in [1.2, 12.5, 20.5, 34.5].into_iter().enumerate() {
        corner.sign(&mut b, &ochre, u, signs[k]);
    }
    b.suffix.clear();

    // ---- Across the back, left of the bridge: a long block of the quay with a great roof,
    // and the little toll house at the bridge's head.
    let block = House {
        origin: Vec3::new(-44.0, 0.0, QUAY_N + 4.0),
        along: Vec3::X,
        out: Vec3::Z,
        width: 32.0,
        floors: 4,
        stone: true,
        ground_floor: GroundFloor::Windows,
        paint: Color::rgb(0.22, 0.28, 0.33),
        seed: 0.7,
        mansard: false,
        depth: 14.0,
        ornate: true,
    };
    b.suffix = "_quayblock".into();
    block.build(&mut b, pal);
    b.suffix.clear();
    let toll = House {
        origin: Vec3::new(-17.0, 0.0, QUAY_N + 13.0),
        along: Vec3::X,
        out: Vec3::Z,
        width: 6.5,
        floors: 0,
        stone: true,
        ground_floor: GroundFloor::Windows,
        paint: Color::rgb(0.16, 0.26, 0.2),
        seed: 0.2,
        mansard: false,
        depth: 5.0,
        ornate: false,
    };
    b.suffix = "_toll".into();
    toll.build(&mut b, pal);
    b.suffix.clear();

    // ---- The island: the Palais along its quay, the clock tower, the Conciergerie's towers.
    for (x0, x1, floors, seed) in [(-60.0, -12.0, 3, 5.0), (0.0, 17.0, 3, 6.0), (36.0, 90.0, 4, 7.0)] {
        row(&mut b, pal, Vec3::new(x0, 0.0, QUAY_S - 4.0), Vec3::X, Vec3::Z, x1 - x0, seed, floors > 3, |_, _, _| {});
    }
    clock_tower(&mut b, pal, Vec3::new(-6.5, 0.0, QUAY_S - 11.0));
    round_tower(&mut b, pal, Vec3::new(22.0, 0.0, QUAY_S - 5.0), 4.2, 19.0);
    round_tower(&mut b, pal, Vec3::new(31.0, 0.0, QUAY_S - 5.0), 4.0, 18.0);
    // The Palais' own great roofs rising behind.
    b.block("ashlar", pal.ashlar, ASHLAR_TILE, Vec3::new(20.0, 10.0, QUAY_S - 26.0), Vec3::new(40.0, 20.0, 14.0));
    // A four-sided cone turned so its edges, not its corners, face the axes, then stretched
    // over the block.
    let hip = Mat4::from_translation(Vec3::new(20.0, 24.5, QUAY_S - 26.0)) * Mat4::from_scale(Vec3::new(21.0 * 1.414, 9.0, 7.5 * 1.414)) * Mat4::from_rotation_y(FRAC_PI_4);
    b.add("slate", pal.slate, SLATE_TILE, &Mesh::cone(1.0, 1.0, 4), hip);

    // ---- The fountain, and lamps about the square.
    let globe_top = fountain(&mut b, pal);
    for (x, z) in LAMPS {
        lamp_post(&mut b, pal, Vec3::new(x, 0.0, z));
    }

    // ---- Market stalls along the shops on the right.
    for (k, z) in [-3.5f32, 2.0, 7.5].into_iter().enumerate() {
        let canopy = [Color::rgb(0.8, 0.76, 0.66), Color::rgb(0.55, 0.12, 0.08), Color::rgb(0.75, 0.6, 0.35)][k];
        stall(&mut b, pal, Vec3::new(26.8, 0.0, z), Vec2::new(-1.0, 0.0), canopy, k as f32 * 1.7);
    }

    // ---- Carts, a wagon, a coach.
    coach(&mut b, Vec3::new(22.0, 0.0, -24.0), Vec2::new(-0.3, 1.0));
    let horses = vec![
        ("horse_bay", Vec3::new(22.0, 0.0, -24.0) + Vec3::new(-0.3, 0.0, 1.0).normalize() * 4.2 + Vec3::new(0.7, 0.0, 0.2), Vec2::new(-0.3, 1.0)),
        ("horse_chestnut", Vec3::new(22.0, 0.0, -24.0) + Vec3::new(-0.3, 0.0, 1.0).normalize() * 4.2 - Vec3::new(0.7, 0.0, 0.2), Vec2::new(-0.3, 1.0)),
    ];

    // ---- The paving's life: spills and trodden dirt everywhere, straw and dung where the
    // horses, carts and stalls stand, and dirt banked along the foot of every wall.
    let decals = pal.decals;
    let fountain = Vec2::new(FOUNTAIN.x, FOUNTAIN.z);
    for k in 0..300 {
        let r = |j: f32| hash(k as f32 * 3.7, j);
        let at = Vec2::new(WALK_MIN.x + r(1.0) * (WALK_MAX.x - WALK_MIN.x), -56.0 + r(2.0) * 100.0);
        if at.distance(fountain) < FOUNTAIN_RADIUS {
            continue;
        }
        let size = 0.8 + r(3.0) * 2.4;
        b.ground_decal("decal_stain", decals.stain, at, Vec2::new(size, size * (0.6 + r(4.0) * 0.6)), r(5.0) * TAU, 0.004 + r(6.0) * 0.002);
    }
    for (x, z) in [(-19.4f32, 60.0f32), (29.4, 60.0)] {
        let mut zz = QUAY_N;
        while zz < z {
            let r = hash(zz * 1.3 + x, 9.0);
            b.ground_decal("decal_stain", decals.stain, Vec2::new(x - x.signum() * r * 0.3, zz), Vec2::new(1.6, 2.5 + r * 2.0), PI / 2.0, 0.005);
            zz += 2.2 + r * 2.0;
        }
    }
    let busy = [Vec2::new(-6.0, -34.0), Vec2::new(1.0, -44.0), Vec2::new(22.0, -24.0), Vec2::new(26.0, 2.0), Vec2::new(-4.0, -50.0), Vec2::new(8.0, -30.0)];
    for (k, place) in busy.iter().enumerate() {
        for j in 0..14 {
            let r = |i: f32| hash(k as f32 * 11.0 + j as f32 * 1.3, i);
            let at = *place + Vec2::new(r(1.0) - 0.5, r(2.0) - 0.5) * 7.0;
            if j % 2 == 0 {
                b.ground_decal("decal_dung", decals.dung, at, Vec2::splat(0.5 + r(3.0) * 0.4), r(4.0) * TAU, 0.006);
            } else {
                b.ground_decal("decal_straw", decals.straw, at, Vec2::splat(1.0 + r(3.0) * 1.2), r(4.0) * TAU, 0.007);
            }
        }
    }

    // Muck trodden and dropped everywhere a horse or a cart has passed.
    for k in 0..90 {
        let r = |j: f32| hash(k as f32 * 5.9 + 200.0, j);
        let at = Vec2::new(WALK_MIN.x + r(1.0) * (WALK_MAX.x - WALK_MIN.x), -56.0 + r(2.0) * 100.0);
        if at.distance(fountain) < FOUNTAIN_RADIUS {
            continue;
        }
        if k % 3 == 0 {
            b.ground_decal("decal_dung", decals.dung, at, Vec2::splat(0.4 + r(3.0) * 0.3), r(4.0) * TAU, 0.006);
        } else {
            b.ground_decal("decal_straw", decals.straw, at, Vec2::splat(0.8 + r(3.0) * 1.0), r(4.0) * TAU, 0.007);
        }
    }

    // ---- The statues: four round the column's foot, facing out on the diagonals, and the
    // Victory on the globe, facing the square.
    let mut statues = Vec::new();
    for k in 0..4 {
        let a = FRAC_PI_4 + k as f32 * PI / 2.0;
        let out = Vec2::new(a.cos(), a.sin());
        statues.push(Placement { model: "statue", at: FOUNTAIN + Vec3::new(out.x, 0.0, out.y) * 1.3 + Vec3::Y * 5.0, facing: out, scale: 1.2 });
    }
    statues.push(Placement { model: "victory", at: FOUNTAIN + Vec3::Y * globe_top, facing: Vec2::new(-0.2, 1.0), scale: 1.25 });

    let (parts, chimneys) = b.finish();
    // Not every hearth is lit at midday.
    let chimneys = chimneys.into_iter().enumerate().filter(|(k, _)| hash(*k as f32 * 1.7, 3.0) < 0.2).map(|(_, p)| p).collect();
    Square {
        parts,
        chimneys,
        people: people(),
        statues,
        horses,
        vehicles: vehicles(),
        animals: animals(),
        trees: TREES.iter().enumerate().map(|(k, (x, z))| (Vec3::new(*x, 0.0, *z), k as f32 * 2.1, 0.75 + hash(k as f32, 4.0) * 0.2)).collect(),
    }
}

/// Hens and a rooster scratching about the stalls, geese by the toll house, pigs rooting along
/// the left-hand houses, and dogs roaming everywhere.
fn animals() -> Vec<(&'static str, Vec2, f32)> {
    vec![
        ("hen_brown", Vec2::new(24.0, -1.0), 3.0),
        ("hen_brown", Vec2::new(24.5, 4.5), 3.0),
        ("hen_white", Vec2::new(23.5, 1.5), 3.0),
        ("hen_white", Vec2::new(24.0, 9.5), 3.0),
        ("hen_brown", Vec2::new(-15.0, -12.0), 3.0),
        ("rooster", Vec2::new(24.5, 2.5), 3.5),
        ("goose", Vec2::new(-11.0, -40.0), 4.0),
        ("goose", Vec2::new(-10.0, -41.5), 4.0),
        ("pig", Vec2::new(-15.5, -6.0), 5.0),
        ("pig", Vec2::new(-15.0, -28.0), 5.0),
        ("beagle", Vec2::new(0.0, 5.0), 30.0),
        ("beagle", Vec2::new(12.0, -30.0), 30.0),
        ("shepherd", Vec2::new(-5.0, -15.0), 30.0),
    ]
}

/// The crowd: people standing in knots and walking across the square, as in the painting.
fn people() -> Vec<Placement> {
    let mut people = Vec::new();
    let mut put = |model: &'static str, x: f32, z: f32, facing: Vec2| {
        people.push(Placement { model, at: Vec3::new(x, 0.0, z), facing: facing.normalize(), scale: 1.0 });
    };
    // Near the viewer: a couple strolling toward the fountain, a gentleman lifting his hat to
    // them, a maid hurrying the other way.
    put("gentleman_walk", -1.2, 14.0, Vec2::new(0.15, -1.0));
    put("lady3_walk", -0.45, 14.3, Vec2::new(0.15, -1.0));
    put("elder", 2.5, 11.0, Vec2::new(-0.8, 0.5));
    put("maid_walk", 4.8, 16.0, Vec2::new(-0.3, 1.0));
    // Three talking by the fountain's step.
    put("citizen", -2.0, -4.0, Vec2::new(0.8, 0.3));
    put("lady2", -0.6, -3.2, Vec2::new(-0.6, -0.6));
    put("elder", -1.0, -5.3, Vec2::new(-0.2, 1.0));
    // Water-carriers at the basin, and one walking off with full buckets.
    put("water_carrier", 1.4, -11.5, Vec2::new(1.0, 0.0));
    put("water_carrier", 6.0, -6.8, Vec2::new(0.0, -1.0));
    put("water_carrier_walk", 9.0, -2.0, Vec2::new(0.5, 1.0));
    // Market women with their stalls' worth of gossip under the awnings.
    put("market_woman", 26.0, -8.0, Vec2::new(-1.0, 0.2));
    put("market_woman", 25.2, -9.1, Vec2::new(0.6, 0.8));
    put("maid", 24.8, -7.4, Vec2::new(0.7, -0.4));
    put("worker", 25.5, 4.0, Vec2::new(-1.0, 0.0));
    put("worker_walk", 20.0, 8.0, Vec2::new(-0.2, 1.0));
    // Grenadiers of the Guard at the corner, one chatting up a maid.
    put("grenadier", 18.0, 14.0, Vec2::new(-0.9, 0.4));
    put("grenadier", 19.2, 15.2, Vec2::new(-0.6, -0.8));
    put("maid", 18.2, 15.6, Vec2::new(0.4, -0.9));
    // By the toll house and the bridge.
    put("worker", -12.0, -41.0, Vec2::new(0.4, 1.0));
    put("citizen_walk", -3.0, -50.0, Vec2::new(0.1, 1.0));
    put("market_woman_walk", -1.0, -30.0, Vec2::new(-0.3, -1.0));
    // Children at play: by the fountain, and near the stalls.
    put("boy1", -1.0, -8.0, Vec2::new(1.0, 0.0));
    put("girl1", 0.5, -6.0, Vec2::new(-0.5, -1.0));
    put("boy2", 1.5, -9.5, Vec2::new(-1.0, 0.3));
    put("girl2", 20.0, 2.0, Vec2::new(0.2, 1.0));
    put("boy1", 19.0, 0.0, Vec2::new(1.0, 1.0));
    put("girl1", 3.0, 22.0, Vec2::new(0.0, -1.0));
    // Walkers crossing everywhere else.
    let walkers = ["gentleman_walk", "lady3_walk", "worker_walk", "citizen_walk", "elder_walk", "maid_walk", "lady2_walk", "market_woman_walk", "water_carrier_walk"];
    let mut placed = 0;
    let mut k = 0;
    while placed < 26 && k < 400 {
        k += 1;
        let (fx, fz) = (hash(k as f32 * 3.1, 1.0), hash(k as f32 * 3.1, 2.0));
        let x = WALK_MIN.x + 2.0 + fx * (WALK_MAX.x - WALK_MIN.x - 4.0);
        let z = -50.0 + fz * 85.0;
        let p = Vec2::new(x, z);
        if p.distance(Vec2::new(FOUNTAIN.x, FOUNTAIN.z)) < FOUNTAIN_RADIUS + 1.5 || (z > 10.0 && (x - 0.0).abs() < 6.0) {
            continue;
        }
        let a = hash(k as f32, 7.0) * TAU;
        put(walkers[k % walkers.len()], x, z, Vec2::new(a.cos(), a.sin()));
        placed += 1;
    }
    people
}

/// The basin's water level.
pub const BASIN_LEVEL: f32 = 0.86;
/// How fast the jets leave the lions' mouths.
const SPOUT_SPEED: f32 = 3.1;

/// Each lion mask's spout: where its jet leaves the mouth, and how fast and which way.
pub fn spouts() -> Vec<(Vec3, Vec3)> {
    (0..4)
        .map(|k| {
            let dir = Quat::from_rotation_y(k as f32 * PI / 2.0) * Vec3::Z;
            (FOUNTAIN + dir * (1.9 + 0.02 + 0.13) + Vec3::Y * 1.7, dir * SPOUT_SPEED + Vec3::Y * 0.3)
        })
        .collect()
}

/// Where each of the fountain's four jets falls into the basin: where a stone thrown from the
/// spout would land.
pub fn jet_splashes() -> Vec<Vec3> {
    spouts()
        .into_iter()
        .map(|(p, v)| {
            let (g, drop) = (9.81, p.y - BASIN_LEVEL);
            let t = (v.y + (v.y * v.y + 2.0 * g * drop).sqrt()) / g;
            Vec3::new(p.x + v.x * t, BASIN_LEVEL, p.z + v.z * t)
        })
        .collect()
}

/// The solid scenery as physics colliders, each placed at a point: the ground, the fountain's
/// steps, basin wall, pedestal and column, the bollards round it, and the stalls, lamp posts
/// and trees (as upright capsules).
pub fn colliders() -> Vec<(Vec3, Collider)> {
    use voxl::physics::TriMesh;
    let f = FOUNTAIN;
    let solid = |mesh: Mesh| Collider::trimesh(TriMesh::from_mesh(&mesh));
    let mut out = vec![
        (Vec3::ZERO, Collider::ground()),
        (f + Vec3::Y * 0.08, solid(Mesh::cylinder(6.4, 0.16, 48))),
        (f + Vec3::Y * 0.24, solid(Mesh::cylinder(6.0, 0.16, 48))),
        (f, solid(ring(4.65, 5.3, 0.0, 1.07, 48))),
        (f + Vec3::Y * 12.5, Collider::capsule(15.0, 0.72)),
    ];
    for (w, y0, y1) in [(4.6, 0.32, 1.15), (4.8, 1.15, 1.3), (3.8, 1.3, 3.9), (4.45, 3.9, 4.3), (3.3, 4.3, 5.0)] {
        out.push((f + Vec3::Y * (y0 + y1) * 0.5, Collider::cuboid(Vec3::new(w, y1 - y0, w) * 0.5)));
    }
    for p in bollards() {
        out.push((Vec3::new(p.x, 0.6, p.y), Collider::capsule(1.2, 0.22)));
    }
    for (c, r) in obstacles() {
        if c.distance(fountain_center()) > 1.0 {
            out.push((Vec3::new(c.x, 1.25, c.y), Collider::capsule(2.5 + r * 2.0, r * 0.8)));
        }
    }
    out
}

/// The stone bollards ringing the fountain.
pub fn bollards() -> Vec<Vec2> {
    (0..16)
        .map(|k| {
            let a = TAU * (k as f32 + 0.5) / 16.0;
            fountain_center() + Vec2::new(a.cos(), a.sin()) * 7.6
        })
        .collect()
}

/// The water in the fountain's basin: dark, clear and mirror-still but for its ripples.
pub fn basin_water() -> Material {
    Material { color: Color::rgb(0.02, 0.025, 0.025), roughness: 0.02, double_sided: true, ..Default::default() }
}
