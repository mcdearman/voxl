use super::gpu::Gpu;

/// A stack of equally sized square RGBA tiles, sampled in shaders as a `texture_2d_array`.
///
/// Unlike a packed atlas, array layers can repeat across a surface and mipmap without bleeding
/// into their neighbours, which is what block textures need.
pub struct TextureArray {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub layers: u32,
}

impl TextureArray {
    /// `tiles` are `tile_size × tile_size` sRGB RGBA8 images. `tile_size` must be a power of two.
    pub fn from_tiles(gpu: &Gpu, tile_size: u32, tiles: &[Vec<u8>]) -> Self {
        assert!(
            tile_size.is_power_of_two(),
            "tile size must be a power of two"
        );
        assert!(!tiles.is_empty(), "a texture array needs at least one tile");
        let bytes = (tile_size * tile_size * 4) as usize;
        let mip_level_count = tile_size.trailing_zeros() + 1;

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture array"),
            size: wgpu::Extent3d {
                width: tile_size,
                height: tile_size,
                depth_or_array_layers: tiles.len() as u32,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for (layer, tile) in tiles.iter().enumerate() {
            assert_eq!(tile.len(), bytes, "tile {layer} has the wrong size");
            let mut pixels = tile.clone();
            let mut size = tile_size;
            for mip_level in 0..mip_level_count {
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * size),
                        rows_per_image: Some(size),
                    },
                    wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                );
                if size > 1 {
                    pixels = halve(&pixels, size);
                    size /= 2;
                }
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture array sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            // Crisp pixels up close, smooth mips far away.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            view,
            sampler,
            layers: tiles.len() as u32,
        }
    }
}

/// Box-filters an RGBA image down to half its size. Averaging sRGB bytes directly is slightly
/// wrong, but it's invisible on tiny block textures.
fn halve(pixels: &[u8], size: u32) -> Vec<u8> {
    let (size, half) = (size as usize, size as usize / 2);
    let mut out = Vec::with_capacity(half * half * 4);
    for y in 0..half {
        for x in 0..half {
            for c in 0..4 {
                let at = |dx: usize, dy: usize| pixels[((y * 2 + dy) * size + x * 2 + dx) * 4 + c];
                let sum = at(0, 0) as u32 + at(1, 0) as u32 + at(0, 1) as u32 + at(1, 1) as u32;
                out.push((sum / 4) as u8);
            }
        }
    }
    out
}
