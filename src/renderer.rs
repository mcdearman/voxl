use std::sync::Arc;

pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: wgpu::Queue,
    surface: wgpu::Surface,
}