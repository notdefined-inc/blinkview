//! One pass over the pixels (ADR-0013).
//!
//! Thumbnails, faces and semantic embeddings all want the same photograph decoded, and
//! decoding is 85% of what each of them costs. This pass decodes once and takes all
//! three from that frame.
//!
//! Work is parallel across photographs and committed by one thread, because the index
//! owns a SQLite connection that cannot be shared. Workers do the expensive part —
//! decode and inference — and write the files they own (thumbnails, face crops); only
//! the rows go over the channel.

use crate::faces::{detect, embed, models, pipeline, store::StoredFace};
use crate::{imageio, semantic, thumbs, Library};
use anyhow::Result;
use rayon::prelude::*;

/// Which stages to run. A stage whose models are missing is skipped rather than fatal.
#[derive(Debug, Clone, Copy)]
pub struct Stages {
    pub thumbs: bool,
    pub faces: bool,
    pub semantic: bool,
}

impl Default for Stages {
    fn default() -> Self {
        Self { thumbs: true, faces: true, semantic: true }
    }
}

impl Stages {
    pub fn only_thumbs() -> Self {
        Self { thumbs: true, faces: false, semantic: false }
    }
    pub fn only_faces() -> Self {
        Self { thumbs: false, faces: true, semantic: false }
    }
    pub fn only_semantic() -> Self {
        Self { thumbs: false, faces: false, semantic: true }
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub considered: usize,
    /// Photographs opened and decoded in full. The measure this pass exists to lower:
    /// with three separate passes it was three times the number of photographs.
    pub decoded: usize,
    /// Thumbnails produced from the camera's embedded preview, with no full decode.
    pub from_preview: usize,
    pub thumbs: usize,
    pub faces: usize,
    pub too_small: usize,
    pub embedded: usize,
    pub skipped: usize,
    /// Photographs recorded as impossible to decode during this pass.
    pub unreadable: usize,
    pub errors: Vec<String>,
}

/// One photograph and what it still needs.
struct Job {
    hash: String,
    path: std::path::PathBuf,
    rel: String,
    thumb: bool,
    faces: bool,
    clip: bool,
}

/// What one worker produces for one photograph. Files are already written; these are
/// the rows the committing thread has to store.
struct Outcome {
    hash: String,
    faces: Option<Vec<StoredFace>>,
    clip: Option<Vec<f32>>,
    /// Set when the photograph could not be decoded at all, so it is recorded and not
    /// attempted again on every future pass.
    unreadable: Option<String>,
    stats: Stats,
}

/// The models one worker holds. Loaded lazily per thread, so a library that needs no
/// face work never pays for a detector.
#[derive(Default)]
struct Kit {
    det: Option<detect::Detector>,
    emb: Option<embed::Embedder>,
    vision: Option<semantic::ImageEncoder>,
}

/// Total physical memory in bytes, if the platform will say.
///
/// No dependency is added for this. macOS is asked through `sysctl`, which the project
/// already shells out to for HEIC (ADR-0005); Linux is a file read. Windows has no
/// equally cheap route, so it gets `None` and sizing falls back to cores alone.
fn physical_memory() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        return String::from_utf8(out.stdout).ok()?.trim().parse().ok();
    }
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = meminfo
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        return Some(kb * 1024);
    }
    #[allow(unreachable_code)]
    None
}

/// How many photographs are worked on at once.
///
/// Not one per core: ONNX Runtime already threads a single inference, so a worker per
/// core buys nothing, while each worker holds its own models and a decoded frame of up
/// to 36 MB.
///
/// Memory is the binding constraint, not cores, and until this was measured the ceiling
/// was chosen from core count alone — so an eight-core machine with 8 GB got four
/// workers and swapped. On 226 photographs across 76 resolutions, peak RSS rose
/// monotonically with workers (1299, 1407, 1520, 1599 MB) while throughput peaked at
/// two and got *worse* at four (50s, then 95s). Four workers was the most expensive
/// setting on both axes at once. One worker per 4 GB reproduces that: two on an 8 GB
/// machine, four on 16 GB and above.
///
/// `BLINKVIEW_WORKERS` overrides the result, for a machine that still wants less.
fn workers() -> usize {
    if let Ok(n) = std::env::var("BLINKVIEW_WORKERS").unwrap_or_default().parse::<usize>() {
        if n > 0 {
            return n;
        }
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let by_memory = physical_memory()
        .map(|bytes| (bytes / (4 * 1024 * 1024 * 1024)) as usize)
        .unwrap_or(usize::MAX);
    cores.min(by_memory).clamp(1, 4)
}

pub fn run(lib: &mut Library, stages: Stages) -> Result<Stats> {
    run_with_progress(lib, stages, &crate::progress::silent)
}

pub fn run_with_progress(
    lib: &mut Library,
    stages: Stages,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Stats> {
    run_cancellable(lib, stages, progress, &|| false)
}

/// As [`run_with_progress`], stopping early when `stop` says to.
///
/// Checked per photograph rather than per batch: removing a folder should stop work on
/// it promptly, and every result is already committed on its own, so stopping loses
/// nothing but the photograph in flight.
pub fn run_cancellable(
    lib: &mut Library,
    stages: Stages,
    progress: &(dyn Fn(usize, usize) + Sync),
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Stats> {
    // A stage whose model is absent is simply not run; the rest still are.
    let want_faces = stages.faces && models::find(models::YUNET).is_ok();
    let want_semantic = stages.semantic && semantic::ImageEncoder::available();

    let root = lib.root().to_path_buf();
    // Workers carry the vault rather than the root: the cache is not under the root
    // any more (ADR-0019), and resolving it per job would re-read the marker per job.
    let vault = lib.vault().to_path_buf();
    let rows: Vec<_> = lib.index.all()?.into_iter().filter(|r| r.kind == "photo").collect();

    // Videos cannot join the one-decode pass — their pixels belong to ffmpeg, not
    // imageio — but leaving every poster to the lazy per-cell path made a fresh
    // import pay one ffmpeg spawn mid-scroll, on the same threads that decode
    // photographs. They are built here instead, by a small sub-pass after the photos.
    let ffmpeg = if stages.thumbs { thumbs::resolve() } else { None };
    let video_todo = video_thumb_todo(lib, ffmpeg.as_deref())?;

    let mut todo = Vec::new();
    let mut skipped = 0usize;
    for r in &rows {
        // A photograph blinkview already failed to read is not outstanding work; it is
        // a known limitation, and retrying it every pass is what made a library report
        // the same "15 left" for ever.
        if lib.index.is_unreadable(&r.hash)? {
            skipped += 1;
            continue;
        }
        let thumb = stages.thumbs && !thumbs::thumb_path(lib, &r.hash).exists();
        let faces = want_faces && !lib.faces_done(&r.hash)?;
        let clip = want_semantic && lib.index.get_clip(&r.hash)?.is_none();
        if !(thumb || faces || clip) {
            skipped += 1;
            continue;
        }
        todo.push(Job {
            hash: r.hash.clone(),
            path: lib.abs(&r.path),
            rel: r.path.clone(),
            thumb,
            faces,
            clip,
        });
    }

    let mut st = Stats { considered: rows.len(), skipped, ..Default::default() };
    if todo.is_empty() && video_todo.is_empty() {
        return Ok(st);
    }

    let counter = crate::progress::Counter::new(todo.len() + video_todo.len(), progress);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(workers()).build()?;
    let (tx, rx) = std::sync::mpsc::channel::<Outcome>();

    std::thread::scope(|scope| -> Result<()> {
        // One committing thread: the index is a single connection, and serialising the
        // cheap part costs far less than the decode it is overlapped with.
        let writer = scope.spawn(move || -> Result<Stats> {
            let mut acc = Stats::default();
            for out in rx {
                if let Some(faces) = out.faces {
                    for f in &faces {
                        lib.put_face(f)?;
                    }
                    lib.mark_faces_done(&out.hash)?;
                }
                if let Some(v) = out.clip {
                    lib.index.put_clip(&out.hash, &v)?;
                }
                if let Some(why) = &out.unreadable {
                    lib.index.mark_unreadable(&out.hash, why)?;
                    acc.unreadable += 1;
                }
                merge(&mut acc, out.stats);
            }
            Ok(acc)
        });

        pool.install(|| {
            todo.par_iter().for_each_with(tx, |tx, job| {
                if stop() {
                    return;
                }
                counter.tick();
                let out = process(&vault, job, want_faces);
                let _ = tx.send(out);
            });
        });
        // `tx` was moved into for_each_with and dropped with it, so the writer's
        // channel closes and it can finish.
        let acc = writer.join().map_err(|_| anyhow::anyhow!("commit thread panicked"))??;
        merge(&mut st, acc);
        Ok(())
    })?;

    if !video_todo.is_empty() {
        let (made, first_error) =
            build_video_thumbs(&root, &vault, &video_todo, ffmpeg.as_deref().expect("checked above"), &counter, stop);
        st.thumbs += made;
        if let Some((rel, e)) = first_error {
            st.errors.push(format!("{rel}: thumbnail: {e}"));
        }
    }

    counter.finish();
    Ok(st)
}

/// How many video posters are extracted at once.
///
/// Not [`workers`]: an ffmpeg pulling one frame holds demux and decode buffers of up
/// to ~140 MB for a 1080p stream (measured on the shipped binary), so photograph-pass
/// sizing would run four of those at once on the 8 GB machine this was measured on.
fn video_workers() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 2)
}

/// The videos still owed a poster, given the ffmpeg a pass will use.
///
/// `None` ffmpeg means no video work: without it a poster cannot be made, and
/// pretending otherwise would fail every clip on every pass.
fn video_thumb_todo(
    lib: &Library,
    ffmpeg: Option<&std::ffi::OsStr>,
) -> Result<Vec<(String, std::path::PathBuf)>> {
    let Some(_) = ffmpeg else { return Ok(Vec::new()) };
    Ok(lib
        .index
        .all()?
        .into_iter()
        .filter(|r| r.kind == "video")
        .map(|r| (r.hash, lib.abs(&r.path)))
        .filter(|(hash, _)| !thumbs::thumb_path_in(lib.vault(), hash).exists())
        .collect())
}

/// Extract the missing video posters with ffmpeg, from a resolved binary.
///
/// Returns how many posters were written and the first failure, with the video's path
/// relative to the library root — the same shape the photograph pass reports errors in.
fn build_video_thumbs(
    root: &std::path::Path,
    vault: &std::path::Path,
    todo: &[(String, std::path::PathBuf)],
    bin: &std::ffi::OsStr,
    counter: &crate::progress::Counter,
    stop: &(dyn Fn() -> bool + Sync),
) -> (usize, Option<(String, anyhow::Error)>) {
    let made = std::sync::atomic::AtomicUsize::new(0);
    let first_error: std::sync::Mutex<Option<(String, anyhow::Error)>> = std::sync::Mutex::new(None);
    let pool = match rayon::ThreadPoolBuilder::new().num_threads(video_workers()).build() {
        Ok(p) => p,
        Err(e) => return (0, Some((String::new(), anyhow::Error::new(e)))),
    };
    pool.install(|| {
        todo.par_iter().for_each(|(hash, src)| {
            if stop() {
                return;
            }
            let dst = thumbs::thumb_path_in(vault, hash);
            match thumbs::render_video_with(bin, src, &dst) {
                Ok(()) => {
                    made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    let mut guard = first_error.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.is_none() {
                        // The relative path is what the photograph pass reports; a
                        // video's relative path is its path under the root.
                        let rel = src.strip_prefix(root).unwrap_or(src).display().to_string();
                        *guard = Some((rel, e));
                    }
                }
            }
            counter.tick();
        });
    });
    (made.into_inner(), first_error.into_inner().unwrap_or_else(|p| p.into_inner()))
}

fn merge(into: &mut Stats, from: Stats) {
    into.decoded += from.decoded;
    into.from_preview += from.from_preview;
    into.thumbs += from.thumbs;
    into.faces += from.faces;
    into.too_small += from.too_small;
    into.embedded += from.embedded;
    into.unreadable += from.unreadable;
    into.errors.extend(from.errors);
}

thread_local! {
    static KIT: std::cell::RefCell<Kit> = std::cell::RefCell::new(Kit::default());
}

/// Everything one photograph needs, from at most one decode.
fn process(vault: &std::path::Path, job: &Job, want_faces: bool) -> Outcome {
    let mut st = Stats::default();
    let mut out = Outcome {
        hash: job.hash.clone(),
        faces: None,
        clip: None,
        unreadable: None,
        stats: Stats::default(),
    };

    // A thumbnail on its own can often come from the preview the camera embedded,
    // which is the whole reason not to decode unless something else needs it.
    if job.thumb && !job.faces && !job.clip {
        let dst = thumbs::thumb_path_in(vault, &job.hash);
        match thumbs::render_to(&job.path, &dst, false) {
            Ok(()) => {
                st.thumbs += 1;
                // Whether the preview was used is knowable without redoing the work.
                if imageio::camera_preview(&job.path, thumbs::THUMB_LONG).is_some() {
                    st.from_preview += 1;
                } else {
                    st.decoded += 1;
                }
            }
            Err(e) => {
                st.errors.push(format!("{}: thumbnail: {e}", job.rel));
                out.unreadable = Some(e.to_string());
            }
        }
        out.stats = st;
        return out;
    }

    let orientation = imageio::orientation(&job.path);
    let full = if imageio::needs_conversion(&job.path) {
        imageio::load_rgb(&job.path).map(|i| (i, 1))
    } else {
        imageio::load_rgb_unrotated(&job.path).map(|i| (i, orientation))
    };
    let (full, owed) = match full {
        Ok(v) => v,
        Err(e) => {
            st.errors.push(format!("{}: decode: {e}", job.rel));
            out.unreadable = Some(e.to_string());
            out.stats = st;
            return out;
        }
    };
    st.decoded += 1;

    // Each stage is attempted on its own: a detector that throws must not cost this
    // photograph its thumbnail.
    if job.thumb {
        let dst = thumbs::thumb_path_in(vault, &job.hash);
        match thumbs::render_from_rgb(&full, owed, &dst) {
            Ok(()) => st.thumbs += 1,
            Err(e) => st.errors.push(format!("{}: thumbnail: {e}", job.rel)),
        }
    }

    // The thumbnail wanted `full` unrotated so it could rotate the small image instead
    // of the large one. Everything after this wants it upright, and wants the same
    // upright image, so it is produced once and `full` is consumed doing it.
    let upright = imageio::apply_rgb(full, owed);

    KIT.with(|cell| {
        let mut kit = cell.borrow_mut();
        if job.faces && want_faces {
            if kit.det.is_none() {
                kit.det = models::find(models::YUNET).ok().and_then(|p| detect::Detector::load(&p).ok());
                kit.emb = models::find(models::SFACE).ok().and_then(|p| embed::Embedder::load(&p).ok());
            }
            let Kit { det, emb, .. } = &mut *kit;
            match (det.as_mut(), emb.as_mut()) {
                (Some(det), Some(emb)) => {
                    match faces_from(vault, job, &upright, det, emb, &mut st) {
                        Ok(f) => out.faces = Some(f),
                        Err(e) => st.errors.push(format!("{}: faces: {e}", job.rel)),
                    }
                }
                _ => st.errors.push(format!("{}: faces: models unavailable", job.rel)),
            }
        }
        if job.clip {
            if kit.vision.is_none() {
                kit.vision = semantic::ImageEncoder::load().ok();
            }
            if let Some(v) = kit.vision.as_mut() {
                // CLIP wants a centre crop of the upright image.
                match v.embed(&upright) {
                    Ok(e) => {
                        out.clip = Some(e);
                        st.embedded += 1;
                    }
                    Err(e) => st.errors.push(format!("{}: embedding: {e}", job.rel)),
                }
            }
        }
    });

    out.stats = st;
    out
}

/// The size `DynamicImage::resize` would settle on for the same request.
///
/// `resize` does not use the dimensions handed to it: it refits them to the aspect
/// ratio in f64 and rounds, where the call site had truncated in f32. Resizing the
/// borrowed image directly skips a 36 MB copy, but only mirrors `faces::pipeline` if it
/// lands on exactly the same size, and the two disagree more often than they look like
/// they would: 186 photographs out of 1926 in a real phone backup, enough of them with
/// faces to change the count.
fn fit_dimensions(w: u32, h: u32, nw: u32, nh: u32) -> (u32, u32) {
    let ratio = (nw as f64 / w as f64).min(nh as f64 / h as f64);
    let rw = ((w as f64 * ratio).round() as u64).max(1);
    let rh = ((h as f64 * ratio).round() as u64).max(1);
    (rw.min(u32::MAX as u64) as u32, rh.min(u32::MAX as u64) as u32)
}

/// Detect and embed faces, writing the crops. Mirrors `faces::pipeline` exactly, so the
/// two produce the same rows for the same photograph.
fn faces_from(
    vault: &std::path::Path,
    job: &Job,
    upright: &image::RgbImage,
    det: &mut detect::Detector,
    emb: &mut embed::Embedder,
    st: &mut Stats,
) -> Result<Vec<StoredFace>> {
    // Detection runs at 1280 on the long edge, so the full-resolution image is resized
    // straight from the borrow. Wrapping it in a `DynamicImage` first would copy all
    // 36 MB of a 12 MP photograph, and `to_rgb8` on an already-RGB image copies again;
    // both are pure allocator churn on the way to a 3.7 MB buffer.
    let long = upright.width().max(upright.height());
    let rgb = if long > pipeline::ANALYSIS_LONG_EDGE {
        let s = pipeline::ANALYSIS_LONG_EDGE as f32 / long as f32;
        let (w, h) = fit_dimensions(
            upright.width(),
            upright.height(),
            (upright.width() as f32 * s) as u32,
            (upright.height() as f32 * s) as u32,
        );
        std::borrow::Cow::Owned(image::imageops::resize(
            upright,
            w,
            h,
            image::imageops::FilterType::Triangle,
        ))
    } else {
        std::borrow::Cow::Borrowed(upright)
    };
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);

    let found = det.detect(rgb.as_raw(), w, h, pipeline::DEFAULT_SCORE, pipeline::DEFAULT_NMS)?;
    let mut rows = Vec::new();
    for (i, f) in found.iter().enumerate() {
        // A detection inside the zero padding, or off the edge, is not a real one.
        if f.x >= w as f32 || f.y >= h as f32 {
            continue;
        }
        let embedding = if f.w >= pipeline::MIN_FACE_PX {
            let aligned = embed::align(rgb.as_raw(), w, h, &f.landmarks);
            emb.embed(&aligned).ok()
        } else {
            st.too_small += 1;
            None
        };
        let m = f.w * 0.45;
        let x0 = (f.x - m).max(0.0) as u32;
        let y0 = (f.y - m).max(0.0) as u32;
        let cw = ((f.w + 2.0 * m) as u32).min(w as u32 - x0).max(1);
        let ch = ((f.h + 2.0 * m) as u32).min(h as u32 - y0).max(1);
        let crop = image::imageops::crop_imm(rgb.as_ref(), x0, y0, cw, ch).to_image();
        let sq = image::imageops::resize(
            &crop,
            pipeline::FACE_CROP,
            pipeline::FACE_CROP,
            image::imageops::FilterType::Triangle,
        );
        let dst = pipeline::face_crop_in(vault, &job.hash, i as i64);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = image::DynamicImage::ImageRgb8(sq).save(&dst);

        rows.push(StoredFace {
            hash: job.hash.clone(),
            idx: i as i64,
            x: f.x,
            y: f.y,
            w: f.w,
            h: f.h,
            score: f.score,
            ratio: f.w / w as f32,
            embedding,
        });
        st.faces += 1;
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache beside the fixture, so a unit test never writes to the machine's.
    fn cache_for(dir: &std::path::Path) -> std::path::PathBuf {
        dir.parent()
            .unwrap()
            .join(format!("{}-cache", dir.file_name().unwrap().to_string_lossy()))
    }

    /// `fit_dimensions` must agree with `DynamicImage::resize` on every shape.
    ///
    /// The resolutions here are the ones a real phone backup produced where the two
    /// disagree — 186 photographs out of 1926, nearly a tenth of the library. Detection
    /// runs on the resized pixels, so a single row of difference is enough to change how
    /// many faces come back.
    /// The machine this runs on must not be handed more workers than it has memory for.
    ///
    /// The rule is one worker per 4 GB, capped at four and at the core count. The
    /// regression this guards against is the original sizing, which read cores alone
    /// and gave an 8 GB laptop four workers.
    #[test]
    fn workers_are_sized_by_memory_not_just_cores() {
        let n = workers();
        assert!((1..=4).contains(&n), "workers out of range: {n}");
        if let Some(bytes) = physical_memory() {
            let gb = bytes / (1024 * 1024 * 1024);
            assert!(gb > 0, "physical memory reported as 0");
            let cap = (gb / 4).max(1) as usize;
            assert!(n <= cap.max(1), "{gb} GB machine was given {n} workers");
        }
    }

    #[test]
    fn the_resize_shortcut_lands_where_dynamic_image_would() {
        for (w, h) in [
            (1899, 1148),
            (1599, 722),
            (2944, 2208),
            (12544, 2032),
            (11008, 1808),
            (840, 1425),
            (2005, 4096),
            (1076, 1859),
            (4032, 2268),
            (4032, 3024),
            (1472, 3264),
            (1281, 1281),
        ] {
            let s = pipeline::ANALYSIS_LONG_EDGE as f32 / w.max(h) as f32;
            let (nw, nh) = ((w as f32 * s) as u32, (h as f32 * s) as u32);
            let want = image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h))
                .resize(nw, nh, image::imageops::FilterType::Triangle);
            assert_eq!(
                fit_dimensions(w, h, nw, nh),
                (want.width(), want.height()),
                "{w}x{h}"
            );
        }
    }

    /// A script that answers like ffmpeg and writes a (non-image) file to its final
    /// argument, which is where `render_video_with` puts the output path.
    fn fake_ffmpeg(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("fake-ffmpeg.sh");
        std::fs::write(&p, "#!/bin/sh\nfor last; do :; done\nprintf x > \"$last\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("blinkview-video-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The sub-pass writes a poster for every clip it is given, using the binary it
    /// was resolved once — not one ffmpeg spawn per clip to rediscover it.
    #[test]
    fn the_video_pass_writes_posters_with_a_resolved_binary() {
        let dir = scratch("write");
        let ff = fake_ffmpeg(&dir);
        let todo: Vec<(String, std::path::PathBuf)> =
            [("v1", "a.mp4"), ("v2", "b.mp4")]
                .into_iter()
                .map(|(hash, name)| {
                    std::fs::write(dir.join(name), b"not really a video").unwrap();
                    (hash.to_string(), dir.join(name))
                })
                .collect();
        let sink = crate::progress::silent;
        let counter = crate::progress::Counter::new(todo.len(), &sink);
        let vault = dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let (made, err) =
            build_video_thumbs(&dir, &vault, &todo, ff.as_os_str(), &counter, &|| false);
        assert_eq!(made, 2, "both posters written");
        assert!(err.is_none(), "unexpected failure: {err:?}");
        for (hash, _) in &todo {
            assert!(thumbs::thumb_path_in(&vault, hash).exists(), "{hash} has no poster");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only videos still owed a poster are collected; photographs never are, and an
    /// absent ffmpeg collects nothing rather than failing every clip.
    #[test]
    fn video_todo_is_videos_owed_a_poster() {
        let dir = scratch("todo");
        let lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
        for (hash, name, kind) in [
            ("aaaa", "done.mp4", "video"),
            ("bbbb", "owed.mp4", "video"),
            ("cccc", "shot.jpg", "photo"),
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
            lib.index
                .upsert(&crate::index::FileRow {
                    hash: hash.into(),
                    path: name.into(),
                    size: 1,
                    mtime: 0,
                    kind: kind.into(),
                    taken_at: None,
                    taken_src: None,
                })
                .unwrap();
        }
        // The poster `aaaa` already has.
        let done = thumbs::thumb_path_in(lib.vault(), "aaaa");
        std::fs::create_dir_all(done.parent().unwrap()).unwrap();
        std::fs::write(&done, b"poster").unwrap();

        let with_ffmpeg =
            video_thumb_todo(&lib, Some(std::ffi::OsStr::new("ffmpeg"))).unwrap();
        assert_eq!(with_ffmpeg.len(), 1, "only the video without a poster: {with_ffmpeg:?}");
        assert_eq!(with_ffmpeg[0].0, "bbbb");

        assert!(video_thumb_todo(&lib, None).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
