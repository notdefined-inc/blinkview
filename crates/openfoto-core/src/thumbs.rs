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

/// Grab a frame from a video via ffmpeg, if it is installed.
///
/// Optional by design: ffmpeg is an external binary, so a missing one degrades to a
/// video with no poster frame rather than failing the whole thumbnail pass.
fn render_video(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let out = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-ss", "00:00:01", "-i"])
        .arg(src)
        .args(["-frames:v", "1", "-vf", &format!("scale='min({THUMB_LONG},iw)':-2")])
        .arg(dst)
        .output()
        .context("running ffmpeg")?;
    if !out.status.success() || !dst.exists() {
        anyhow::bail!("ffmpeg could not read {}", src.display());
    }
    Ok(())
}

pub fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build any missing thumbnails, for photos and videos alike. Returns how many were made.
pub fn build(lib: &Library) -> Result<usize> {
    let rows = lib.index.all()?;
    let ffmpeg = have_ffmpeg();
    let todo: Vec<(bool, std::path::PathBuf, std::path::PathBuf)> = rows
        .iter()
        .filter(|r| r.kind == "photo" || (r.kind == "video" && ffmpeg))
        .map(|r| (r.kind == "video", lib.abs(&r.path), thumb_path(lib, &r.hash)))
        .filter(|(_, _, dst)| !dst.exists())
        .collect();

    let made: usize = todo
        .par_iter()
        .map(|(is_video, src, dst)| {
            let ok = if *is_video { render_video(src, dst) } else { render_one(src, dst) };
            usize::from(ok.is_ok())
        })
        .sum();
    Ok(made)
}
