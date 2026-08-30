//! Running detection over a library and grouping the results.

use crate::{
    cluster,
    faces::{assign, detect, embed, models, people::People, store::StoredFace},
    Library,
};
use anyhow::Result;

/// Long edge the image is scaled to before detection. 1280 is what the reference
/// library was analysed at; the ratio recorded per face is scale-invariant, but the
/// pixel-size floor below is not, so the two move together.
pub const ANALYSIS_LONG_EDGE: u32 = 1280;
/// Below this face width in analysis pixels, SFace embeddings are unreliable enough
/// that a match is not worth trusting (ADR-0003).
pub const MIN_FACE_PX: f32 = 50.0;
pub const DEFAULT_SCORE: f32 = 0.75;
pub const DEFAULT_NMS: f32 = 0.3;
/// Side of the cached face crop, in pixels.
pub const FACE_CROP: u32 = 160;

/// Where a detected face's cached crop lives.
///
/// Written during analysis rather than on demand, because the UI wants to show faces
/// in lists and sidebars: producing them lazily would mean decoding a full photo to
/// draw a 28px avatar.
pub fn face_crop_path(root: &std::path::Path, hash: &str, idx: i64) -> std::path::PathBuf {
    root.join(crate::library::VAULT_DIR)
        .join("faces")
        .join(format!("{hash}-{idx}.jpg"))
}

#[derive(Debug, Default)]
pub struct AnalyzeStats {
    pub photos: usize,
    pub skipped_cached: usize,
    pub faces: usize,
    pub too_small: usize,
    pub errors: Vec<String>,
}

/// Detect and embed faces for every photo that has not been analysed yet.
///
/// Single-threaded: an `ort` Session is not shared across threads, and decode is no
/// longer the bottleneck here — inference is.
pub fn analyze(lib: &Library, score_thr: f32) -> Result<AnalyzeStats> {
    analyze_with_progress(lib, score_thr, &crate::progress::silent)
}

/// As [`analyze`], reporting (done, total). This is the slowest operation blinkview
/// performs, so it is the one that most needs to prove it is still working.
pub fn analyze_with_progress(
    lib: &Library,
    score_thr: f32,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<AnalyzeStats> {
    let mut st = AnalyzeStats::default();
    let rows: Vec<_> = lib.index.all()?.into_iter().filter(|r| r.kind == "photo").collect();

    let mut todo = Vec::new();
    for r in &rows {
        if lib.faces_done(&r.hash)? {
            st.skipped_cached += 1;
        } else {
            todo.push(r.clone());
        }
    }
    if todo.is_empty() {
        return Ok(st);
    }

    let mut det = detect::Detector::load(&models::find(models::YUNET)?)?;
    let mut emb = embed::Embedder::load(&models::find(models::SFACE)?)?;
    let counter = crate::progress::Counter::new(todo.len(), progress);

    for r in todo {
        st.photos += 1;
        counter.tick();
        let path = lib.abs(&r.path);
        let rgb0 = match crate::imageio::load_rgb(&path) {
            Ok(i) => i,
            Err(e) => {
                st.errors.push(format!("{}: {e}", r.path));
                continue;
            }
        };
        let img = image::DynamicImage::ImageRgb8(rgb0);
        let img = if img.width().max(img.height()) > ANALYSIS_LONG_EDGE {
            let s = ANALYSIS_LONG_EDGE as f32 / img.width().max(img.height()) as f32;
            img.resize(
                (img.width() as f32 * s) as u32,
                (img.height() as f32 * s) as u32,
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);

        let faces = match det.detect(rgb.as_raw(), w, h, score_thr, DEFAULT_NMS) {
            Ok(f) => f,
            Err(e) => {
                st.errors.push(format!("{}: {e}", r.path));
                continue;
            }
        };
        for (i, f) in faces.iter().enumerate() {
            // A face detected inside the zero padding, or clipped off the edge, is
            // not a real detection.
            if f.x >= w as f32 || f.y >= h as f32 {
                continue;
            }
            let embedding = if f.w >= MIN_FACE_PX {
                let aligned = embed::align(rgb.as_raw(), w, h, &f.landmarks);
                emb.embed(&aligned).ok()
            } else {
                st.too_small += 1;
                None
            };
            // Cache a square crop so faces can be shown without re-decoding the photo.
            let m = f.w * 0.45;
            let x0 = (f.x - m).max(0.0) as u32;
            let y0 = (f.y - m).max(0.0) as u32;
            let cw = ((f.w + 2.0 * m) as u32).min(w as u32 - x0).max(1);
            let ch = ((f.h + 2.0 * m) as u32).min(h as u32 - y0).max(1);
            let crop = image::imageops::crop_imm(&rgb, x0, y0, cw, ch).to_image();
            let sq = image::imageops::resize(
                &crop, FACE_CROP, FACE_CROP, image::imageops::FilterType::Triangle);
            let dst = face_crop_path(lib.root(), &r.hash, i as i64);
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = image::DynamicImage::ImageRgb8(sq).save(&dst);

            lib.put_face(&StoredFace {
                hash: r.hash.clone(),
                idx: i as i64,
                x: f.x,
                y: f.y,
                w: f.w,
                h: f.h,
                score: f.score,
                ratio: f.w / w as f32,
                embedding,
            })?;
            st.faces += 1;
        }
        lib.mark_faces_done(&r.hash)?;
    }
    counter.finish();
    Ok(st)
}

/// Group faces that no known person claims, so the user can name them.
///
/// Complete-linkage, like duplicate detection and for the same reason: a chain of
/// pairwise-similar faces is not a person.
pub fn cluster_unassigned(
    lib: &Library,
    people: &People,
    opt: &assign::Options,
    max_distance: f32,
) -> Result<Vec<Vec<StoredFace>>> {
    let faces: Vec<StoredFace> = lib
        .all_faces()?
        .into_iter()
        .filter(|f| f.embedding.is_some())
        // A dismissed face is out of review but still in the index: nothing is
        // deleted, and restoring is only putting the list back.
        .filter(|f| !people.is_dismissed(&f.hash, f.idx))
        .filter(|f| {
            let e = f.embedding.as_ref().unwrap();
            assign::assign(e, people, opt).person().is_none()
        })
        .collect();

    let embs: Vec<&Vec<f32>> = faces.iter().map(|f| f.embedding.as_ref().unwrap()).collect();
    let n = embs.len();
    let close = |a: usize, b: usize| 1.0 - embed::cosine(embs[a], embs[b]) <= max_distance;

    let mut pairs = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let d = 1.0 - embed::cosine(embs[i], embs[j]);
            if d <= max_distance {
                pairs.push((d, i, j));
            }
        }
    }
    // complete_linkage only returns groups of two or more; a face that resembles
    // nothing else is still someone, so it comes back as a group of one.
    let mut groups = cluster::complete_linkage(n, pairs, close);
    let grouped: std::collections::HashSet<usize> = groups.iter().flatten().copied().collect();
    for i in 0..n {
        if !grouped.contains(&i) {
            groups.push(vec![i]);
        }
    }
    Ok(groups
        .into_iter()
        .map(|g| g.into_iter().map(|i| faces[i].clone()).collect())
        .collect())
}
