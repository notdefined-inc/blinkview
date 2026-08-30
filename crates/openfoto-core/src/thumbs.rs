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

/// The environment variable naming an exact ffmpeg to use.
///
/// The desktop app sets this to its bundled sidecar at startup (ADR-0014). It wins over
/// everything else so that a packaged build never depends on what the host happens to
/// have installed, or on which of two ffmpegs `PATH` reaches first.
pub const FFMPEG_ENV: &str = "OPENFOTO_FFMPEG";

/// Where to look for ffmpeg, in order.
///
/// Taking the override as an argument rather than reading the environment keeps this
/// orderable in a test without mutating process-global state, which parallel tests
/// cannot do safely.
fn candidates(explicit: Option<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(FFMPEG_FALLBACKS.len() + 2);
    out.extend(explicit);
    out.push(std::ffi::OsString::from("ffmpeg"));
    out.extend(FFMPEG_FALLBACKS.iter().map(std::ffi::OsString::from));
    out
}

/// The first candidate that answers `-version` successfully.
///
/// Running it is the test, not its presence on disk: a path can exist and be a broken
/// symlink, the wrong architecture, or not ffmpeg at all.
fn first_runnable(candidates: Vec<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    candidates.into_iter().find(|cmd| {
        // A bare name is resolved through PATH by the OS; an absolute path that is not
        // there would spawn and fail, so skip it rather than pay for the attempt.
        if std::path::Path::new(cmd).is_absolute() && !std::path::Path::new(cmd).exists() {
            return false;
        }
        std::process::Command::new(cmd)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// The ffmpeg to run: the bundled sidecar if the app named one, else whatever PATH
/// offers, else the first well-known install path that works.
fn ffmpeg_bin() -> Option<std::ffi::OsString> {
    first_runnable(candidates(std::env::var_os(FFMPEG_ENV)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    /// A script that answers `-version` like ffmpeg does, and reports which one it is.
    fn fake_ffmpeg(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\necho \"$0\"\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("openfoto-ffmpeg-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The sidecar the app names must beat anything else, including a working `PATH`
    /// ffmpeg — a packaged build must not depend on what the host has installed.
    #[test]
    fn an_explicit_ffmpeg_wins_over_path() {
        let d = tmpdir("explicit");
        let bundled = fake_ffmpeg(&d, "bundled");
        let order = candidates(Some(OsString::from(&bundled)));
        assert_eq!(order.first(), Some(&OsString::from(&bundled)));
        assert_eq!(order.get(1), Some(&OsString::from("ffmpeg")));
        assert_eq!(first_runnable(order), Some(OsString::from(&bundled)));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// With nothing named, `PATH` is tried before the hard-coded install prefixes.
    #[test]
    fn without_an_override_path_is_tried_first() {
        let order = candidates(None);
        assert_eq!(order.first(), Some(&OsString::from("ffmpeg")));
        assert_eq!(order.len(), 1 + FFMPEG_FALLBACKS.len());
    }

    /// Presence is not enough: a candidate that cannot run is skipped, and when none
    /// can run the answer is None rather than a path that will fail later.
    #[test]
    fn a_candidate_that_cannot_run_is_not_chosen() {
        let d = tmpdir("broken");
        let broken = d.join("not-executable");
        std::fs::write(&broken, "this is not a program").unwrap();
        let missing = d.join("does-not-exist");
        assert!(broken.exists());
        assert_eq!(
            first_runnable(vec![OsString::from(&missing), OsString::from(&broken)]),
            None
        );
        let working = fake_ffmpeg(&d, "works");
        assert_eq!(
            first_runnable(vec![OsString::from(&broken), OsString::from(&working)]),
            Some(OsString::from(&working)),
            "a broken candidate must not stop the search"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
