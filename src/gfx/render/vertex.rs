#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    position: [f32; 3],
    tex_coords: [f32; 2],
    tex_idx: u32,
}

impl Vertex {
    pub fn new(pos: [i8; 3], tc: [i8; 2], tex_idx: u32) -> Self {
        Vertex {
            position: [pos[0] as f32, pos[1] as f32, pos[2] as f32],
            tex_coords: [tc[0] as f32, tc[1] as f32],
            tex_idx,
        }
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        const ATTRIBUTE_LAYOUT: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Sint32];
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: ATTRIBUTE_LAYOUT,
        }
    }
}

pub fn create_vertices() -> (Vec<Vertex>, Vec<u16>) {
    let vertices = [
        // front
        Vertex::new([1, 1, 0], [1, 0], 0),
        Vertex::new([1, -1, 0], [1, 1], 0),
        Vertex::new([-1, 1, 0], [0, 0], 0),
        Vertex::new([-1, -1, 0], [0, 1], 0),
        // back
        Vertex::new([1, 1, -2], [1, 0], 0),
        Vertex::new([1, -1, -2], [1, 1], 0),
        Vertex::new([-1, 1, -2], [0, 0], 0),
        Vertex::new([-1, -1, -2], [0, 1], 0),
        // right
        Vertex::new([1, -1, 0], [0, 1], 0),
        Vertex::new([1, 1, 0], [0, 0], 0),
        Vertex::new([1, 1, -2], [1, 0], 0),
        Vertex::new([1, -1, -2], [1, 1], 0),
        // left
        Vertex::new([-1, 1, 0], [1, 0], 0),
        Vertex::new([-1, -1, 0], [1, 1], 0),
        Vertex::new([-1, 1, -2], [0, 0], 0),
        Vertex::new([-1, -1, -2], [0, 1], 0),
        // top
        Vertex::new([1, 1, 0], [1, 1], 1),
        Vertex::new([-1, 1, 0], [0, 1], 1),
        Vertex::new([1, 1, -2], [1, 0], 1),
        Vertex::new([-1, 1, -2], [0, 0], 1),
        // bottom
        Vertex::new([1, -1, 0], [1, 1], 2),
        Vertex::new([-1, -1, 0], [0, 1], 2),
        Vertex::new([1, -1, -2], [1, 0], 2),
        Vertex::new([-1, -1, -2], [0, 0], 2),
    ];

    let indices: &[u16] = &[
        0, 2, 1, 1, 2, 3, // front
        6, 4, 5, 6, 5, 7, // back
        8, 10, 9, 8, 11, 10, // right
        12, 14, 15, 15, 13, 12, // left
        16, 18, 19, 19, 17, 16, // top
        23, 22, 20, 20, 21, 23, // bottom
    ];
    (vertices.to_vec(), indices.to_vec())
}
