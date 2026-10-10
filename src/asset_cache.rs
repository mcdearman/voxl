//! What assets are made into before they are used, kept on disk so the making is done once.
//!
//! A picture file is decoded, filtered down into its smaller levels and compressed into the
//! blocks a graphics card reads directly. That takes far longer than reading the result, so
//! the result is written to a folder beside the project (`.mira/cache`) under a name that
//! is the hash of the file's bytes and of how it was processed. A file that changes has
//! another hash and is processed again; one that hasn't is read straight from the cache.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::render::Processed;

/// Changes when the processing does, so what an older build made is not mistaken for new.
const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"MIRAIMG1";

/// A name for these bytes processed this way: 128 bits of FNV-1a, as hex.
pub fn key(bytes: &[u8], how: &str) -> String {
    let hash = |seed: u64| {
        let mut hash = seed;
        for byte in how.bytes().chain(VERSION.to_le_bytes()).chain(bytes.iter().copied()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    };
    format!("{:016x}{:016x}", hash(0xcbf2_9ce4_8422_2325), hash(0x8422_2325_cbf2_9ce4))
}

fn file(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.img"))
}

fn format_number(format: wgpu::TextureFormat) -> Option<u32> {
    match format {
        wgpu::TextureFormat::Bc7RgbaUnorm => Some(0),
        wgpu::TextureFormat::Bc7RgbaUnormSrgb => Some(1),
        _ => None,
    }
}

/// A processed picture as bytes: a header, then each level's size and blocks.
pub(crate) fn encode(processed: &Processed) -> Option<Vec<u8>> {
    let mut out = MAGIC.to_vec();
    let numbers = [
        format_number(processed.format)?,
        processed.width,
        processed.height,
        processed.levels.len() as u32,
    ];
    for number in numbers {
        out.extend(number.to_le_bytes());
    }
    for value in processed.average {
        out.extend(value.to_le_bytes());
    }
    for (width, height, blocks) in &processed.levels {
        for number in [*width, *height, blocks.len() as u32] {
            out.extend(number.to_le_bytes());
        }
        out.extend(blocks);
    }
    Some(out)
}

/// The other way; none for bytes that are not a whole processed picture.
pub(crate) fn decode(bytes: &[u8]) -> Option<Processed> {
    let mut rest = bytes.strip_prefix(MAGIC)?;
    fn take<'a>(rest: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
        let (taken, after) = rest.split_at_checked(count)?;
        *rest = after;
        Some(taken)
    }
    fn number(rest: &mut &[u8]) -> Option<u32> {
        Some(u32::from_le_bytes(take(rest, 4)?.try_into().ok()?))
    }
    let format = match number(&mut rest)? {
        0 => wgpu::TextureFormat::Bc7RgbaUnorm,
        1 => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        _ => return None,
    };
    let (width, height, count) = (number(&mut rest)?, number(&mut rest)?, number(&mut rest)?);
    let mut average = [0.0; 3];
    for value in &mut average {
        *value = f32::from_le_bytes(take(&mut rest, 4)?.try_into().ok()?);
    }
    let mut levels = Vec::new();
    for _ in 0..count.min(32) {
        let (w, h, len) = (number(&mut rest)?, number(&mut rest)?, number(&mut rest)?);
        levels.push((w, h, take(&mut rest, len as usize)?.to_vec()));
    }
    (levels.len() == count as usize).then_some(Processed { format, width, height, average, levels })
}

/// What the cache holds under `key`, if anything whole.
pub(crate) fn read(dir: &Path, key: &str) -> Option<Processed> {
    decode(&std::fs::read(file(dir, key)).ok()?)
}

/// Keeps a processed picture under `key`. Written beside and renamed into place, so a
/// reader never finds half a file. Failing to write is no harm: it is made again next time.
pub(crate) fn write(dir: &Path, key: &str, processed: &Processed) {
    let Some(bytes) = encode(processed) else {
        return;
    };
    let path = file(dir, key);
    let partial = path.with_extension(format!("{}.part", std::process::id()));
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(err) = written {
        log::warn!("can't keep {} in the asset cache: {err}", path.display());
        let _ = std::fs::remove_file(partial);
    }
}

/// Removes the files least lately used until the cache is no bigger than `limit` bytes.
/// Returns how many went.
pub fn prune(dir: &Path, limit: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            let used = meta.accessed().or_else(|_| meta.modified()).ok()?;
            meta.is_file().then(|| (used, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    // The longest unused first.
    files.sort();
    let mut removed = 0;
    for (_, size, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Processed {
        Processed {
            format: wgpu::TextureFormat::Bc7RgbaUnormSrgb,
            width: 8,
            height: 4,
            average: [0.25, 0.5, 0.75],
            levels: vec![(8, 4, vec![7; 32]), (4, 4, vec![9; 16])],
        }
    }

    #[test]
    fn a_name_depends_on_the_bytes_and_on_how_they_were_processed() {
        let name = key(b"bricks", "bc7-srgb");
        assert_eq!(name.len(), 32);
        assert_eq!(name, key(b"bricks", "bc7-srgb"));
        assert_ne!(name, key(b"brick5", "bc7-srgb"));
        assert_ne!(name, key(b"bricks", "bc7-linear"));
    }

    #[test]
    fn a_processed_picture_comes_back_from_the_cache_as_it_went_in() {
        let dir = std::env::temp_dir().join(format!("mira-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read(&dir, "nothing").is_none());
        write(&dir, "wall", &picture());
        assert_eq!(read(&dir, "wall"), Some(picture()));
        // Half a file, or something else's file, is not a picture.
        let whole = encode(&picture()).unwrap();
        assert!(decode(&whole[..whole.len() - 3]).is_none());
        assert!(decode(b"something else entirely").is_none());

        // Over its limit, the cache gives up what was used longest ago.
        write(&dir, "floor", &picture());
        let size = std::fs::metadata(file(&dir, "wall")).unwrap().len();
        assert_eq!(prune(&dir, size * 2), 0);
        assert_eq!(prune(&dir, size), 1);
        assert_eq!(prune(&dir, 0), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
