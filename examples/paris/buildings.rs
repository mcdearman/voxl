//! Paris buildings of about 1810: tall stone and plaster houses with shops below, carriage
//! gates, wrought-iron balconies and slate mansards, plus the pieces the square needs: a toll
//! house, a medieval clock tower, round towers with conical roofs, and lanterns.
//!
//! Everything is built from boxes and turned shapes. One unit is one metre, +Y is up.

use std::f32::consts::{PI, TAU};

use mira::{
    glam::{Mat3, Vec2, Vec3},
    prelude::*,
    render::Vertex,
    voxel::value_noise,
};

use crate::materials::{Palette, ASHLAR_TILE, BRICK_TILE, IRON_TILE, PLASTER_TILE, SLATE_TILE, WOOD_TILE};

pub fn hash(a: f32, b: f32) -> f32 {
    value_noise(71, Vec3::new(a.floor(), b.floor(), 0.0))
}

/// Meshes by material, each with texture coordinates in real metres over the tile size.
pub struct Builder {
    parts: Vec<(String, Material, f32, Mesh)>,
    /// Added to part names, so each house's stone and plaster keep their own colour.
    pub suffix: String,
    /// The tops of chimney pots, for smoke.
    pub chimneys: Vec<Vec3>,
    /// The house being built, if any: its corner at the top of the ground floor, the way its
    /// front runs, and the way it faces. Wall textures are laid from here, so stone courses
    /// start at the corner and fall on the floor lines rather than running on from the
    /// neighbour.
    pub frame: Option<(Vec3, Vec3, Vec3)>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            parts: Vec::new(),
            suffix: String::new(),
            chimneys: Vec::new(),
            frame: None,
        }
    }

    pub fn mesh(&mut self, name: &str, material: Material, tile: f32) -> &mut Mesh {
        let name = &format!("{name}{}", self.suffix);
        if let Some(i) = self.parts.iter().position(|(n, ..)| n == name) {
            return &mut self.parts[i].3;
        }
        self.parts.push((name.to_string(), material, tile, Mesh::default()));
        &mut self.parts.last_mut().unwrap().3
    }

    /// Appends a shape, then projects texture coordinates from world space along each face's
    /// dominant axis, so textures keep their true scale however a part is stretched.
    pub fn add(&mut self, name: &str, material: Material, tile: f32, shape: &Mesh, transform: Mat4) {
        let frame = self.frame;
        let mesh = self.mesh(name, material, tile);
        let start = mesh.vertices.len();
        mesh.append(shape, transform);
        for v in &mut mesh.vertices[start..] {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal).abs();
            if let Some((origin, along, out)) = frame {
                if n.y < 0.7 {
                    let n = Vec3::from(v.normal);
                    let across = if n.dot(along).abs() < n.dot(out).abs() { along } else { out };
                    // Ashlar repeats once a storey, so its courses meet the string courses.
                    let rise = if (tile - ASHLAR_TILE).abs() < 1e-3 { FLOOR } else { tile };
                    v.uv = [(p - origin).dot(across) / tile, -(p.y - origin.y) / rise];
                    continue;
                }
            }
            let uv = if n.y >= n.x && n.y >= n.z {
                Vec2::new(p.x, p.z)
            } else if n.x >= n.z {
                Vec2::new(p.z, -p.y)
            } else {
                Vec2::new(p.x, -p.y)
            };
            v.uv = (uv / tile).into();
        }
    }

    pub fn block(&mut self, name: &str, material: Material, tile: f32, center: Vec3, size: Vec3) {
        self.rotated(name, material, tile, center, size, Quat::IDENTITY);
    }

    /// A box. Dressed stone (trim, the fountain, quays, towers) has its edges rounded, as
    /// worked and weathered stone does, so its corners catch a line of light.
    pub fn rotated(&mut self, name: &str, material: Material, tile: f32, center: Vec3, size: Vec3, rotation: Quat) {
        let dressed = name == "trim" || ["fountain_dressed", "fountain_stone", "quay", "tower"].iter().any(|n| name.starts_with(n));
        let radius = if dressed { (size.min_element() * 0.3).min(if name == "trim" { 0.025 } else { 0.04 }) } else { 0.0 };
        if radius > 0.003 {
            self.rounded(name, material, tile, center, size, rotation, radius);
        } else {
            self.add(name, material, tile, &CUBE.with(|c| c.clone()), Mat4::from_scale_rotation_translation(size, rotation, center));
        }
    }

    /// A box with rounded edges of the given radius.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded(&mut self, name: &str, material: Material, tile: f32, center: Vec3, size: Vec3, rotation: Quat, radius: f32) {
        self.add(name, material, tile, &Mesh::rounded_box(size, radius), Mat4::from_rotation_translation(rotation, center));
    }

    /// A thin bar from `a` to `b`.
    pub fn rod(&mut self, name: &str, material: Material, tile: f32, a: Vec3, b: Vec3, thickness: f32) {
        let dir = b - a;
        let rotation = Quat::from_rotation_arc(Vec3::Y, dir.normalize());
        self.rotated(name, material, tile, (a + b) * 0.5, Vec3::new(thickness, dir.length() + thickness * 0.3, thickness), rotation);
    }

    /// A decal: a quad spanning `right` and `up` about `center`, its texture laid once across it.
    pub fn decal(&mut self, name: &str, material: Material, center: Vec3, right: Vec3, up: Vec3) {
        let normal = right.cross(up).normalize();
        let mesh = self.mesh(name, material, 1.0);
        let base = mesh.vertices.len() as u32;
        for (du, dv) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let p = center + right * du + up * dv;
            mesh.vertices.push(Vertex::new(p, normal, Vec2::new(du + 0.5, 0.5 - dv)));
        }
        mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A decal lying flat on the ground at height `y`, turned by `angle`.
    pub fn ground_decal(&mut self, name: &str, material: Material, at: Vec2, size: Vec2, angle: f32, y: f32) {
        let (s, c) = angle.sin_cos();
        let right = Vec3::new(s, 0.0, c) * size.x;
        let up = Vec3::new(c, 0.0, -s) * size.y;
        self.decal(name, material, Vec3::new(at.x, y, at.y), right, up);
    }

    pub fn finish(self) -> (Vec<(Material, Mesh)>, Vec<Vec3>) {
        (self.parts.into_iter().map(|(_, m, _, mesh)| (m, mesh)).collect(), self.chimneys)
    }
}

thread_local! {
    static CUBE: Mesh = Mesh::cube(1.0);
}

/// The rotation taking a box's x axis to `x` and its z axis to `z` (perpendicular to `x`).
pub fn oriented(x: Vec3, z: Vec3) -> Quat {
    let (x, z) = (x.normalize(), z.normalize());
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z))
}

#[derive(Clone, Copy, PartialEq)]
pub enum GroundFloor {
    Shop,
    Gate,
    Windows,
}

pub const GROUND_FLOOR: f32 = 4.4;
pub const FLOOR: f32 = 3.2;
/// Windows sit this far back from the face of the wall.
const REVEAL: f32 = 0.32;

/// A house front. Its facade runs `width` metres from `origin` along `along`, and faces `out`;
/// both are horizontal and lie along the world axes.
pub struct House {
    pub origin: Vec3,
    pub along: Vec3,
    pub out: Vec3,
    pub width: f32,
    pub floors: usize,
    pub stone: bool,
    pub ground_floor: GroundFloor,
    /// Shutters, shopfronts and doors.
    pub paint: Color,
    pub seed: f32,
    /// A mansard with dormers, or a plain pitched roof of slate.
    pub mansard: bool,
    /// Metres from the facade to the back wall.
    pub depth: f32,
    /// A grander front: pilasters (on stone), pediments over the first-floor windows, and
    /// keystones over the rest.
    pub ornate: bool,
}

impl House {
    /// A point on the facade: `u` metres along from the house's start, `v` up, `d` metres out
    /// from the wall face.
    pub fn at(&self, u: f32, v: f32, d: f32) -> Vec3 {
        self.origin + self.along * u + Vec3::Y * v + self.out * d
    }

    pub fn height(&self) -> f32 {
        GROUND_FLOOR + FLOOR * self.floors as f32
    }

    /// A box whose extents are given in facade terms: along, up, and out from the wall.
    #[allow(clippy::too_many_arguments)]
    fn slab(&self, b: &mut Builder, name: &str, material: Material, tile: f32, u: (f32, f32), v: (f32, f32), d: (f32, f32)) {
        let a = self.at(u.0, v.0, d.0);
        let c = self.at(u.1, v.1, d.1);
        b.block(name, material, tile, (a + c) * 0.5, (c - a).abs());
    }

    /// A box turned in the facade's plane: `size` is (out, across, length), the length lying
    /// along `dir` (a direction in the facade: along and up).
    #[allow(clippy::too_many_arguments)]
    fn turned(&self, b: &mut Builder, name: &str, material: Material, tile: f32, center: Vec3, size: Vec3, dir: Vec3) {
        b.rotated(name, material, tile, center, size, oriented(self.out, dir));
    }

    pub fn build(&self, b: &mut Builder, pal: &Palette) {
        b.frame = Some((self.at(0.0, GROUND_FLOOR, 0.0), self.along, self.out));
        self.build_front(b, pal);
        b.frame = None;
    }

    fn build_front(&self, b: &mut Builder, pal: &Palette) {
        let wall_upper = if self.stone { ("ashlar", pal.ashlar, ASHLAR_TILE) } else { ("plaster", pal.plaster, PLASTER_TILE) };
        let bays = ((self.width - 0.8) / 2.5).floor().max(2.0) as usize;
        let bay = self.width / bays as f32;

        // ---- Ground floor, always dressed stone.
        match self.ground_floor {
            GroundFloor::Shop => self.shop(b, pal, bays),
            GroundFloor::Gate => self.gate(b, pal),
            GroundFloor::Windows => {
                for i in 0..bays {
                    let c = (i as f32 + 0.5) * bay;
                    self.window(b, pal, c, 0.9, 1.3, 2.5, ("ashlar", pal.ashlar, ASHLAR_TILE), true);
                }
                self.piers(b, bays, bay, 0.0, GROUND_FLOOR, 0.9, 1.3, 2.5, ("ashlar", pal.ashlar, ASHLAR_TILE), true);
            }
        }
        // Plinth, and the heavy stone band that caps the ground floor.
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.0, self.width), (-0.3, 0.45), (0.0, 0.08));
        // Street dirt thrown up against the foot of the wall.
        b.decal("decal_grime", pal.decals.grime, self.at(self.width * 0.5, 0.45, 0.085), self.along * self.width, Vec3::Y * 0.9);
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.0, self.width), (GROUND_FLOOR - 0.25, GROUND_FLOOR + 0.05), (0.0, 0.14));
        self.roll(b, pal, GROUND_FLOOR - 0.27, 0.12, 0.05);

        // ---- Upper floors: tall French windows in every bay.
        for f in 0..self.floors {
            let base = GROUND_FLOOR + f as f32 * FLOOR;
            let (sill, win_h) = if f == 0 { (0.35, 2.55) } else { (0.75, 2.05 - f as f32 * 0.12) };
            let win_w = 1.15;
            for i in 0..bays {
                let c = (i as f32 + 0.5) * bay;
                self.window(b, pal, c, base + sill, win_w, win_h, wall_upper, f > 0);
            }
            self.piers(b, bays, bay, base, FLOOR, base + sill, win_w, win_h, wall_upper, false);
            if self.ornate {
                for i in 0..bays {
                    let c = (i as f32 + 0.5) * bay;
                    let head = base + sill + win_h + 0.16;
                    if f == 0 {
                        self.pediment(b, pal, c, head, win_w, i % 2 == 0);
                    } else {
                        self.keystone(b, pal, c, head);
                    }
                }
            }
            // Plaster cracks between the windows here and there.
            if !self.stone {
                for i in 0..=bays {
                    if hash(i as f32 * 3.3 + self.seed * 41.0, f as f32 * 5.0) > 0.72 {
                        let center = self.at(i as f32 * bay, base + FLOOR * 0.5, 0.004);
                        b.decal("decal_crack", pal.decals.crack, center, self.along * 0.9, Vec3::Y * 1.8);
                    }
                }
            }
            // A thin string course marks each floor.
            if f > 0 {
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.0, self.width), (base - 0.06, base + 0.06), (0.0, 0.07));
            }
            // The étage noble has a continuous balcony of wrought iron.
            if f == 0 && self.floors >= 3 {
                self.balcony(b, pal, base);
            }
        }

        // ---- Quoins up the corners of a plaster front, or pilasters between the bays of an
        // ornate stone one.
        let top = self.height();
        if !self.stone {
            let mut v = GROUND_FLOOR + 0.05;
            let mut long = true;
            while v + 0.33 < top - 0.35 {
                let reach = if long { 0.55 } else { 0.32 };
                for (u0, u1) in [(0.0, reach), (self.width - reach, self.width)] {
                    self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u0, u1), (v, v + 0.33), (0.0, 0.025));
                }
                v += 0.36;
                long = !long;
            }
        } else if self.ornate {
            for i in 0..=bays {
                let u = (i as f32 * bay).clamp(0.3, self.width - 0.3);
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.24, u + 0.24), (GROUND_FLOOR + 0.05, top - 0.55), (0.0, 0.06));
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.3, u + 0.3), (GROUND_FLOOR + 0.05, GROUND_FLOOR + 0.35), (0.0, 0.09));
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.32, u + 0.32), (top - 0.75, top - 0.55), (0.0, 0.1));
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.28, u + 0.28), (top - 0.85, top - 0.75), (0.0, 0.08));
            }
        }

        // ---- Cornice on brackets, and the roof above it.
        let modillions = (self.width / 0.55) as usize;
        for k in 0..modillions {
            let u = (k as f32 + 0.5) * self.width / modillions as f32;
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.07, u + 0.07), (top - 0.55, top - 0.35), (0.0, 0.16));
        }
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.0, self.width), (top - 0.35, top - 0.1), (0.0, 0.18));
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.0, self.width), (top - 0.1, top + 0.12), (0.0, 0.36));
        // An ovolo under the cornice's corona, and a bead under the frieze.
        self.roll(b, pal, top - 0.12, 0.26, 0.09);
        self.roll(b, pal, top - 0.37, 0.12, 0.035);
        self.roof(b, pal, top, bays, bay);

        // ---- The ends and back of the building, closing it in.
        for u in [0.0, self.width] {
            let a = self.at(u, -0.5, 0.0);
            let c = self.at(u, top, -self.depth);
            let size = (c - a).abs() + self.along.abs() * 0.05;
            b.block("side", pal.plaster, PLASTER_TILE, (a + c) * 0.5, size);
        }
        let a = self.at(0.0, -0.5, -self.depth);
        let c = self.at(self.width, top, -self.depth);
        b.block("side", pal.plaster, PLASTER_TILE, (a + c) * 0.5, (c - a).abs() + self.out.abs() * 0.05);
    }

    /// The solid wall between and around a row of windows: piers between them, and the
    /// breast below and lintel band above each.
    #[allow(clippy::too_many_arguments)]
    fn piers(&self, b: &mut Builder, bays: usize, bay: f32, base: f32, height: f32, sill: f32, win_w: f32, win_h: f32, wall: (&str, Material, f32), rusticated: bool) {
        let top = base + height;
        for i in 0..=bays {
            let from = if i == 0 { 0.0 } else { (i as f32 - 0.5) * bay + win_w * 0.5 };
            let to = if i == bays { self.width } else { (i as f32 + 0.5) * bay - win_w * 0.5 };
            self.wall(b, wall, (from, to), (base, top), rusticated);
        }
        for i in 0..bays {
            let c = (i as f32 + 0.5) * bay;
            let (l, r) = (c - win_w * 0.5, c + win_w * 0.5);
            self.wall(b, wall, (l, r), (base, sill), rusticated);
            self.wall(b, wall, (l, r), (sill + win_h, top), rusticated);
        }
    }

    /// A stretch of wall. Rusticated, it is laid in heavy courses with deep sunk joints between,
    /// as the ground floors of stone houses are; the joints line up across the whole front.
    fn wall(&self, b: &mut Builder, wall: (&str, Material, f32), u: (f32, f32), v: (f32, f32), rusticated: bool) {
        let (name, material, tile) = wall;
        if !rusticated {
            self.slab(b, name, material, tile, u, v, (-0.4, 0.0));
            return;
        }
        let (course, joint) = (0.46, 0.04);
        self.slab(b, name, material, tile, u, v, (-0.4, -0.035));
        let mut k = (v.0 / course).floor();
        while k * course < v.1 {
            let c0 = (k * course + joint * 0.5).max(v.0);
            let c1 = ((k + 1.0) * course - joint * 0.5).min(v.1);
            if c1 > c0 {
                let a = self.at(u.0, c0, -0.035);
                let c = self.at(u.1, c1, 0.0);
                b.rounded(name, material, tile, (a + c) * 0.5, (c - a).abs(), Quat::IDENTITY, 0.025);
            }
            k += 1.0;
        }
    }

    /// Over a first-floor window, a pediment: triangular, or a flat hood on consoles.
    fn pediment(&self, b: &mut Builder, pal: &Palette, c: f32, head: f32, w: f32, triangular: bool) {
        let half = w * 0.5 + 0.28;
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - half, c + half), (head, head + 0.09), (0.0, 0.14));
        if triangular {
            let rise = 0.36;
            let length = (half * half + rise * rise).sqrt();
            for s in [-1.0f32, 1.0] {
                let dir = (self.along * -s * half + Vec3::Y * rise).normalize();
                let center = self.at(c + s * half * 0.5, head + 0.09 + rise * 0.5, 0.07);
                self.turned(b, "trim", pal.stone_trim, ASHLAR_TILE, center, Vec3::new(0.14, 0.08, length + 0.04), dir);
            }
            // The tympanum, filled in behind.
            for k in 0..6 {
                let t = (k as f32 + 0.5) / 6.0;
                let wv = half * (1.0 - t) * 2.0 - 0.1;
                if wv > 0.0 {
                    let v = head + 0.09 + rise * t;
                    self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - wv * 0.5, c + wv * 0.5), (v - rise / 12.0, v + rise / 12.0), (0.0, 0.03));
                }
            }
        } else {
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - half - 0.05, c + half + 0.05), (head + 0.09, head + 0.2), (0.0, 0.2));
            for s in [-1.0f32, 1.0] {
                let u = c + s * (half - 0.06);
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.06, u + 0.06), (head - 0.3, head), (0.0, 0.12));
            }
        }
    }

    /// A round moulding running the width of the front: `d` out from the wall, `radius` thick.
    fn roll(&self, b: &mut Builder, pal: &Palette, v: f32, d: f32, radius: f32) {
        let center = self.at(self.width * 0.5, v, d - radius * 0.6);
        let rotation = Quat::from_rotation_arc(Vec3::Y, self.along);
        b.add("trim", pal.stone_trim, ASHLAR_TILE, &Mesh::cylinder(radius, self.width, 16), Mat4::from_rotation_translation(rotation, center));
    }

    /// A keystone at the head of a window.
    fn keystone(&self, b: &mut Builder, pal: &Palette, c: f32, head: f32) {
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - 0.1, c + 0.1), (head - 0.2, head + 0.06), (0.0, 0.06));
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - 0.13, c + 0.13), (head + 0.02, head + 0.08), (0.0, 0.07));
    }

    /// A window set back in the wall: stone reveals, a sill, a painted casement of small panes,
    /// the dark room beyond, and sometimes louvred shutters folded back.
    #[allow(clippy::too_many_arguments)]
    fn window(&self, b: &mut Builder, pal: &Palette, c: f32, sill: f32, w: f32, h: f32, wall: (&str, Material, f32), shutters: bool) {
        let (l, r) = (c - w * 0.5, c + w * 0.5);
        let top = sill + h;
        let (name, material, tile) = wall;
        // Reveals: the thickness of the wall inside the opening.
        for (u0, u1) in [(l - 0.02, l), (r, r + 0.02)] {
            self.slab(b, name, material, tile, (u0, u1), (sill, top), (-REVEAL, 0.0));
        }
        self.slab(b, name, material, tile, (l, r), (top, top + 0.02), (-REVEAL, 0.0));
        // A stone sill projecting a little, and a moulded surround.
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (l - 0.1, r + 0.1), (sill - 0.08, sill), (-REVEAL, 0.07));
        for (u0, u1) in [(l - 0.14, l), (r, r + 0.14)] {
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u0, u1), (sill, top + 0.14), (0.0, 0.035));
        }
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (l - 0.14, r + 0.14), (top, top + 0.16), (0.0, 0.035));

        // The casement: frame, glazing bars, glass, and the room behind.
        let d = -REVEAL + 0.04;
        let paint = Material { color: Color::rgb(0.62, 0.62, 0.58), roughness: 0.55, ..pal.wood };
        let frame = 0.06;
        self.slab(b, "casement", paint, WOOD_TILE, (l, r), (sill, sill + frame), (d - 0.05, d));
        self.slab(b, "casement", paint, WOOD_TILE, (l, r), (top - frame, top), (d - 0.05, d));
        for u in [l, r - frame, c - 0.025] {
            self.slab(b, "casement", paint, WOOD_TILE, (u, u + frame.min(r - u)), (sill, top), (d - 0.05, d));
        }
        let panes = (h / 0.42).round().max(2.0) as usize;
        for k in 1..panes {
            let v = sill + h * k as f32 / panes as f32;
            self.slab(b, "casement", paint, WOOD_TILE, (l, r), (v - 0.018, v + 0.018), (d - 0.04, d - 0.005));
        }
        self.slab(b, "glass", pal.glass, 1.0, (l, r), (sill, top), (d - 0.035, d - 0.03));
        self.slab(b, "interior", pal.interior, 1.0, (l - 0.3, r + 0.3), (sill - 0.5, top + 0.3), (-2.8, -2.75));
        // Soot and rain run down the wall from the sill.
        if hash(c * 5.0 + self.seed * 31.0, sill * 2.0) > 0.35 {
            let length = 0.9 + hash(c * 7.0, sill + self.seed) * 1.4;
            let center = self.at(c, sill - 0.08 - length * 0.5, 0.004);
            b.decal("decal_streak", pal.decals.streak, center, self.along * (w + 0.3), Vec3::Y * length);
        }
        // Curtains, pale linen, drawn aside just inside the glass.
        let curtain = Material { color: Color::rgb(0.55, 0.5, 0.42), roughness: 0.95, ..Default::default() };
        if hash(c * 3.0 + self.seed * 50.0, sill) > 0.4 {
            for (u0, u1) in [(l + 0.02, l + 0.3), (r - 0.3, r - 0.02)] {
                self.slab(b, "curtain", curtain, 1.0, (u0, u1), (sill + 0.05, top - 0.05), (-0.6, -0.55));
            }
        }

        if shutters && hash(c + self.seed * 17.0, sill * 3.0) > 0.45 {
            let painted = Material { color: self.paint, roughness: 0.6, ..pal.wood };
            for (u0, u1) in [(l - 0.16 - w * 0.5, l - 0.16), (r + 0.16, r + 0.16 + w * 0.5)] {
                self.slab(b, "shutter", painted, WOOD_TILE, (u0, u1), (sill, top), (0.02, 0.06));
                // Louvres catching the light, tipped out and down.
                let slats = (h / 0.09) as usize;
                let tipped = (self.out * 0.6f32.cos() - Vec3::Y * 0.6f32.sin()).normalize();
                for k in 0..slats {
                    let v = sill + 0.1 + k as f32 * (h - 0.2) / slats as f32;
                    let center = self.at((u0 + u1) * 0.5, v + 0.025, 0.07);
                    b.rotated("shutter", painted, WOOD_TILE, center, Vec3::new(0.012, 0.05, u1 - u0 - 0.12), oriented(tipped, self.along));
                }
            }
        }
    }

    fn balcony(&self, b: &mut Builder, pal: &Palette, base: f32) {
        let slab_top = base + 0.12;
        self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (0.1, self.width - 0.1), (base - 0.08, slab_top), (0.0, 0.7));
        // Carved stone consoles under the slab, deep at the top and tapering to the wall.
        let n = (self.width / 2.2) as usize;
        for i in 0..=n {
            let u = 0.4 + i as f32 * (self.width - 0.8) / n as f32;
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.09, u + 0.09), (base - 0.2, base - 0.08), (0.0, 0.6));
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.07, u + 0.07), (base - 0.36, base - 0.2), (0.0, 0.36));
            self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (u - 0.055, u + 0.055), (base - 0.5, base - 0.36), (0.0, 0.16));
        }
        // Railing: top and bottom rails, balusters, and a scrolled panel every so often.
        let rail = 1.0;
        for v in [slab_top + 0.08, slab_top + rail] {
            self.slab(b, "iron", pal.iron, IRON_TILE, (0.15, self.width - 0.15), (v - 0.02, v + 0.02), (0.64, 0.68));
        }
        let bars = (self.width / 0.11) as usize;
        for i in 0..=bars {
            let u = 0.15 + i as f32 * (self.width - 0.3) / bars as f32;
            self.slab(b, "iron", pal.iron, IRON_TILE, (u - 0.008, u + 0.008), (slab_top, slab_top + rail), (0.647, 0.663));
        }
        for end in [0.15, self.width - 0.15] {
            self.slab(b, "iron", pal.iron, IRON_TILE, (end - 0.02, end + 0.02), (slab_top, slab_top + rail), (0.0, 0.68));
            self.slab(b, "iron", pal.iron, IRON_TILE, (end - 0.02, end + 0.02), (slab_top + rail - 0.02, slab_top + rail + 0.02), (0.0, 0.68));
        }
        let circles = (self.width / 1.2) as usize;
        for i in 0..circles {
            let u = (i as f32 + 0.5) * self.width / circles as f32;
            let center = self.at(u, slab_top + rail * 0.5, 0.655);
            let ring = 18;
            for k in 0..ring {
                let p = |k: usize| {
                    let a = k as f32 / ring as f32 * TAU;
                    center + Vec3::Y * a.sin() * 0.22 + self.along * a.cos() * 0.22
                };
                b.rod("iron", pal.iron, IRON_TILE, p(k), p(k + 1), 0.012);
            }
        }
    }

    fn shop(&self, b: &mut Builder, pal: &Palette, bays: usize) {
        // Ashlar piers at the ends, a painted timber shopfront between.
        let wall = ("ashlar", pal.ashlar, ASHLAR_TILE);
        self.wall(b, wall, (0.0, 0.6), (0.0, GROUND_FLOOR), true);
        self.wall(b, wall, (self.width - 0.6, self.width), (0.0, GROUND_FLOOR), true);
        self.wall(b, wall, (0.6, self.width - 0.6), (3.6, GROUND_FLOOR), true);
        let painted = Material { color: self.paint, roughness: 0.45, ..pal.wood };
        // Bills pasted on the stone piers either side.
        for (k, u) in [0.3, self.width - 0.3].into_iter().enumerate() {
            let r = hash(u * 2.1 + self.seed * 13.0, k as f32 + 7.0);
            if r > 0.35 {
                let poster = pal.decals.posters[(r * 30.0) as usize % 3];
                let center = self.at(u, 1.75 + r * 0.3, 0.005);
                b.decal(&format!("decal_poster_{}", (r * 30.0) as usize % 3), poster, center, self.along * 0.48, Vec3::Y * 0.66);
            }
        }
        // Fascia board for the shop's name.
        self.slab(b, "shopfront", painted, WOOD_TILE, (0.6, self.width - 0.6), (3.05, 3.6), (0.0, 0.12));
        self.slab(b, "shopfront", painted, WOOD_TILE, (0.6, self.width - 0.6), (2.98, 3.06), (0.0, 0.16));
        // Stallriser below the windows.
        self.slab(b, "shopfront", painted, WOOD_TILE, (0.6, self.width - 0.6), (0.0, 0.75), (-0.05, 0.08));
        let door = bays / 2;
        for i in 0..bays {
            let (l, r) = (0.6 + i as f32 * (self.width - 1.2) / bays as f32, 0.6 + (i + 1) as f32 * (self.width - 1.2) / bays as f32);
            self.slab(b, "shopfront", painted, WOOD_TILE, (l, l + 0.09), (0.0, 3.0), (-0.02, 0.1));
            if i == door {
                // A panelled door, half glazed.
                self.slab(b, "shopfront", painted, WOOD_TILE, (l + 0.09, r), (0.0, 1.3), (-0.12, -0.07));
                self.slab(b, "glass", pal.glass, 1.0, (l + 0.09, r), (1.3, 2.95), (-0.1, -0.095));
                continue;
            }
            // Big panes in a light frame, and the shop's shelves glimpsed inside.
            let mullions = 3;
            for k in 1..mullions {
                let u = l + (r - l) * k as f32 / mullions as f32;
                self.slab(b, "shopfront", painted, WOOD_TILE, (u - 0.02, u + 0.02), (0.75, 3.0), (-0.02, 0.04));
            }
            self.slab(b, "shopfront", painted, WOOD_TILE, (l, r), (1.9, 1.94), (-0.02, 0.04));
            self.slab(b, "glass", pal.glass, 1.0, (l, r), (0.75, 3.0), (0.0, 0.005));
            let warm = Material { color: Color::rgb(0.1, 0.07, 0.045), roughness: 0.8, ..Default::default() };
            for v in [1.0, 1.45, 1.95, 2.45] {
                self.slab(b, "shelves", warm, 1.0, (l + 0.05, r - 0.05), (v, v + 0.03), (-0.6, -0.2));
            }
            self.slab(b, "interior", pal.interior, 1.0, (l, r), (0.75, 3.0), (-1.2, -1.15));
        }
    }

    /// A striped canvas awning over the shop windows, let down against the sun.
    pub fn awning(&self, b: &mut Builder, pal: &Palette, from: f32, to: f32, stripes: (Color, Color)) {
        let (top, reach, drop) = (3.55, 1.6, 0.9);
        let slope = (self.out * reach - Vec3::Y * drop).normalize();
        let length = (reach * reach + drop * drop).sqrt();
        let n = ((to - from) / 0.35).round().max(1.0) as usize;
        for k in 0..n {
            let u0 = from + (to - from) * k as f32 / n as f32;
            let u1 = from + (to - from) * (k + 1) as f32 / n as f32;
            let color = if k % 2 == 0 { stripes.0 } else { stripes.1 };
            let canvas = Material { color, ..pal.canvas };
            let center = self.at((u0 + u1) * 0.5, top, 0.0) + slope * length * 0.5;
            let normal = slope.cross(self.along).normalize();
            // Parts are merged by name, so each colour of canvas is its own.
            let name = &format!("awning_{:.3}_{:.3}_{:.3}", color.r, color.g, color.b);
            b.rotated(name, canvas, 1.0, center, Vec3::new(0.012, length, u1 - u0), oriented(normal, self.along));
            // The scalloped valance hanging from the front edge.
            let front = self.at((u0 + u1) * 0.5, top - drop - 0.14, reach);
            self.slab_at(b, name, canvas, front, u1 - u0, 0.28);
        }
        // Iron arms holding it out.
        let arm = Material { color: Color::rgb(0.05, 0.05, 0.05), roughness: 0.6, metallic: 1.0, ..Default::default() };
        for u in [from + 0.1, to - 0.1] {
            b.rod("iron", arm, IRON_TILE, self.at(u, top - 1.3, 0.02), self.at(u, top - drop, reach), 0.025);
        }
    }

    fn slab_at(&self, b: &mut Builder, name: &str, material: Material, center: Vec3, along: f32, height: f32) {
        b.block(name, material, 1.0, center, self.along.abs() * along + Vec3::Y * height + self.out.abs() * 0.012);
    }

    /// A hanging shop sign on an iron bracket, painted in a bright colour.
    pub fn sign(&self, b: &mut Builder, pal: &Palette, u: f32, color: Color) {
        let arm = self.at(u, 3.9, 0.0);
        b.rod("iron", pal.iron, IRON_TILE, arm, arm + self.out * 1.1, 0.035);
        b.rod("iron", pal.iron, IRON_TILE, arm - Vec3::Y * 0.5, arm + self.out * 0.7, 0.025);
        let board = Material { color, roughness: 0.5, ..pal.wood };
        let center = arm + self.out * 0.75 - Vec3::Y * 0.45;
        b.block("sign", board, WOOD_TILE, center, self.out.abs() * 0.7 + Vec3::Y * 0.6 + self.along.abs() * 0.05);
        let gilt = Material { color: Color::rgb(0.9, 0.62, 0.25), roughness: 0.3, metallic: 1.0, ..Default::default() };
        b.block("gilt", gilt, 1.0, center, self.out.abs() * 0.5 + Vec3::Y * 0.35 + self.along.abs() * 0.07);
    }

    /// A porte cochère: a tall arched carriage entrance with great panelled doors.
    fn gate(&self, b: &mut Builder, pal: &Palette) {
        let wall = ("ashlar", pal.ashlar, ASHLAR_TILE);
        let c = self.width * 0.5;
        let half = 1.5;
        let spring = 2.9;
        self.wall(b, wall, (0.0, c - half), (0.0, GROUND_FLOOR), true);
        self.wall(b, wall, (c + half, self.width), (0.0, GROUND_FLOOR), true);
        // The arch: voussoirs round the opening, and the wall filled in above.
        let n = 15;
        for k in 0..n {
            let a0 = PI * k as f32 / n as f32;
            let a1 = PI * (k + 1) as f32 / n as f32;
            let mid = (a0 + a1) * 0.5;
            let p = self.at(c - mid.cos() * (half + 0.2), spring + mid.sin() * (half + 0.2), 0.02);
            let len = (half + 0.2) * (a1 - a0) + 0.02;
            let tangent = self.along * mid.sin() - Vec3::Y * mid.cos();
            self.turned(b, "trim", pal.stone_trim, ASHLAR_TILE, p, Vec3::new(0.46, 0.42, len), tangent);
        }
        let columns = 12;
        for k in 0..columns {
            let u0 = c - half + 2.0 * half * k as f32 / columns as f32;
            let u1 = c - half + 2.0 * half * (k + 1) as f32 / columns as f32;
            let um = (u0 + u1) * 0.5 - c;
            let arch = spring + (half * half - um * um).max(0.0).sqrt();
            self.slab(b, wall.0, wall.1, wall.2, (u0, u1 + 0.01), (arch + 0.1, GROUND_FLOOR), (-0.4, 0.0));
        }
        // The doors, set well back, with raised panels.
        let door = Material { color: Color::rgb(0.2, 0.26, 0.2), roughness: 0.5, ..pal.wood };
        for k in 0..12 {
            let u0 = c - half + 2.0 * half * k as f32 / 12.0;
            let u1 = u0 + 2.0 * half / 12.0;
            let um = (u0 + u1) * 0.5 - c;
            let top = spring + (half * half - um * um).max(0.0).sqrt();
            self.slab(b, "gate", door, WOOD_TILE, (u0, u1), (0.0, top), (-0.55, -0.5));
        }
        for (v0, v1) in [(0.3, 1.3), (1.5, 2.7)] {
            for (u0, u1) in [(c - half + 0.15, c - 0.1), (c + 0.1, c + half - 0.15)] {
                self.slab(b, "gate", door, WOOD_TILE, (u0, u1), (v0, v1), (-0.5, -0.46));
            }
        }
        self.slab(b, "gate", door, WOOD_TILE, (c - 0.03, c + 0.03), (0.0, spring + half - 0.1), (-0.5, -0.44));
        // Stone bornes to keep carriage wheels off the jambs.
        for u in [c - half - 0.15, c + half + 0.15] {
            borne(b, pal, self.at(u, 0.0, 0.25), 1.0);
        }
    }

    fn roof(&self, b: &mut Builder, pal: &Palette, top: f32, bays: usize, bay: f32) {
        let depth = self.depth;
        // A slope of slate from `from` to `to`, each (out from the wall, height).
        let slope = |b: &mut Builder, from: (f32, f32), to: (f32, f32)| {
            let a = self.at(0.0, from.1, from.0);
            let e = self.at(self.width, to.1, to.0);
            let dir = self.out * (to.0 - from.0) + Vec3::Y * (to.1 - from.1);
            let normal = dir.cross(self.along).normalize();
            let rotation = oriented(normal, self.along);
            b.rotated("slate", pal.slate, SLATE_TILE, (a + e) * 0.5, Vec3::new(0.12, dir.length() + 0.05, self.width + 0.02), rotation);
        };
        let rise;
        if self.mansard {
            // A steep lower slope of slate with dormers in it, a gentler upper slope.
            let steep = 1.25f32;
            rise = 3.2;
            let run = rise / steep.tan();
            slope(b, (0.2, top + 0.1), (0.2 - run, top + 0.1 + rise));
            slope(b, (0.2 - run, top + 0.1 + rise), (-depth * 0.5, top + rise + 1.2));
            slope(b, (-depth - 0.2, top + 0.1), (-depth + run, top + 0.1 + rise));
            slope(b, (-depth + run, top + 0.1 + rise), (-depth * 0.5, top + rise + 1.2));
            for i in 0..bays {
                if (i + (self.seed * 10.0) as usize) % 2 == 1 {
                    continue;
                }
                let c = (i as f32 + 0.5) * bay;
                let base = top + 0.4;
                let d = 0.2 - run * 0.2;
                self.slab(b, "trim", pal.stone_trim, ASHLAR_TILE, (c - 0.6, c + 0.6), (base, base + 1.7), (d - 1.2, d));
                self.slab(b, "glass", pal.glass, 1.0, (c - 0.4, c + 0.4), (base + 0.2, base + 1.45), (d, d + 0.01));
                self.slab(b, "casement", Material { color: Color::rgb(0.6, 0.6, 0.56), ..pal.wood }, WOOD_TILE, (c - 0.02, c + 0.02), (base + 0.2, base + 1.45), (d, d + 0.03));
                let hood = self.at(c, base + 1.9, d - 0.5);
                b.add("slate", pal.slate, SLATE_TILE, &Mesh::cone(0.95, 0.5, 4), Mat4::from_rotation_translation(Quat::from_rotation_y(PI / 4.0), hood));
            }
        } else {
            // A plain pitched roof, ridge in the middle.
            rise = depth * 0.5 * 0.8;
            slope(b, (0.3, top + 0.05), (-depth * 0.5, top + rise));
            slope(b, (-depth - 0.3, top + 0.05), (-depth * 0.5, top + rise));
        }
        // A rounded cap of lead along the ridge.
        let ridge = if self.mansard { top + rise + 1.2 } else { top + rise };
        let cap = Material { color: Color::rgb(0.3, 0.31, 0.33), roughness: 0.45, metallic: 0.6, ..pal.iron };
        b.add("ridge", cap, IRON_TILE, &Mesh::cylinder(0.09, self.width + 0.1, 10), Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Y, self.along), self.at(self.width * 0.5, ridge + 0.03, -depth * 0.5)));
        // The gable ends below a plain roof, or the flat ends of a mansard.
        for u in [0.0, self.width] {
            let n = 10;
            for k in 0..n {
                let t0 = k as f32 / n as f32;
                let d = -depth * (t0 + 0.5 / n as f32);
                let h = rise * (1.0 - ((t0 + 0.5 / n as f32) * 2.0 - 1.0).abs());
                let p = self.at(u, top + h * 0.5, d);
                b.block("side", pal.plaster, PLASTER_TILE, p, self.out.abs() * (depth / n as f32 + 0.02) + Vec3::Y * h + self.along.abs() * 0.05);
            }
        }
        // Chimney stacks on the party walls, with clay pots.
        for u in [0.25, self.width - 0.25] {
            let p = self.at(u, top + rise + 1.4, -depth * 0.45);
            b.block("brick", pal.brick, BRICK_TILE, p, self.out.abs() * 1.4 + Vec3::Y * 2.8 + self.along.abs() * 0.7);
            b.block("trim", pal.stone_trim, ASHLAR_TILE, p + Vec3::Y * 1.35, self.out.abs() * 1.55 + Vec3::Y * 0.12 + self.along.abs() * 0.85);
            for k in 0..3 {
                let pot = p + Vec3::Y * 1.55 + self.out * (-0.4 + k as f32 * 0.4);
                let clay = Material { color: Color::rgb(0.42, 0.2, 0.12), roughness: 0.8, ..Default::default() };
                b.add("pots", clay, 1.0, &Mesh::frustum(0.1, 0.08, 0.5, 16), Mat4::from_translation(pot));
                b.chimneys.push(pot + Vec3::Y * 0.3);
            }
        }
    }
}

/// A stone borne: a squat post with a rounded cap, to keep wheels off walls and fountains.
pub fn borne(b: &mut Builder, pal: &Palette, at: Vec3, scale: f32) {
    let shape = Mesh::frustum(0.2 * scale, 0.14 * scale, 0.7 * scale, 20);
    b.add("trim", pal.stone_trim, ASHLAR_TILE, &shape, Mat4::from_translation(at + Vec3::Y * 0.35 * scale));
    let cap = Mesh::uv_sphere(0.14 * scale, 20, 10);
    b.add("trim", pal.stone_trim, ASHLAR_TILE, &cap, Mat4::from_translation(at + Vec3::Y * 0.7 * scale));
}

/// An oil réverbère hung from an iron bracket on a wall, `out` from it.
pub fn bracket_lantern(b: &mut Builder, pal: &Palette, wall: Vec3, out: Vec3) {
    let tip = wall + out * 1.3;
    b.rod("iron", pal.iron, IRON_TILE, wall, tip, 0.04);
    b.rod("iron", pal.iron, IRON_TILE, wall - Vec3::Y * 0.8, wall + out * 0.9, 0.03);
    b.rod("iron", pal.iron, IRON_TILE, tip, tip - Vec3::Y * 0.5, 0.012);
    lantern_body(b, pal, tip - Vec3::Y * 0.9);
}

/// A lantern standing on top of a post.
pub fn post_lantern(b: &mut Builder, pal: &Palette, at: Vec3) {
    lantern_body(b, pal, at + Vec3::Y * 0.3);
}

/// The lantern itself: a glazed box with a reflector hood, unlit in daylight.
fn lantern_body(b: &mut Builder, pal: &Palette, body: Vec3) {
    for (dx, dz) in [(-0.16, -0.16), (0.16, -0.16), (-0.16, 0.16), (0.16, 0.16)] {
        b.block("iron", pal.iron, IRON_TILE, body + Vec3::new(dx, 0.0, dz), Vec3::new(0.02, 0.5, 0.02));
    }
    for (dx, dz, w, d) in [(0.0, -0.16, 0.32, 0.005), (0.0, 0.16, 0.32, 0.005), (-0.16, 0.0, 0.005, 0.32), (0.16, 0.0, 0.005, 0.32)] {
        b.block("lamp_glass", pal.glass, 1.0, body + Vec3::new(dx, 0.0, dz), Vec3::new(w, 0.46, d));
    }
    b.add("iron", pal.iron, IRON_TILE, &Mesh::cone(0.3, 0.28, 16), Mat4::from_translation(body + Vec3::Y * 0.39));
    b.add("iron", pal.iron, IRON_TILE, &Mesh::cylinder(0.18, 0.04, 16), Mat4::from_translation(body - Vec3::Y * 0.27));
}

/// A pool of still water with a ragged edge, lying on the paving at height `y`.
pub fn puddle(b: &mut Builder, pal: &Palette, tile: f32, center: Vec2, radius: Vec2, y: f32, seed: f32) {
    let mesh = b.mesh("water", pal.water, tile);
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex::new(Vec3::new(center.x, y, center.y), Vec3::Y, center / tile));
    let n = 40;
    for k in 0..=n {
        let a = k as f32 / n as f32 * TAU;
        let ragged = 1.0 + 0.35 * (value_noise(13, Vec3::new(a.cos() * 2.0 + seed, a.sin() * 2.0, seed)) - 0.5) * 2.0
            + 0.12 * (value_noise(17, Vec3::new(a.cos() * 6.0 + seed, a.sin() * 6.0, seed)) - 0.5) * 2.0;
        let p = center + Vec2::new(a.cos() * radius.x, a.sin() * radius.y) * ragged;
        mesh.vertices.push(Vertex::new(Vec3::new(p.x, y, p.y), Vec3::Y, p / tile));
    }
    for k in 0..n as u32 {
        mesh.indices.extend([base, base + 2 + k, base + 1 + k]);
    }
}
