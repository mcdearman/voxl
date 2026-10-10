use anyhow::Context;

/// A picture made ready for the graphics card ahead of time: every level, from the whole
/// picture down, compressed into the 4 by 4 blocks the card reads directly. A quarter of the
/// memory of plain pixels, and nothing left to do when it is first drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Processed {
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    /// The picture's average colour, in linear light.
    pub average: [f32; 3],
    /// Each level's size in pixels, rounded up to whole blocks, and its blocks.
    pub levels: Vec<(u32, u32, Vec<u8>)>,
}

/// An RGBA8 texture. Store in `Assets<Image>` and refer to it from a `Material`.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// The pixels. Empty for a picture the asset server loaded already processed, unless it
    /// was told to keep them (`AssetServer::keep_pixels`).
    pub data: Vec<u8>,
    /// Colour textures are sRGB; data such as normal and roughness maps are linear.
    pub srgb: bool,
    /// The picture as the graphics card will hold it, when that was made ahead of time.
    /// It is what gets drawn: after changing `data`, set this to `None`.
    pub processed: Option<std::sync::Arc<Processed>>,
}

impl Image {
    /// Decodes a PNG or JPEG.
    pub fn from_bytes(bytes: &[u8], srgb: bool) -> anyhow::Result<Self> {
        let image = image::load_from_memory(bytes)
            .context("unsupported image")?
            .to_rgba8();
        Ok(Self::from_rgba(image.width(), image.height(), image.into_raw(), srgb))
    }

    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>, srgb: bool) -> Self {
        assert_eq!(data.len(), (width * height * 4) as usize, "image data has the wrong size");
        Self {
            width,
            height,
            data,
            srgb,
            processed: None,
        }
    }

    /// A picture known only as processed: its pixels were never kept.
    pub fn from_processed(processed: Processed) -> Self {
        Self {
            width: processed.width,
            height: processed.height,
            data: Vec::new(),
            srgb: processed.format.is_srgb(),
            processed: Some(std::sync::Arc::new(processed)),
        }
    }

    /// Whether the pixels are here to be read.
    pub fn has_pixels(&self) -> bool {
        !self.data.is_empty()
    }

    /// Compresses the picture and its smaller levels into BC7 blocks. None for a picture
    /// whose sides are not multiples of four, which block compression can't hold, or one
    /// with no pixels.
    pub fn process(&self) -> Option<Processed> {
        if !self.has_pixels() || !self.width.is_multiple_of(4) || !self.height.is_multiple_of(4) {
            return None;
        }
        let opaque = self.data.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255);
        let settings = if opaque {
            intel_tex_2::bc7::opaque_very_fast_settings()
        } else {
            intel_tex_2::bc7::alpha_very_fast_settings()
        };
        let levels = self
            .mip_chain()
            .into_iter()
            .map(|(width, height, data)| {
                // The last levels are smaller than a block: their edges are repeated to fill one.
                let (wide, tall) = (width.next_multiple_of(4), height.next_multiple_of(4));
                let padded = if (wide, tall) == (width, height) {
                    data
                } else {
                    let mut padded = Vec::with_capacity((wide * tall * 4) as usize);
                    for y in 0..tall {
                        for x in 0..wide {
                            let at = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
                            padded.extend_from_slice(&data[at..at + 4]);
                        }
                    }
                    padded
                };
                let surface = intel_tex_2::RgbaSurface { data: &padded, width: wide, height: tall, stride: wide * 4 };
                (wide, tall, intel_tex_2::bc7::compress_blocks(&settings, &surface))
            })
            .collect();
        Some(Processed {
            format: if self.srgb { wgpu::TextureFormat::Bc7RgbaUnormSrgb } else { wgpu::TextureFormat::Bc7RgbaUnorm },
            width: self.width,
            height: self.height,
            average: self.average().to_array(),
            levels,
        })
    }

    pub fn solid(rgba: [u8; 4], srgb: bool) -> Self {
        Self::from_rgba(1, 1, rgba.to_vec(), srgb)
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        if self.srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        }
    }

    /// The average colour, in linear light.
    pub fn average(&self) -> glam::Vec3 {
        if let (false, Some(processed)) = (self.has_pixels(), &self.processed) {
            return glam::Vec3::from(processed.average);
        }
        let step = (self.data.len() / 4 / 4096).max(1);
        let decode = |c: u8| {
            let c = c as f32 / 255.0;
            if self.srgb {
                c.powf(2.2)
            } else {
                c
            }
        };
        let (mut sum, mut n) = (glam::Vec3::ZERO, 0.0);
        for p in self.data.as_chunks::<4>().0.iter().step_by(step) {
            sum += glam::Vec3::new(decode(p[0]), decode(p[1]), decode(p[2]));
            n += 1.0;
        }
        sum / f32::max(n, 1.0)
    }

    /// Every mip level down to 1×1, box filtered. Colour is averaged in linear light, so
    /// distant textures don't darken.
    pub fn mip_chain(&self) -> Vec<(u32, u32, Vec<u8>)> {
        let mut levels = vec![(self.width, self.height, self.data.clone())];
        let table: [f32; 256] = std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if self.srgb {
                c.powf(2.2)
            } else {
                c
            }
        });
        let decode = |c: u8| table[c as usize];
        // Encoding back goes through a fine table too: a power per texel is slow on big images.
        let back: Vec<u8> = (0..4096)
            .map(|i| {
                let c = i as f32 / 4095.0;
                let c = if self.srgb { c.powf(1.0 / 2.2) } else { c };
                (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect();
        let encode = |c: f32| back[(c.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize];
        while levels.last().is_some_and(|(w, h, _)| *w > 1 || *h > 1) {
            let (w, h, data) = levels.last().unwrap();
            let (w, h) = (*w as usize, *h as usize);
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = Vec::with_capacity(nw * nh * 4);
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut sum = 0.0;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let sx = (x * 2 + dx).min(w - 1);
                            let sy = (y * 2 + dy).min(h - 1);
                            let v = data[(sy * w + sx) * 4 + c];
                            sum += if c == 3 { v as f32 / 255.0 } else { decode(v) };
                        }
                        let avg = sum / 4.0;
                        next.push(if c == 3 { (avg * 255.0 + 0.5) as u8 } else { encode(avg) });
                    }
                }
            }
            levels.push((nw as u32, nh as u32, next));
        }
        levels
    }

    pub(crate) fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
        match &self.processed {
            Some(processed) => processed.upload(device, queue),
            None => self.upload_levels(device, queue, &self.mip_chain()),
        }
    }

    /// Uploads a mip chain made earlier by [`Self::mip_chain`].
    pub(crate) fn upload_levels(&self, device: &wgpu::Device, queue: &wgpu::Queue, levels: &[(u32, u32, Vec<u8>)]) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, data)) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(*h),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
            );
        }
        texture.create_view(&Default::default())
    }
}

impl Processed {
    /// Whether this device can hold pictures processed this way.
    pub fn supported(device: &wgpu::Device) -> bool {
        device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    pub(crate) fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("processed image"),
            size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            mip_level_count: self.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (wide, tall, blocks)) in self.levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                blocks,
                // Sixteen bytes to a block, and rows counted in blocks.
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(wide / 4 * 16),
                    rows_per_image: Some(tall / 4),
                },
                wgpu::Extent3d { width: *wide, height: *tall, depth_or_array_layers: 1 },
            );
        }
        texture.create_view(&Default::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_compressed_level_by_level_into_blocks() {
        let pixels: Vec<u8> = (0..16 * 8).flat_map(|at| [(at * 2) as u8, 90, 200, 255]).collect();
        let image = Image::from_rgba(16, 8, pixels, true);
        let processed = image.process().unwrap();
        assert_eq!(processed.format, wgpu::TextureFormat::Bc7RgbaUnormSrgb);
        // 16x8, 8x4, 4x2, 2x1, 1x1: each in whole blocks of sixteen bytes.
        let sizes: Vec<(u32, u32, usize)> =
            processed.levels.iter().map(|(w, h, blocks)| (*w, *h, blocks.len())).collect();
        assert_eq!(sizes, [(16, 8, 128), (8, 4, 32), (4, 4, 16), (4, 4, 16), (4, 4, 16)]);
        // A picture kept only as processed still knows its size and its colour.
        let kept = Image::from_processed(processed);
        assert!(!kept.has_pixels() && kept.srgb);
        assert_eq!((kept.width, kept.height), (16, 8));
        assert!((kept.average() - image.average()).length() < 1e-6);
        // Sides that are not multiples of four can't be held in blocks.
        assert!(Image::from_rgba(6, 4, vec![0; 96], false).process().is_none());
    }

    #[test]
    fn mips_run_down_to_one_pixel() {
        let image = Image::from_rgba(8, 2, vec![255; 64], true);
        let chain = image.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, [(8, 2), (4, 1), (2, 1), (1, 1)]);
        assert!(chain.iter().all(|(_, _, d)| d.iter().all(|b| *b == 255)));
    }
}
