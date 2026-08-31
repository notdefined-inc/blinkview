//! The thumbnail cache that makes the grid feel instant.
//!
//! Thumbnails are content-addressed (`.blinkview/thumbs/<hash>.jpg`), so they survive
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
    lib.vault().join("thumbs").join(format!("{hash}.jpg"))
}

/// Thumbnail path from a library root, for callers that hold no `Library` — the
/// `photo://` handler, which has only the path it is serving.
///
/// Resolves the library's cache, which is not under the root any more (ADR-0019);
/// `vault_for` memoizes, so asking per thumbnail is one map lookup after the first.
pub fn thumb_path_at(root: &std::path::Path, hash: &str) -> std::path::PathBuf {
    thumb_path_in(&crate::cache::vault_for(root), hash)
}

/// Thumbnail path from a cache directory. For code that already knows where the
/// library's cache is — everything holding a `Library`, or a rayon worker carrying
/// its vault because the `Library` itself is not `Sync`.
pub fn thumb_path_in(vault: &std::path::Path, hash: &str) -> std::path::PathBuf {
    vault.join("thumbs").join(format!("{hash}.jpg"))
}

/// Render a single thumbnail. Public so the desktop app can produce one on demand
/// when the grid asks for it, rather than requiring a full pre-pass first.
pub fn render_to(src: &std::path::Path, dst: &std::path::Path, is_video: bool) -> Result<()> {
    if is_video {
        render_video(src, dst)
    } else {
        render_one(src, dst)
    }
}

/// Write a thumbnail from pixels already decoded, applying the rotation still owed.
///
/// The shared-decode entry point (ADR-0013): the analysis pass has the frame in hand
/// and must not open the file again to get it.
pub fn render_from_rgb(
    img: &image::RgbImage,
    orientation: u16,
    dst: &std::path::Path,
) -> Result<()> {
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
    let (img, o) = match imageio::camera_preview(src, THUMB_LONG) {
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

/// Long edge of the lightbox preview.
///
/// The stepper was slow because every step loaded the full original — a 12–48 MP
/// decode per keypress. Two thousand pixels fills a Retina window at full-screen
/// size, and at ~400 KB it is the difference between a step and a wait.
pub const PREVIEW_LONG: u32 = 2000;

/// Where a photograph's preview lives, from the library root.
pub fn preview_path_at(root: &std::path::Path, hash: &str) -> std::path::PathBuf {
    vault_preview_in(&crate::cache::vault_for(root), hash)
}

/// As [`preview_path_at`], from a cache directory already in hand.
pub fn preview_path_in(vault: &std::path::Path, hash: &str) -> std::path::PathBuf {
    vault_preview_in(vault, hash)
}

fn vault_preview_in(vault: &std::path::Path, hash: &str) -> std::path::PathBuf {
    vault.join("derived").join(format!("p-{hash}.jpg"))
}

/// Render the lightbox preview: a [`PREVIEW_LONG`] JPEG derived once, on first view.
///
/// Returns `false` when no derived file was written because the source is already at
/// or below [`PREVIEW_LONG`] *and the webview can decode it as-is* — the original is
/// then the same view for none of the cost. A format the webview cannot decode (HEIC,
/// camera RAW) is never its own preview at any size: it always converts. An embedded
/// camera preview of 2,000 px or more is used in preference to a full decode, exactly
/// as thumbnails do; a RAW whose embedded preview is smaller (ARW 1616px, RAF 1920px)
/// goes through `sips` instead, which renders the sensor at full resolution rather
/// than stretching the small preview.
pub fn render_preview(src: &std::path::Path, dst: &std::path::Path) -> Result<bool> {
    let (img, o, full_decode) = match imageio::camera_preview(src, PREVIEW_LONG) {
        Some(preview) => (preview, imageio::orientation(src), false),
        // A RAW whose embedded preview is under PREVIEW_LONG is better served by a
        // full conversion where one exists; elsewhere the small preview stands.
        None if crate::raw::is_raw(src) => match imageio::load_rgb_converted(src) {
            Ok(img) => (img, 1, true),
            Err(_) => (imageio::load_rgb(src)?, 1, true),
        },
        None if imageio::needs_conversion(src) => (imageio::load_rgb(src)?, 1, true),
        None => (
            imageio::load_rgb_unrotated(src)?,
            imageio::orientation(src),
            true,
        ),
    };
    let (w, h) = (img.width(), img.height());
    // Only a full decode knows the source is small; an embedded preview said to be
    // at least PREVIEW_LONG may still stand in for a much larger original. A source
    // the webview cannot decode is never its own preview, whatever its size.
    if full_decode && w.max(h) <= PREVIEW_LONG && o == 1 && !imageio::needs_conversion(src) {
        return Ok(false);
    }
    let scale = PREVIEW_LONG as f32 / w.max(h) as f32;
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
    let out = imageio::apply_rgb(shrunk, o);
    write_jpeg(&out, dst)?;
    Ok(true)
}

/// Where ffmpeg is, when the environment will not say.
///
/// An app launched from Finder does not inherit a shell's PATH — launchd hands it
/// `/usr/bin:/bin:/usr/sbin:/sbin` — so `ffmpeg` resolves in a terminal and not in the
/// installed .app, which is where the packaged build silently produced no video
/// thumbnails at all. These are the usual install prefixes for Homebrew on Apple
/// silicon, Homebrew on Intel, and MacPorts or hand-built copies.
const FFMPEG_FALLBACKS: &[&str] = &[
    "/opt/homebrew/bin/ffmpeg",
    "/usr/local/bin/ffmpeg",
    "/opt/local/bin/ffmpeg",
];

/// The environment variable naming an exact ffmpeg to use.
///
/// The desktop app sets this to its bundled sidecar at startup (ADR-0014). It wins over
/// everything else so that a packaged build never depends on what the host happens to
/// have installed, or on which of two ffmpegs `PATH` reaches first.
pub const FFMPEG_ENV: &str = "BLINKVIEW_FFMPEG";

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
    let Some(bin) = ffmpeg_bin() else {
        anyhow::bail!("ffmpeg not found");
    };
    render_video_with(&bin, src, dst)
}

/// [`render_video`] against a named binary.
///
/// Resolving ffmpeg costs a process spawn (`-version`), so a caller building many
/// posters resolves once and names the binary here. It is also the seam that lets a
/// test supply a fake ffmpeg without mutating process-global state.
pub fn render_video_with(
    bin: &std::ffi::OsStr,
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let out = std::process::Command::new(bin)
        .args(["-loglevel", "error", "-y", "-ss", "00:00:01", "-i"])
        .arg(src)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale='min({THUMB_LONG},iw)':-2"),
        ])
        .arg(dst)
        .output()
        .context("running ffmpeg")?;
    if !out.status.success() || !dst.exists() {
        anyhow::bail!("ffmpeg could not read {}", src.display());
    }
    Ok(())
}

/// The ffmpeg this process will use, if any.
///
/// Public because a multi-video pass wants to resolve once and hand the answer to
/// [`render_video_with`], rather than pay a `-version` spawn per clip.
pub fn resolve() -> Option<std::ffi::OsString> {
    ffmpeg_bin()
}

pub fn have_ffmpeg() -> bool {
    resolve().is_some()
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
    let ffmpeg = resolve();
    let todo: Vec<(bool, std::path::PathBuf, std::path::PathBuf)> = rows
        .iter()
        .filter(|r| r.kind == "photo" || (r.kind == "video" && ffmpeg.is_some()))
        .map(|r| {
            (
                r.kind == "video",
                lib.abs(&r.path),
                thumb_path(lib, &r.hash),
            )
        })
        .filter(|(_, _, dst)| !dst.exists())
        .collect();

    let counter = crate::progress::Counter::new(todo.len(), progress);
    let results: Vec<Result<()>> = todo
        .par_iter()
        .map(|(is_video, src, dst)| {
            let ok = match (ffmpeg.as_deref(), is_video) {
                // Resolved once for the whole pass, not once per clip.
                (Some(bin), true) => render_video_with(bin, src, dst),
                (None, true) => Err(anyhow::anyhow!("ffmpeg not found")), // filtered out above
                (_, false) => render_one(src, dst),
            };
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
        let d = std::env::temp_dir().join(format!("blinkview-ffmpeg-{}-{tag}", std::process::id()));
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

    /// A photograph larger than the preview long edge gets a derived JPEG of exactly
    /// that edge; one already small enough is served as itself and no file is written.
    #[test]
    fn previews_are_made_only_for_what_needs_one() {
        let d = tmpdir("preview");
        let big = d.join("big.jpg");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            3000,
            2000,
            image::Rgb([120, 60, 30]),
        ))
        .save(&big)
        .unwrap();
        let dst = d.join("p-big.jpg");
        assert!(
            render_preview(&big, &dst).unwrap(),
            "a large source makes a preview"
        );
        let (w, h) = image::image_dimensions(&dst).unwrap();
        assert_eq!(w.max(h), PREVIEW_LONG);

        let small = d.join("small.jpg");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            800,
            600,
            image::Rgb([1, 2, 3]),
        ))
        .save(&small)
        .unwrap();
        let dst2 = d.join("p-small.jpg");
        assert!(
            !render_preview(&small, &dst2).unwrap(),
            "a small source needs no derived file"
        );
        assert!(!dst2.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ARW and RAF showed a broken image in the lightbox: their embedded previews
    /// (1616px, 1920px) are under `PREVIEW_LONG`, so the "already small enough" shortcut
    /// fired and served the raw bytes straight to a webview that cannot decode them. The
    /// shortcut must never fire for a format `needs_conversion`, whatever its size.
    ///
    /// Real RAW sensor data isn't available to a unit test, so this stands a plain JPEG
    /// under a `.arw` name: `raw::preview` won't recognise it as a RAW container (wrong
    /// header) and falls through to the same `sips`-conversion path a genuine small-preview
    /// RAW takes, exercising the exact branch and shortcut condition that was fixed.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_raw_extension_never_takes_the_small_source_shortcut() {
        let d = tmpdir("raw-shortcut");
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            800,
            600,
            image::Rgb([10, 20, 30]),
        ));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        let src = d.join("small.arw");
        std::fs::write(&src, &bytes).unwrap();

        let dst = d.join("p-small-arw.jpg");
        assert!(
            render_preview(&src, &dst).unwrap(),
            "a RAW-named source must always get a derived, webview-decodable preview"
        );
        assert!(dst.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
