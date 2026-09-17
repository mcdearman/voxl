use std::f32::consts::{PI, TAU};

use glam::{Vec2, Vec3};

use crate::{assets::Handle, ecs::Component};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];

    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position: position.into(),
            normal: normal.into(),
            uv: uv.into(),
        }
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
}

/// Renders the referenced mesh at this entity's `GlobalTransform`.
#[derive(Clone, Copy, Debug)]
pub struct Mesh3d(pub Handle<Mesh>);

impl Component for Mesh3d {}

#[cfg(test)]
mod tests {
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
        assert_front_facing(&Mesh::uv_sphere(1.0, 16, 8));
    }
}
