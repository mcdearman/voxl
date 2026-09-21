use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use super::gpu::Gpu;

/// Set `path` to save the next rendered frame as a PNG. It is cleared once the frame is saved.
#[derive(Default)]
pub struct Screenshot {
    pub path: Option<PathBuf>,
}

impl Screenshot {
    pub fn request(&mut self, path: impl Into<PathBuf>) {
        self.path = Some(path.into());
    }
}

pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
    bgra: bool,
}

/// Queues a copy of the finished frame into a CPU-readable buffer.
pub(crate) fn copy_to_buffer(
    gpu: &Gpu,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Option<Readback> {
    if !gpu.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
        log::warn!("screenshots aren't supported on this surface");
        return None;
    }
    let (width, height) = (texture.width(), texture.height());
    // Rows in the destination buffer must be padded to a multiple of 256 bytes.
    let padded_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    let bgra = matches!(
        texture.format(),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    Some(Readback {
        buffer,
        width,
        height,
        padded_row,
        bgra,
    })
}

/// Waits for the copy to finish and writes the PNG. Blocks for a moment, which is fine for a
/// screenshot.
pub(crate) fn save(gpu: &Gpu, readback: Readback, path: &Path) -> anyhow::Result<()> {
    let slice = readback.buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    gpu.device.poll(wgpu::PollType::Wait)?;
    receiver.recv()?.context("failed to map the screenshot")?;

    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((readback.width * readback.height * 4) as usize);
    for row in data.chunks_exact(readback.padded_row as usize) {
        for pixel in row[..(readback.width * 4) as usize].chunks_exact(4) {
            let [a, b, c, _] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            pixels.extend(if readback.bgra {
                [c, b, a, 255]
            } else {
                [a, b, c, 255]
            });
        }
    }
    drop(data);

    let Some(image) = image::RgbaImage::from_raw(readback.width, readback.height, pixels) else {
        bail!("screenshot has the wrong size");
    };
    image.save(path)?;
    Ok(())
}
