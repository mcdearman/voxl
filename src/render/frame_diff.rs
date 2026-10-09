//! Comparing a rendered frame with one stored earlier, for tests that check the picture.
//!
//! Two machines never draw the same scene to the same pixels: anti-aliasing, the order of
//! floating-point sums and the screen's size all differ. So frames are compared small
//! ([`shrink`]), a pixel counts as different only beyond a tolerance, and a frame passes
//! while few enough pixels do ([`Allowed`]). That still catches what such tests are for: a
//! missing shadow, a wrong colour, an object that has moved or gone.

use std::path::{Path, PathBuf};

use image::{imageops, Rgba, RgbaImage};

/// How two frames of the same size differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    /// The share of pixels that differ by more than the tolerance in some channel.
    pub differing: f32,
    /// The largest difference in any channel of any pixel, out of 255.
    pub worst: u8,
    /// The mean difference over every channel of every pixel, out of 255.
    pub mean: f32,
}

/// How much difference still passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Allowed {
    /// A channel may differ by this much, out of 255, before its pixel counts as different.
    pub tolerance: u8,
    /// The share of pixels that may be different.
    pub differing: f32,
}

impl Default for Allowed {
    fn default() -> Self {
        Self {
            tolerance: 24,
            differing: 0.01,
        }
    }
}

/// The width frames are compared at.
pub const COMPARED_WIDTH: u32 = 320;

/// A frame made small, so that what is compared is the picture and not its pixels.
pub fn shrink(frame: &RgbaImage) -> RgbaImage {
    let width = COMPARED_WIDTH.min(frame.width()).max(1);
    let height = (frame.height() as u64 * width as u64 / frame.width().max(1) as u64).max(1);
    imageops::resize(frame, width, height as u32, imageops::FilterType::Triangle)
}

fn channels(a: &Rgba<u8>, b: &Rgba<u8>) -> impl Iterator<Item = u8> {
    // What is behind the frame is not part of the picture: alpha is left out.
    (0..3).map({
        let (a, b) = (a.0, b.0);
        move |channel| a[channel].abs_diff(b[channel])
    })
}

/// How two frames differ, or why they can't be compared.
pub fn compare(a: &RgbaImage, b: &RgbaImage, tolerance: u8) -> Result<Difference, String> {
    if a.dimensions() != b.dimensions() {
        return Err(format!(
            "one frame is {}x{} and the other {}x{}",
            a.width(),
            a.height(),
            b.width(),
            b.height()
        ));
    }
    let (mut differing, mut worst, mut total) = (0u64, 0u8, 0u64);
    for (a, b) in a.pixels().zip(b.pixels()) {
        let most = channels(a, b).max().unwrap_or(0);
        differing += (most > tolerance) as u64;
        worst = worst.max(most);
        total += channels(a, b).map(u64::from).sum::<u64>();
    }
    let pixels = (a.width() as u64 * a.height() as u64).max(1);
    Ok(Difference {
        differing: differing as f32 / pixels as f32,
        worst,
        mean: total as f32 / (pixels * 3) as f32,
    })
}

/// The first frame dimmed, with every pixel that differs from the second marked red: where
/// to look.
pub fn picture(a: &RgbaImage, b: &RgbaImage, tolerance: u8) -> RgbaImage {
    let mut out = a.clone();
    for (out, b) in out.pixels_mut().zip(b.pixels()) {
        let differs = channels(out, b).any(|by| by > tolerance);
        *out = if differs {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([out.0[0] / 3, out.0[1] / 3, out.0[2] / 3, 255])
        };
    }
    out
}

fn beside(stored: &Path, what: &str) -> PathBuf {
    let stem = stored.file_stem().unwrap_or_default().to_string_lossy();
    stored.with_file_name(format!("{stem}.{what}.png"))
}

/// Checks a rendered frame against the one stored at `stored`.
///
/// With `MIRA_UPDATE_FRAMES=1`, or when nothing is stored yet, the frame is stored (shrunk)
/// and the check passes: that is how a stored frame is made, and remade after a change that
/// was meant. Otherwise a frame that differs by more than `allowed` fails, leaving
/// `<name>.new.png` (what was drawn) and `<name>.diff.png` (where it differs) beside the
/// stored frame.
pub fn check_frame(frame: &RgbaImage, stored: &Path, allowed: Allowed) -> Result<(), String> {
    let frame = shrink(frame);
    let update = std::env::var("MIRA_UPDATE_FRAMES").is_ok_and(|update| update != "0");
    if update || !stored.exists() {
        if let Some(folder) = stored.parent() {
            std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
        }
        return frame
            .save(stored)
            .map_err(|err| format!("can't store {}: {err}", stored.display()));
    }
    let kept = image::open(stored)
        .map_err(|err| format!("can't read {}: {err}", stored.display()))?
        .to_rgba8();
    let difference = compare(&frame, &kept, allowed.tolerance)?;
    let (new, diff) = (beside(stored, "new"), beside(stored, "diff"));
    if difference.differing <= allowed.differing {
        // Left over from a failure since put right.
        let _ = std::fs::remove_file(new);
        let _ = std::fs::remove_file(diff);
        return Ok(());
    }
    let _ = frame.save(&new);
    let _ = picture(&frame, &kept, allowed.tolerance).save(&diff);
    Err(format!(
        "the frame differs from {}: {:.1}% of pixels are off by more than {} (at most {:.1}% may be), the worst by {}; see {} and {}",
        stored.display(),
        difference.differing * 100.0,
        allowed.tolerance,
        allowed.differing * 100.0,
        difference.worst,
        new.display(),
        diff.display(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, colour: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba([colour[0], colour[1], colour[2], 255]))
    }

    #[test]
    fn frames_are_compared_by_how_much_of_the_picture_differs() {
        let grey = frame(100, 50, [100, 100, 100]);
        assert_eq!(
            compare(&grey, &grey, 0),
            Ok(Difference {
                differing: 0.0,
                worst: 0,
                mean: 0.0
            })
        );
        // A little off everywhere is noise; a patch that is far off is a difference.
        let noisy = frame(100, 50, [104, 97, 100]);
        let near = compare(&grey, &noisy, 8).unwrap();
        assert_eq!((near.differing, near.worst), (0.0, 4));
        let mut patched = grey.clone();
        for x in 0..10 {
            for y in 0..10 {
                patched.put_pixel(x, y, Rgba([250, 100, 100, 255]));
            }
        }
        let far = compare(&grey, &patched, 8).unwrap();
        assert_eq!((far.differing, far.worst), (0.02, 150));
        assert_eq!(far.mean, 1.0);
        // Alpha is not part of the picture.
        let mut clear = grey.clone();
        clear.pixels_mut().for_each(|pixel| pixel.0[3] = 0);
        assert_eq!(compare(&grey, &clear, 0).unwrap().worst, 0);
        // Frames of different sizes are not compared at all.
        assert!(compare(&grey, &frame(50, 50, [0; 3]), 0).is_err());

        let marked = picture(&grey, &patched, 8);
        assert_eq!(marked.get_pixel(3, 3), &Rgba([255, 0, 0, 255]));
        assert_eq!(marked.get_pixel(50, 30), &Rgba([33, 33, 33, 255]));
    }

    #[test]
    fn a_frame_is_shrunk_to_one_width_whatever_the_screen() {
        let small = shrink(&frame(2560, 1440, [9, 9, 9]));
        assert_eq!(small.dimensions(), (320, 180));
        assert_eq!(shrink(&frame(1280, 720, [9, 9, 9])).dimensions(), (320, 180));
        assert_eq!(shrink(&frame(64, 64, [9, 9, 9])).dimensions(), (64, 64));
        assert_eq!(small.get_pixel(100, 100), &Rgba([9, 9, 9, 255]));
    }

    #[test]
    fn a_frame_is_checked_against_the_one_stored() {
        if std::env::var("MIRA_UPDATE_FRAMES").is_ok() {
            // Everything passes while frames are being remade; nothing to learn here.
            return;
        }
        let folder = std::env::temp_dir().join(format!("mira-frames-{}", std::process::id()));
        let stored = folder.join("scene.png");
        let sky = frame(640, 360, [90, 140, 220]);
        // Nothing stored: this frame becomes the stored one.
        assert_eq!(check_frame(&sky, &stored, Allowed::default()), Ok(()));
        assert_eq!(image::open(&stored).unwrap().width(), 320);
        // The same picture from a bigger screen, a touch darker, still passes.
        let bigger = frame(1280, 720, [86, 136, 216]);
        assert_eq!(check_frame(&bigger, &stored, Allowed::default()), Ok(()));
        // A quarter of it gone black does not, and says where to look.
        let mut broken = sky.clone();
        for x in 0..320 {
            for y in 0..180 {
                broken.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let why = check_frame(&broken, &stored, Allowed::default()).unwrap_err();
        // A quarter, and the blurred edge of it.
        assert!(why.contains(": 25."), "{why}");
        assert!(folder.join("scene.new.png").exists() && folder.join("scene.diff.png").exists());
        // Put right, the leftovers go.
        assert_eq!(check_frame(&sky, &stored, Allowed::default()), Ok(()));
        assert!(!folder.join("scene.diff.png").exists());
        let _ = std::fs::remove_dir_all(folder);
    }
}
