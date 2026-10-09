use std::f32::consts::{PI, TAU};

use glam::{Mat3, Mat4, Vec2, Vec3};

use super::Color;
use crate::{assets::Handle, ecs::Component};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGB, multiplied with the entity's `Material` color. White unless set.
    pub color: [f32; 3],
}

impl Vertex {
    // Locations 3-10 belong to the per-instance data.
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 11 => Float32x3
    ];

    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position: position.into(),
            normal: normal.into(),
            uv: uv.into(),
            color: [1.0; 3],
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = [color.r, color.g, color.b];
        self
    }

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// CPU-side triangle mesh. Triangles wind counter-clockwise when seen from the front.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Appends a quad centered at `center`, facing `normal`, spanning `half_u`/`half_v`.
    /// `half_u × half_v` must point along `normal` for the winding to be correct.
    pub fn push_quad(&mut self, center: Vec3, normal: Vec3, half_u: Vec3, half_v: Vec3) {
        let base = self.vertices.len() as u32;
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for (u, v) in corners {
            let position = center + half_u * u + half_v * v;
            let uv = Vec2::new((u + 1.0) * 0.5, (1.0 - v) * 0.5);
            self.vertices.push(Vertex::new(position, normal, uv));
        }
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Appends a copy of `other` moved by `transform`. Combining many small parts into one mesh
    /// draws far faster than the same parts as separate entities.
    pub fn append(&mut self, other: &Mesh, transform: Mat4) {
        let base = self.vertices.len() as u32;
        let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();
        self.vertices.extend(other.vertices.iter().map(|v| {
            Vertex {
                position: transform.transform_point3(v.position.into()).into(),
                normal: (normal_matrix * Vec3::from(v.normal))
                    .normalize_or_zero()
                    .into(),
                ..*v
            }
        }));
        // A mirroring transform turns triangles inside out; swapping two corners undoes it.
        let mirrored = transform.determinant() < 0.0;
        self.indices
            .extend(other.indices.as_chunks::<3>().0.iter().flat_map(|t| {
                let [a, b, c] = t.map(|i| base + i);
                if mirrored {
                    [a, c, b]
                } else {
                    [a, b, c]
                }
            }));
    }

    pub fn cube(size: f32) -> Self {
        let h = size * 0.5;
        let mut mesh = Mesh::default();
        for (normal, u) in [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z),
            (Vec3::Y, Vec3::Z),
            (Vec3::NEG_Y, Vec3::X),
            (Vec3::Z, Vec3::X),
            (Vec3::NEG_Z, Vec3::Y),
        ] {
            let v = normal.cross(u);
            mesh.push_quad(normal * h, normal, u * h, v * h);
        }
        mesh
    }

    /// A box of the given size, centred on the origin, with its edges and corners rounded to
    /// `radius`: each rounded edge is two facets with smooth normals, which catch the light as
    /// worn stone and moulded timber do. Build it at its final size rather than scaling it, so
    /// the rounding stays round.
    pub fn rounded_box(size: Vec3, radius: f32) -> Self {
        let h = size * 0.5;
        let r = radius.min(h.min_element() * 0.9).max(0.0);
        if r <= 1e-5 {
            let mut mesh = Mesh::cube(1.0);
            for v in &mut mesh.vertices {
                v.position = (Vec3::from(v.position) * size).into();
            }
            return mesh;
        }
        let inner = h - Vec3::splat(r);
        let mut mesh = Mesh::default();
        for (normal, u) in [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z),
            (Vec3::Y, Vec3::Z),
            (Vec3::NEG_Y, Vec3::X),
            (Vec3::Z, Vec3::X),
            (Vec3::NEG_Z, Vec3::Y),
        ] {
            let v = normal.cross(u);
            let (hu, hv) = (h.dot(u.abs()), h.dot(v.abs()));
            let steps = |half: f32| [-half, -half + r, half - r, half];
            let base = mesh.vertices.len() as u32;
            for b in steps(hv) {
                for a in steps(hu) {
                    // A point on the flat box, pulled in to the inner box and pushed back out
                    // by the radius along the way it pointed.
                    let p = normal * h.dot(normal.abs()) + u * a + v * b;
                    let q = p.clamp(-inner, inner);
                    let out = (p - q).normalize_or(normal);
                    let pos = q + out * r;
                    mesh.vertices.push(Vertex::new(pos, out, Vec2::new(a, b)));
                }
            }
            for j in 0..3 {
                for i in 0..3 {
                    let k = base + j * 4 + i;
                    mesh.indices.extend([k, k + 1, k + 5, k, k + 5, k + 4]);
                }
            }
        }
        mesh
    }

    /// A square on the XZ plane, facing +Y.
    pub fn plane(size: f32) -> Self {
        let h = size * 0.5;
        let mut mesh = Mesh::default();
        mesh.push_quad(Vec3::ZERO, Vec3::Y, Vec3::Z * h, Vec3::X * h);
        mesh
    }

    pub fn uv_sphere(radius: f32, sectors: u32, stacks: u32) -> Self {
        let mut mesh = Mesh::default();
        for i in 0..=stacks {
            let theta = PI * i as f32 / stacks as f32;
            for j in 0..=sectors {
                let phi = TAU * j as f32 / sectors as f32;
                let normal = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                let uv = Vec2::new(j as f32 / sectors as f32, i as f32 / stacks as f32);
                mesh.vertices.push(Vertex::new(normal * radius, normal, uv));
            }
        }
        let row = sectors + 1;
        for i in 0..stacks {
            for j in 0..sectors {
                let a = i * row + j;
                let b = a + row;
                mesh.indices.extend([a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        mesh
    }

    /// A capped, possibly tapering cylinder along Y, centered on the origin. A top radius of
    /// zero makes a cone.
    pub fn frustum(bottom_radius: f32, top_radius: f32, height: f32, segments: u32) -> Self {
        let mut mesh = Mesh::default();
        let h = height * 0.5;
        let slope = (bottom_radius - top_radius) / height;
        let ring = |i: u32| {
            let (sin, cos) = (TAU * i as f32 / segments as f32).sin_cos();
            Vec3::new(cos, 0.0, sin)
        };
        for i in 0..=segments {
            let r = ring(i);
            let normal = Vec3::new(r.x, slope, r.z).normalize();
            let u = i as f32 / segments as f32;
            mesh.vertices.extend([
                Vertex::new(r * bottom_radius - Vec3::Y * h, normal, Vec2::new(u, 1.0)),
                Vertex::new(r * top_radius + Vec3::Y * h, normal, Vec2::new(u, 0.0)),
            ]);
        }
        for i in 0..segments {
            let (b0, t0, b1, t1) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
            mesh.indices.extend([b0, t0, b1, b1, t0, t1]);
        }
        for (radius, y, normal) in [(bottom_radius, -h, Vec3::NEG_Y), (top_radius, h, Vec3::Y)] {
            if radius <= 0.0 {
                continue;
            }
            let center = mesh.vertices.len() as u32;
            mesh.vertices
                .push(Vertex::new(Vec3::Y * y, normal, Vec2::splat(0.5)));
            for i in 0..=segments {
                let r = ring(i);
                let uv = Vec2::new(r.x, r.z) * 0.5 + 0.5;
                mesh.vertices
                    .push(Vertex::new(r * radius + Vec3::Y * y, normal, uv));
            }
            for i in 0..segments {
                let (a, b) = (center + 1 + i, center + 2 + i);
                mesh.indices.extend(if normal.y < 0.0 {
                    [center, a, b]
                } else {
                    [center, b, a]
                });
            }
        }
        mesh
    }

    pub fn cylinder(radius: f32, height: f32, segments: u32) -> Self {
        Self::frustum(radius, radius, height, segments)
    }

    pub fn cone(radius: f32, height: f32, segments: u32) -> Self {
        Self::frustum(radius, 0.0, height, segments)
    }
}

/// Renders the referenced mesh at this entity's `GlobalTransform`.
#[derive(Clone, Copy, Debug, crate::reflect::Reflect)]
#[reflect(name = "mira.Mesh3d")]
pub struct Mesh3d(pub Handle<Mesh>);

impl Component for Mesh3d {}

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;

    /// Back-face culling drops anything that isn't counter-clockwise from the front.
    fn assert_front_facing(mesh: &Mesh) {
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[tri[i] as usize]);
            let (pa, pb, pc) = (
                Vec3::from(a.position),
                Vec3::from(b.position),
                Vec3::from(c.position),
            );
            let face = (pb - pa).cross(pc - pa);
            if face.length_squared() < 1e-12 {
                continue; // degenerate triangles at the sphere's poles
            }
            let normal = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(face.dot(normal) > 0.0, "triangle {tri:?} winds clockwise");
        }
    }

    #[test]
    fn primitives_wind_counter_clockwise() {
        assert_front_facing(&Mesh::cube(1.0));
        assert_front_facing(&Mesh::plane(1.0));
        assert_front_facing(&Mesh::rounded_box(Vec3::new(2.0, 0.5, 1.0), 0.1));
        assert_front_facing(&Mesh::uv_sphere(1.0, 16, 8));
        assert_front_facing(&Mesh::cylinder(1.0, 2.0, 12));
        assert_front_facing(&Mesh::cone(1.0, 2.0, 12));
        assert_front_facing(&Mesh::frustum(1.0, 0.5, 2.0, 12));
    }

    #[test]
    fn rounded_box_keeps_its_size() {
        let size = Vec3::new(2.0, 0.5, 1.0);
        let mesh = Mesh::rounded_box(size, 0.1);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &mesh.vertices {
            lo = lo.min(Vec3::from(v.position));
            hi = hi.max(Vec3::from(v.position));
        }
        assert!((hi - lo - size).abs().max_element() < 1e-5);
    }

    #[test]
    fn append_keeps_winding_under_any_transform() {
        let cube = Mesh::cube(1.0);
        let mut mesh = Mesh::default();
        mesh.append(
            &cube,
            Mat4::from_scale_rotation_translation(
                Vec3::new(2.0, 0.5, 1.0),
                Quat::from_rotation_y(0.7),
                Vec3::X,
            ),
        );
        mesh.append(&cube, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert_eq!(mesh.vertices.len(), cube.vertices.len() * 2);
        assert_front_facing(&mesh);
    }
}
