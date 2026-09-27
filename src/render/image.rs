use anyhow::Context;

/// An RGBA8 texture. Store in `Assets<Image>` and refer to it from a `Material`.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Colour textures are sRGB; data such as normal and roughness maps are linear.
    pub srgb: bool,
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
        }
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
        self.upload_levels(device, queue, &self.mip_chain())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_run_down_to_one_pixel() {
        let image = Image::from_rgba(8, 2, vec![255; 64], true);
        let chain = image.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, [(8, 2), (4, 1), (2, 1), (1, 1)]);
        assert!(chain.iter().all(|(_, _, d)| d.iter().all(|b| *b == 255)));
    }
}
