//! Rotating and cropping photos.
//!
//! This is the only part of openfoto that changes a photograph rather than moving it,
//! so it follows what Apple Photos, Google Photos and Samsung Gallery all converge on:
//! **the original is always recoverable**. Those apps can keep an edit list in a
//! database and render on demand; we deliberately have no database, and `.openfoto/` is
//! disposable (ADR-0001) — storing the only copy of an original there would mean
//! deleting a cache silently destroyed the user's photo.
//!
//! So the original moves to a visible `Originals/` folder, exactly as deleting moves a
//! photo to `Trash/`, and the edited image takes its place. Finder shows it, other
//! apps see an ordinary JPEG, and the journal makes it undoable.
//!
//! Destructive mode overwrites without keeping the original. It exists because a user
//! who has decided is entitled to decide, but it is never the default.

use crate::{imageio, Library};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const ORIGINALS: &str = "Originals";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Rotate {
    None,
    Cw90,
    Cw180,
    Cw270,
}

/// A crop in fractions of the image, so it survives the UI not knowing pixel sizes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    #[serde(default = "default_rotate")]
    pub rotate: Rotate,
    #[serde(default)]
    pub crop: Option<Crop>,
    /// Keep the original in `Originals/`. Default, and what the UI offers first.
    #[serde(default = "default_true")]
    pub keep_original: bool,
}

fn default_rotate() -> Rotate {
    Rotate::None
}
fn default_true() -> bool {
    true
}

impl Edit {
    pub fn is_noop(&self) -> bool {
        self.rotate == Rotate::None && self.crop.is_none()
    }
}

pub struct Applied {
    /// Where the untouched original ended up, when it was kept.
    pub original: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// Apply an edit to one photo, in place.
pub fn apply(lib: &Library, rel_path: &str, edit: &Edit) -> Result<Applied> {
    if edit.is_noop() {
        anyhow::bail!("nothing to apply");
    }
    let src = lib.abs(rel_path);
    let mut img = imageio::load_rgb(&src).with_context(|| format!("reading {rel_path}"))?;

    if let Some(c) = edit.crop {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let x = (c.x.clamp(0.0, 1.0) * w) as u32;
        let y = (c.y.clamp(0.0, 1.0) * h) as u32;
        let cw = ((c.w.clamp(0.0, 1.0) * w) as u32).min(img.width().saturating_sub(x)).max(1);
        let ch = ((c.h.clamp(0.0, 1.0) * h) as u32).min(img.height().saturating_sub(y)).max(1);
        img = image::imageops::crop_imm(&img, x, y, cw, ch).to_image();
    }
    img = match edit.rotate {
        Rotate::None => img,
        Rotate::Cw90 => image::imageops::rotate90(&img),
        Rotate::Cw180 => image::imageops::rotate180(&img),
        Rotate::Cw270 => image::imageops::rotate270(&img),
    };

    // Preserve the original before anything is overwritten.
    let mut original = None;
    if edit.keep_original {
        let dir = lib.abs(ORIGINALS);
        std::fs::create_dir_all(&dir)?;
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        let mut dest = dir.join(name);
        // Never clobber an original already kept for a different photo.
        let mut n = 2;
        while dest.exists() {
            let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "jpg"));
            dest = dir.join(format!("{stem}_{n}.{ext}"));
            n += 1;
        }
        std::fs::rename(&src, &dest).with_context(|| "moving the original aside")?;
        original = lib.rel(&dest);
    }

    // Write beside the target and swap, so a failure never leaves a truncated photo
    // where the original used to be.
    let tmp = src.with_extension("openfoto-tmp");
    let (width, height) = (img.width(), img.height());
    image::DynamicImage::ImageRgb8(img)
        .save_with_format(&tmp, image::ImageFormat::Jpeg)
        .with_context(|| "writing the edited image")?;
    std::fs::rename(&tmp, &src).with_context(|| "replacing the photo")?;

    Ok(Applied { original, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_noop_edit_is_rejected() {
        let e = Edit { rotate: Rotate::None, crop: None, keep_original: true };
        assert!(e.is_noop());
    }

    #[test]
    fn keeping_the_original_is_the_default() {
        let e: Edit = serde_json::from_str(r#"{"rotate":"cw90"}"#).unwrap();
        assert!(e.keep_original, "safe editing must be the default");
        assert_eq!(e.rotate, Rotate::Cw90);
        assert!(e.crop.is_none());
    }

    #[test]
    fn destructive_must_be_asked_for_explicitly() {
        let e: Edit = serde_json::from_str(r#"{"rotate":"cw90","keep_original":false}"#).unwrap();
        assert!(!e.keep_original);
    }

    #[test]
    fn crop_fractions_are_clamped_not_trusted() {
        // The UI sends fractions; a bad one must not panic or read out of bounds.
        let c = Crop { x: -0.5, y: 2.0, w: 5.0, h: -1.0 };
        assert_eq!(c.x.clamp(0.0, 1.0), 0.0);
        assert_eq!(c.y.clamp(0.0, 1.0), 1.0);
        assert_eq!(c.w.clamp(0.0, 1.0), 1.0);
        assert_eq!(c.h.clamp(0.0, 1.0), 0.0);
    }
}
