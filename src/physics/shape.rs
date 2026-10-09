//! Collision shapes: their bounds, mass properties and ray casts, and triangle meshes with a
//! bounding volume hierarchy for static level geometry.

use std::sync::Arc;

use glam::{Mat3, Quat, Vec3};

use crate::reflect::{Reflect, ReflectError, Schema, Value};

/// A position and orientation, without scale.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(name = "voxl.Iso")]
pub struct Iso {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Iso {
    pub const IDENTITY: Self = Self { position: Vec3::ZERO, rotation: Quat::IDENTITY };

    pub fn new(position: Vec3, rotation: Quat) -> Self {
        Self { position, rotation }
    }

    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        self.position + self.rotation * p
    }

    pub fn inverse_transform_point(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.position)
    }

    pub fn mul(&self, other: &Iso) -> Iso {
        Iso { position: self.transform_point(other.position), rotation: (self.rotation * other.rotation).normalize() }
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self { min: Vec3::splat(f32::MAX), max: Vec3::splat(f32::MIN) };

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        points.into_iter().fold(Self::EMPTY, |b, p| Self { min: b.min.min(p), max: b.max.max(p) })
    }

    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    pub fn expand(&self, by: f32) -> Aabb {
        Aabb { min: self.min - Vec3::splat(by), max: self.max + Vec3::splat(by) }
    }

    pub fn overlaps(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    pub fn contains(&self, p: Vec3) -> bool {
        self.min.cmple(p).all() && p.cmple(self.max).all()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Where a ray enters the box, if it does within `max_t`.
    pub fn ray(&self, origin: Vec3, inv_dir: Vec3, max_t: f32) -> Option<f32> {
        let t1 = (self.min - origin) * inv_dir;
        let t2 = (self.max - origin) * inv_dir;
        let near = t1.min(t2).max_element().max(0.0);
        let far = t1.max(t2).min_element().min(max_t);
        (near <= far).then_some(near)
    }
}

/// A shape to collide. Shapes sit at their collider's origin (see `Collider::offset`).
#[derive(Clone, Debug, Reflect)]
#[reflect(name = "voxl.Shape")]
pub enum Shape {
    Sphere { radius: f32 },
    /// A box with the given half extents along its local axes.
    Cuboid { half: Vec3 },
    /// A capsule along local Y: a segment `half_height` either side of the origin, swept by
    /// `radius`.
    Capsule { half_height: f32, radius: f32 },
    /// Everything below the plane through the origin facing local `normal`: the ground.
    HalfSpace { normal: Vec3 },
    /// Triangles, for static level geometry. Only spheres, capsules and boxes collide with it.
    TriMesh(Arc<TriMesh>),
}

impl Shape {
    pub fn sphere(radius: f32) -> Self {
        Shape::Sphere { radius }
    }

    pub fn cuboid(half: Vec3) -> Self {
        Shape::Cuboid { half }
    }

    /// A capsule standing `height` tall overall.
    pub fn capsule(height: f32, radius: f32) -> Self {
        Shape::Capsule { half_height: (height * 0.5 - radius).max(0.0), radius }
    }

    pub fn ground() -> Self {
        Shape::HalfSpace { normal: Vec3::Y }
    }

    /// Bounds in world space at `iso`.
    pub fn aabb(&self, iso: &Iso) -> Aabb {
        match self {
            Shape::Sphere { radius } => Aabb { min: iso.position - Vec3::splat(*radius), max: iso.position + Vec3::splat(*radius) },
            Shape::Cuboid { half } => {
                let m = Mat3::from_quat(iso.rotation);
                let extent = m.x_axis.abs() * half.x + m.y_axis.abs() * half.y + m.z_axis.abs() * half.z;
                Aabb { min: iso.position - extent, max: iso.position + extent }
            }
            Shape::Capsule { half_height, radius } => {
                let axis = iso.rotation * Vec3::Y * *half_height;
                let extent = axis.abs() + Vec3::splat(*radius);
                Aabb { min: iso.position - extent, max: iso.position + extent }
            }
            Shape::HalfSpace { .. } => Aabb { min: Vec3::splat(-1e7), max: Vec3::splat(1e7) },
            Shape::TriMesh(mesh) => {
                let b = mesh.bounds();
                Aabb::from_points((0..8).map(|i| {
                    let c = Vec3::new(
                        if i & 1 == 0 { b.min.x } else { b.max.x },
                        if i & 2 == 0 { b.min.y } else { b.max.y },
                        if i & 4 == 0 { b.min.z } else { b.max.z },
                    );
                    iso.transform_point(c)
                }))
            }
        }
    }

    /// Volume, and the inertia tensor's diagonal for unit density, about the shape's centre.
    pub fn mass_properties(&self) -> (f32, Vec3) {
        match self {
            Shape::Sphere { radius } => {
                let v = 4.0 / 3.0 * std::f32::consts::PI * radius.powi(3);
                (v, Vec3::splat(0.4 * v * radius * radius))
            }
            Shape::Cuboid { half } => {
                let d = *half * 2.0;
                let v = d.x * d.y * d.z;
                (v, Vec3::new(d.y * d.y + d.z * d.z, d.x * d.x + d.z * d.z, d.x * d.x + d.y * d.y) * v / 12.0)
            }
            Shape::Capsule { half_height, radius } => {
                let (r, h) = (*radius, *half_height * 2.0);
                let cylinder = std::f32::consts::PI * r * r * h;
                let caps = 4.0 / 3.0 * std::f32::consts::PI * r.powi(3);
                let v = cylinder + caps;
                // Cylinder, plus the two hemispheres moved out to the ends.
                let iy = cylinder * r * r * 0.5 + caps * 0.4 * r * r;
                let ix = cylinder * (3.0 * r * r + h * h) / 12.0 + caps * (0.4 * r * r + h * h / 4.0 + 3.0 * h * r / 8.0);
                (v, Vec3::new(ix, iy, ix))
            }
            // Infinitely heavy: only ever static.
            Shape::HalfSpace { .. } | Shape::TriMesh(_) => (0.0, Vec3::ZERO),
        }
    }

    /// Where a ray (world space) first meets the shape at `iso`, and the surface normal there.
    pub fn ray(&self, iso: &Iso, origin: Vec3, dir: Vec3, max_t: f32) -> Option<(f32, Vec3)> {
        let o = iso.inverse_transform_point(origin);
        let d = iso.rotation.inverse() * dir;
        let hit = match self {
            Shape::Sphere { radius } => ray_sphere(o, d, Vec3::ZERO, *radius),
            Shape::Cuboid { half } => ray_box(o, d, *half),
            Shape::Capsule { half_height, radius } => ray_capsule(o, d, *half_height, *radius),
            Shape::HalfSpace { normal } => {
                let denom = d.dot(*normal);
                let t = -o.dot(*normal) / denom;
                (denom < 0.0 && t >= 0.0).then_some((t, *normal))
            }
            Shape::TriMesh(mesh) => mesh.ray(o, d, max_t),
        };
        hit.filter(|(t, _)| *t <= max_t).map(|(t, n)| (t, iso.rotation * n))
    }
}

fn ray_sphere(o: Vec3, d: Vec3, c: Vec3, r: f32) -> Option<(f32, Vec3)> {
    let m = o - c;
    let b = m.dot(d);
    let cc = m.dot(m) - r * r;
    if cc > 0.0 && b > 0.0 {
        return None;
    }
    let a = d.dot(d);
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return None;
    }
    let t = ((-b - disc.sqrt()) / a).max(0.0);
    Some((t, (o + d * t - c).normalize_or(Vec3::Y)))
}

fn ray_box(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let inv = d.recip();
    let t1 = (-half - o) * inv;
    let t2 = (half - o) * inv;
    let near = t1.min(t2);
    let far = t1.max(t2);
    let (tn, tf) = (near.max_element(), far.min_element());
    if tn > tf || tf < 0.0 {
        return None;
    }
    let t = tn.max(0.0);
    let axis = if near.x >= near.y && near.x >= near.z { 0 } else if near.y >= near.z { 1 } else { 2 };
    let mut n = Vec3::ZERO;
    n[axis] = -d[axis].signum();
    Some((t, n))
}

fn ray_capsule(o: Vec3, d: Vec3, hh: f32, r: f32) -> Option<(f32, Vec3)> {
    let mut best: Option<(f32, Vec3)> = None;
    let mut consider = |hit: Option<(f32, Vec3)>| {
        if let Some(h) = hit {
            if best.is_none_or(|b| h.0 < b.0) {
                best = Some(h);
            }
        }
    };
    // The cylinder part, about Y.
    let (ox, dx) = (glam::Vec2::new(o.x, o.z), glam::Vec2::new(d.x, d.z));
    let a = dx.dot(dx);
    if a > 1e-12 {
        let b = ox.dot(dx);
        let c = ox.dot(ox) - r * r;
        let disc = b * b - a * c;
        if disc >= 0.0 {
            let t = (-b - disc.sqrt()) / a;
            let y = o.y + d.y * t;
            if t >= 0.0 && y.abs() <= hh {
                let p = o + d * t;
                consider(Some((t, Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X))));
            }
        }
    }
    consider(ray_sphere(o, d, Vec3::Y * hh, r));
    consider(ray_sphere(o, d, -Vec3::Y * hh, r));
    best
}

/// Triangles with a bounding volume hierarchy over them.
#[derive(Debug)]
pub struct TriMesh {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    nodes: Vec<Node>,
    /// Triangle indices, ordered so each leaf's are contiguous.
    order: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
struct Node {
    bounds: Aabb,
    /// For a leaf, the first entry in `order` and the count; for an inner node, the index of
    /// the left child (the right follows it) and 0.
    start: u32,
    count: u32,
}

/// Saved as its vertices and triangles; the hierarchy over them is rebuilt on loading.
impl Reflect for Arc<TriMesh> {
    fn type_name() -> &'static str {
        "voxl.TriMesh"
    }

    fn to_value(&self) -> Value {
        Value::Map(vec![
            ("vertices".into(), self.vertices.to_value()),
            ("triangles".into(), self.triangles.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        let part = |name: &'static str| {
            value
                .field(name)
                .ok_or_else(|| ReflectError::missing("voxl.TriMesh", name))
        };
        let vertices =
            Vec::<Vec3>::from_value(part("vertices")?).map_err(|err| err.inside("vertices"))?;
        let triangles = Vec::<[u32; 3]>::from_value(part("triangles")?)
            .map_err(|err| err.inside("triangles"))?;
        if let Some(index) = triangles
            .iter()
            .flatten()
            .find(|&&i| i as usize >= vertices.len())
        {
            return Err(ReflectError::new(format!(
                "triangle names vertex {index}, but there are {}",
                vertices.len()
            )));
        }
        Ok(Arc::new(TriMesh::new(vertices, triangles)))
    }

    fn schema() -> Schema {
        Schema::Struct {
            name: "voxl.TriMesh",
            fields: Box::new(Schema::Fields(vec![
                ("vertices", Vec::<Vec3>::schema()),
                ("triangles", Vec::<[u32; 3]>::schema()),
            ])),
        }
    }
}

impl TriMesh {
    pub fn new(vertices: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Self {
        let mut mesh = Self { vertices, triangles, nodes: Vec::new(), order: Vec::new() };
        mesh.order = (0..mesh.triangles.len() as u32).collect();
        if !mesh.triangles.is_empty() {
            let n = mesh.triangles.len();
            mesh.build(0, n);
        }
        mesh
    }

    /// From an engine mesh (in its own space; place it with the collider's transform).
    pub fn from_mesh(mesh: &crate::render::Mesh) -> Self {
        let vertices = mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect();
        let triangles = mesh.indices.as_chunks::<3>().0.to_vec();
        Self::new(vertices, triangles)
    }

    pub fn triangle(&self, i: u32) -> [Vec3; 3] {
        let t = self.triangles[i as usize];
        [self.vertices[t[0] as usize], self.vertices[t[1] as usize], self.vertices[t[2] as usize]]
    }

    fn tri_bounds(&self, i: u32) -> Aabb {
        Aabb::from_points(self.triangle(i))
    }

    fn build(&mut self, start: usize, end: usize) -> usize {
        let bounds = self.order[start..end].iter().fold(Aabb::EMPTY, |b, &i| b.union(&self.tri_bounds(i)));
        let index = self.nodes.len();
        self.nodes.push(Node { bounds, start: start as u32, count: (end - start) as u32 });
        if end - start <= 4 {
            return index;
        }
        // Split at the median along the longest axis of the triangles' centres.
        let centres = Aabb::from_points(self.order[start..end].iter().map(|&i| self.tri_bounds(i).center()));
        let size = centres.max - centres.min;
        let axis = if size.x >= size.y && size.x >= size.z { 0 } else if size.y >= size.z { 1 } else { 2 };
        let mid = (start + end) / 2;
        let mut part: Vec<u32> = self.order[start..end].to_vec();
        part.sort_by(|&a, &b| self.tri_bounds(a).center()[axis].total_cmp(&self.tri_bounds(b).center()[axis]));
        self.order[start..end].copy_from_slice(&part);
        let left = self.build(start, mid);
        self.build(mid, end);
        self.nodes[index] = Node { bounds, start: left as u32, count: 0 };
        index
    }

    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb { min: Vec3::ZERO, max: Vec3::ZERO }, |n| n.bounds)
    }

    /// Every triangle whose bounds overlap `query` (in the mesh's space).
    pub fn overlapping(&self, query: &Aabb, out: &mut Vec<u32>) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let node = self.nodes[i];
            if !node.bounds.overlaps(query) {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    if self.tri_bounds(t).overlaps(query) {
                        out.push(t);
                    }
                }
            } else {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
            }
        }
    }

    fn ray(&self, o: Vec3, d: Vec3, max_t: f32) -> Option<(f32, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = d.recip();
        let mut best: Option<(f32, Vec3)> = None;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let node = self.nodes[i];
            let limit = best.map_or(max_t, |b| b.0);
            if node.bounds.ray(o, inv, limit).is_none() {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let [a, b, c] = self.triangle(t);
                    if let Some(hit) = ray_triangle(o, d, a, b, c) {
                        if hit.0 <= best.map_or(max_t, |b| b.0) {
                            best = Some(hit);
                        }
                    }
                }
            } else {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
            }
        }
        best
    }
}

/// Möller-Trumbore, both faces; the normal faces the ray's origin.
pub fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec3)> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-10 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    if t < 0.0 {
        return None;
    }
    let mut n = e1.cross(e2).normalize_or(Vec3::Y);
    if n.dot(d) > 0.0 {
        n = -n;
    }
    Some((t, n))
}
