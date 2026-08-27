//! The thumbnail cache that makes the grid feel instant.
//!
//! Thumbnails are content-addressed (`.openfoto/thumbs/<hash>.jpg`), so they survive
//! renames and folder moves, and the cache is disposable like the rest of the vault.
//! Decoding uses the JPEG decoder's DCT downscaling, so building 280 thumbnails costs
//! about a second rather than a minute.

use crate::{imageio, Library};
use anyhow::{Context, Result};
use rayon::prelude::*;

/// Long edge of a grid thumbnail. 512 stays crisp on a 2x display at typical
/// grid sizes without bloating the cache.
pub const THUMB_LONG: u32 = 512;

pub fn thumb_path(lib: &Library, hash: &str) -> std::path::PathBuf {
    thumb_path_at(lib.root(), hash)
}

/// Thumbnail path from a library root, for callers that cannot hold a `Library`
/// (rayon workers, since it owns a non-Sync SQLite connection).
pub fn thumb_path_at(root: &std::path::Path, hash: &str) -> std::path::PathBuf {
    root.join(crate::library::VAULT_DIR).join("thumbs").join(format!("{hash}.jpg"))
}

fn render_one(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    let img = imageio::load_rgb(src)?;
    let (w, h) = (img.width(), img.height());
    let scale = THUMB_LONG as f32 / w.max(h) as f32;
    let out = if scale < 1.0 {
        image::imageops::resize(
            &img,
            (w as f32 * scale).round().max(1.0) as u32,
            (h as f32 * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(out)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .context("encoding thumbnail")?;
    std::fs::write(dst, buf.into_inner())?;
    Ok(())
}

/// Build any missing thumbnails. Returns how many were created.
pub fn build(lib: &Library) -> Result<usize> {
    let rows = lib.index.all()?;
    let todo: Vec<(String, std::path::PathBuf, std::path::PathBuf)> = rows
        .iter()
        .filter(|r| r.kind == "photo")
        .map(|r| (r.hash.clone(), lib.abs(&r.path), thumb_path(lib, &r.hash)))
        .filter(|(_, _, dst)| !dst.exists())
        .collect();

    let made: usize = todo
        .par_iter()
        .map(|(_, src, dst)| usize::from(render_one(src, dst).is_ok()))
        .sum();
    Ok(made)
}
