//! Images loaded from files, kept up to date as the files change.
//!
//! Load a texture through [`ImageFiles`] instead of decoding it yourself and it is watched:
//! save a new version from a paint program and every material using it shows the change a
//! moment later, without restarting.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;

use super::Image;
use crate::{
    assets::{Assets, Handle},
    ecs::ResMut,
};

/// A file's modification time and size: enough to tell that it was saved again.
type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

struct Entry {
    path: PathBuf,
    handle: Handle<Image>,
    srgb: bool,
    /// The version of the file that is loaded.
    current: Option<Stamp>,
    /// A newer version seen on the last check, waiting to see whether it is still changing.
    settling: Option<Stamp>,
    /// A version that failed to decode, so it isn't retried until the file changes again.
    rejected: Option<Stamp>,
}

/// The images that came from files.
pub struct ImageFiles {
    entries: Vec<Entry>,
    last_check: Option<Instant>,
    /// How often the files are checked for changes.
    pub check_interval: Duration,
    /// Whether changed files are reloaded. On in development builds.
    pub hot_reload: bool,
}

impl Default for ImageFiles {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            last_check: None,
            check_interval: Duration::from_millis(250),
            hot_reload: cfg!(debug_assertions),
        }
    }
}

fn decode(path: &Path, srgb: bool) -> anyhow::Result<Image> {
    let bytes = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    Image::from_bytes(&bytes, srgb).with_context(|| format!("can't decode {}", path.display()))
}

impl ImageFiles {
    /// Loads a PNG or JPEG and watches the file. `srgb` is true for colour textures and false
    /// for data such as normal and roughness maps.
    pub fn load(
        &mut self,
        images: &mut Assets<Image>,
        path: impl AsRef<Path>,
        srgb: bool,
    ) -> anyhow::Result<Handle<Image>> {
        let path = path.as_ref().to_owned();
        let current = stamp(&path);
        let handle = images.add(decode(&path, srgb)?);
        self.entries.push(Entry {
            path,
            handle,
            srgb,
            current,
            settling: None,
            rejected: None,
        });
        Ok(handle)
    }

    /// Reloads every image whose file has changed, right now, without waiting for the file to
    /// settle. Returns how many were reloaded. The frame loop does this by itself (with the
    /// wait); call it when you know a file has just been written.
    pub fn reload_changed(&mut self, images: &mut Assets<Image>) -> usize {
        let mut reloaded = 0;
        for entry in &mut self.entries {
            let now = stamp(&entry.path);
            if now.is_some() && now != entry.current && entry.reload(images, now) {
                reloaded += 1;
            }
        }
        reloaded
    }

    fn check(&mut self, images: &mut Assets<Image>) {
        if !self.hot_reload || self.entries.is_empty() {
            return;
        }
        if self
            .last_check
            .is_some_and(|t| t.elapsed() < self.check_interval)
        {
            return;
        }
        self.last_check = Some(Instant::now());
        for entry in &mut self.entries {
            let now = stamp(&entry.path);
            if now.is_none() || now == entry.current || now == entry.rejected {
                entry.settling = None;
            } else if now == entry.settling {
                // Unchanged since the last check: whatever was writing it has finished.
                entry.settling = None;
                entry.reload(images, now);
            } else {
                entry.settling = now;
            }
        }
    }
}

impl Entry {
    fn reload(&mut self, images: &mut Assets<Image>, version: Option<Stamp>) -> bool {
        match decode(&self.path, self.srgb) {
            Ok(image) => {
                // Writing through `get_mut` marks the image changed, which makes the renderer
                // upload it again and rebuild the materials that use it.
                let Some(slot) = images.get_mut(self.handle) else {
                    return false;
                };
                *slot = image;
                self.current = version;
                self.rejected = None;
                log::info!("reloaded {}", self.path.display());
                true
            }
            Err(err) => {
                log::error!("{err:#}; still using the old image");
                self.rejected = version;
                false
            }
        }
    }
}

pub(crate) fn reload_changed_images(
    mut files: ResMut<ImageFiles>,
    mut images: ResMut<Assets<Image>>,
) {
    files.check(&mut images);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, rgba: [u8; 4]) {
        image::RgbaImage::from_pixel(2, 2, image::Rgba(rgba))
            .save(path)
            .unwrap();
    }

    #[test]
    fn a_saved_image_is_reloaded_and_a_broken_one_is_not() {
        let dir = std::env::temp_dir().join(format!("voxl-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tile.png");
        write(&path, [255, 0, 0, 255]);

        let mut images = Assets::<Image>::default();
        let mut files = ImageFiles::default();
        let handle = files.load(&mut images, &path, true).unwrap();
        assert_eq!(images.get(handle).unwrap().data[..4], [255, 0, 0, 255]);
        images.take_changes();
        assert_eq!(
            files.reload_changed(&mut images),
            0,
            "nothing has changed yet"
        );

        write(&path, [0, 0, 255, 255]);
        assert_eq!(files.reload_changed(&mut images), 1);
        assert_eq!(images.get(handle).unwrap().data[..4], [0, 0, 255, 255]);
        // The renderer learns of it the way it learns of any changed asset.
        assert_eq!(images.take_changes().0, [handle.id()]);

        std::fs::write(&path, b"not an image").unwrap();
        assert_eq!(files.reload_changed(&mut images), 0);
        assert_eq!(
            images.get(handle).unwrap().data[..4],
            [0, 0, 255, 255],
            "the old image stays"
        );

        // The frame loop waits for a file to stop changing before reading it.
        files.hot_reload = true;
        files.check_interval = Duration::ZERO;
        write(&path, [0, 255, 0, 255]);
        files.check(&mut images);
        assert_eq!(
            images.get(handle).unwrap().data[..4],
            [0, 0, 255, 255],
            "seen, not yet read"
        );
        files.check(&mut images);
        assert_eq!(images.get(handle).unwrap().data[..4], [0, 255, 0, 255]);

        assert!(files
            .load(&mut images, dir.join("missing.png"), true)
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
