//! The thumbnail cache that makes the grid feel instant.
//!
//! Thumbnails are content-addressed (`.openfoto/thumbs/<hash>.jpg`), so they survive
//! renames and folder moves, and the cache is disposable like the rest of the vault.
//! The cost of a thumbnail is the decode, not the resize or the encode, and the decode
//! is avoided entirely when the camera already wrote a preview into the file — see
//! `imageio::embedded_preview`. Failing that, the image is decoded once, shrunk, and
//! only then rotated, because rotating twelve megapixels to throw away all but a
//! 512-pixel edge costs 14 ms for nothing.

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

/// Render a single thumbnail. Public so the desktop app can produce one on demand
/// when the grid asks for it, rather than requiring a full pre-pass first.
pub fn render_to(src: &std::path::Path, dst: &std::path::Path, is_video: bool) -> Result<()> {
    if is_video { render_video(src, dst) } else { render_one(src, dst) }
}

/// Write a thumbnail from pixels already decoded, applying the rotation still owed.
///
/// The shared-decode entry point (ADR-0013): the analysis pass has the frame in hand
/// and must not open the file again to get it.
pub fn render_from_rgb(img: &image::RgbImage, orientation: u16, dst: &std::path::Path) -> Result<()> {
    let (w, h) = (img.width(), img.height());
    let scale = THUMB_LONG as f32 / w.max(h) as f32;
    let shrunk = if scale < 1.0 {
        image::imageops::resize(
            img,
            (w as f32 * scale).round().max(1.0) as u32,
            (h as f32 * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img.clone()
    };
    write_jpeg(&imageio::apply_rgb(shrunk, orientation), dst)
}

fn write_jpeg(img: &image::RgbImage, dst: &std::path::Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .context("encoding thumbnail")?;
    std::fs::write(dst, buf.into_inner())?;
    Ok(())
}

fn render_one(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    // The camera's own preview first: reading 37 KB and decoding a 512px image instead
    // of twelve megapixels is the difference between minutes and seconds over a phone
    // backup. `embedded_preview` refuses anything that is too small or whose shape
    // disagrees with the photograph, so this is a fast path, never a guess.
    // `o` is the rotation still owed. The HEIC route rotates inside `load_rgb`, so it
    // owes nothing further — applying it twice would turn a portrait upside down.
    let (img, o) = match std::fs::read(src)
        .ok()
        .and_then(|b| imageio::embedded_preview(&b, THUMB_LONG))
    {
        Some(preview) => (preview, imageio::orientation(src)),
        None if imageio::needs_conversion(src) => (imageio::load_rgb(src)?, 1),
        None => (imageio::load_rgb_unrotated(src)?, imageio::orientation(src)),
    };

    let (w, h) = (img.width(), img.height());
    let scale = THUMB_LONG as f32 / w.max(h) as f32;
    let shrunk = if scale < 1.0 {
        image::imageops::resize(
            &img,
            (w as f32 * scale).round().max(1.0) as u32,
            (h as f32 * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    // Rotate last, on the small image.
    let out = imageio::apply_rgb(shrunk, o);
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

/// Where ffmpeg is, when the environment will not say.
///
/// An app launched from Finder does not inherit a shell's PATH — launchd hands it
/// `/usr/bin:/bin:/usr/sbin:/sbin` — so `ffmpeg` resolves in a terminal and not in the
/// installed .app, which is where the packaged build silently produced no video
/// thumbnails at all. These are the usual install prefixes for Homebrew on Apple
/// silicon, Homebrew on Intel, and MacPorts or hand-built copies.
const FFMPEG_FALLBACKS: &[&str] =
    &["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/opt/local/bin/ffmpeg"];

/// The ffmpeg to run: whatever PATH offers, else the first well-known path present.
fn ffmpeg_bin() -> Option<std::ffi::OsString> {
    let runs = |cmd: &std::ffi::OsStr| {
        std::process::Command::new(cmd)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let bare = std::ffi::OsString::from("ffmpeg");
    if runs(&bare) {
        return Some(bare);
    }
    FFMPEG_FALLBACKS
        .iter()
        .map(std::ffi::OsString::from)
        .find(|c| std::path::Path::new(c).exists() && runs(c))
}

/// Grab a frame from a video via ffmpeg, if it is installed.
///
/// Optional by design: ffmpeg is an external binary, so a missing one degrades to a
/// video with no poster frame rather than failing the whole thumbnail pass.
fn render_video(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let Some(bin) = ffmpeg_bin() else {
        anyhow::bail!("ffmpeg not found");
    };
    let out = std::process::Command::new(bin)
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
    ffmpeg_bin().is_some()
}

/// Build any missing thumbnails, for photos and videos alike. Returns how many were made.
pub fn build(lib: &Library) -> Result<usize> {
    build_with_progress(lib, &crate::progress::silent)
}

/// As [`build`], reporting (done, total) as it goes.
pub fn build_with_progress(
    lib: &Library,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<usize> {
    let rows = lib.index.all()?;
    let ffmpeg = have_ffmpeg();
    let todo: Vec<(bool, std::path::PathBuf, std::path::PathBuf)> = rows
        .iter()
        .filter(|r| r.kind == "photo" || (r.kind == "video" && ffmpeg))
        .map(|r| (r.kind == "video", lib.abs(&r.path), thumb_path(lib, &r.hash)))
        .filter(|(_, _, dst)| !dst.exists())
        .collect();

    let counter = crate::progress::Counter::new(todo.len(), progress);
    let results: Vec<Result<()>> = todo
        .par_iter()
        .map(|(is_video, src, dst)| {
            let ok = if *is_video { render_video(src, dst) } else { render_one(src, dst) };
            counter.tick();
            ok.with_context(|| format!("thumbnail for {}", src.display()))
        })
        .collect();
    counter.finish();

    let made = results.iter().filter(|r| r.is_ok()).count();
    // Failures used to be counted and discarded, which hid a hard stop partway
    // through a large library. Surface the first one; the caller decides.
    if let Some(Err(e)) = results.into_iter().find(|r| r.is_err()) {
        if made == 0 {
            return Err(e);
        }
        eprintln!("[thumbs] {made} built, first failure: {e:#}");
    }
    Ok(made)
}
