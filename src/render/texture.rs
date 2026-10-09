use super::{gpu::Gpu, image::Image};

/// A stack of equally sized textures, sampled in shaders as a `texture_2d_array`.
///
/// Unlike a packed atlas, array layers can repeat across a surface and mipmap without bleeding
/// into their neighbours, which is what block and terrain textures need.
pub struct TextureArray {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub layers: u32,
}

impl TextureArray {
    /// `tiles` are `tile_size × tile_size` sRGB RGBA8 images, magnified without smoothing for
    /// crisp pixel-art blocks.
    pub fn from_tiles(gpu: &Gpu, tile_size: u32, tiles: &[Vec<u8>]) -> Self {
        let images: Vec<Image> = tiles
            .iter()
            .map(|t| Image::from_rgba(tile_size, tile_size, t.clone(), true))
            .collect();
        Self::build(gpu, &images, true, wgpu::FilterMode::Nearest)
    }

    /// Photographic textures, all the same size; `srgb` for colour, not for data maps.
    pub fn from_images(gpu: &Gpu, images: &[Image], srgb: bool) -> Self {
        Self::build(gpu, images, srgb, wgpu::FilterMode::Linear)
    }

    fn build(gpu: &Gpu, images: &[Image], srgb: bool, mag_filter: wgpu::FilterMode) -> Self {
        assert!(!images.is_empty(), "a texture array needs at least one layer");
        let (width, height) = (images[0].width, images[0].height);
        for image in images {
            assert_eq!((image.width, image.height), (width, height), "layers differ in size");
        }
        // Filtering big layers down is slow; do them side by side.
        let chains: Vec<_> = std::thread::scope(|s| {
            let jobs: Vec<_> = images
                .iter()
                .map(|image| s.spawn(move || Image { srgb, ..image.clone() }.mip_chain()))
                .collect();
            jobs.into_iter()
                .map(|j| j.join().expect("mip generation panicked"))
                .collect()
        });
        let mip_level_count = chains[0].len() as u32;

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture array"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: images.len() as u32,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, chain) in chains.iter().enumerate() {
            for (mip_level, (w, h, pixels)) in chain.iter().enumerate() {
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: mip_level as u32,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * w),
                        rows_per_image: Some(*h),
                    },
                    wgpu::Extent3d {
                        width: *w,
                        height: *h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let smooth = mag_filter == wgpu::FilterMode::Linear;
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture array sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: if smooth { 16 } else { 1 },
            ..Default::default()
        });
        Self {
            view,
            sampler,
            layers: images.len() as u32,
        }
    }
}
