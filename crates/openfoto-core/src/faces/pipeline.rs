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

    for r in todo {
        st.photos += 1;
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
    Ok(cluster::complete_linkage(n, pairs, close)
        .into_iter()
        .map(|g| g.into_iter().map(|i| faces[i].clone()).collect())
        .collect())
}
