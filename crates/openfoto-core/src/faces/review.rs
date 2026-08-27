//! Building the payload the review page renders.
//!
//! The review step is the part no comparable tool offers, and it exists because
//! automation was measurably wrong: clustering proposes groups, a human confirms them.
//! Everything here is read-only — reviewing never moves a file.

use crate::{
    faces::{assign, people::People, store::StoredFace},
    imageio, Library,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Thumbnail crop side. Small: these tile at ~44px in the strip.
const CROP: u32 = 128;
/// Hero crop side. The card shows this at ~230px, so 128 would visibly soften it.
const HERO_CROP: u32 = 320;
/// Padding around the detected box, as a fraction of face width. Enough to include
/// hair and chin — a box-tight crop reads as a clipped head, not a portrait.
const CROP_MARGIN: f32 = 0.55;
/// Crops shown per cluster. Enough to judge identity without bloating the page.
const MAX_CROPS: usize = 12;

#[derive(Debug, Serialize)]
pub struct ReviewFace {
    pub hash: String,
    pub idx: i64,
    /// JPEG data URI of the face crop.
    pub crop: String,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct ReviewCluster {
    pub id: usize,
    pub faces: Vec<ReviewFace>,
    pub photo_count: usize,
    pub face_count: usize,
    /// Best-matching known person, if any, with its similarity.
    pub suggestion: Option<String>,
    pub similarity: Option<f32>,
    pub runner_up: Option<String>,
    pub runner_up_similarity: Option<f32>,
    /// True when two identities are too close to separate confidently.
    pub ambiguous: bool,
    /// Unit-length mean embedding, for live client-side re-suggestion.
    pub centroid: Vec<f32>,
}

#[derive(Debug, Serialize)]
pub struct ReviewPayload {
    pub library: String,
    pub clusters: Vec<ReviewCluster>,
    pub known_people: Vec<String>,
    pub total_faces: usize,
    pub unassigned_faces: usize,
}

/// What the page sends back.
#[derive(Debug, Deserialize, Default)]
pub struct ReviewResult {
    /// cluster id -> person name. Absent means "leave alone".
    pub assignments: std::collections::BTreeMap<usize, String>,
}

fn crop_data_uri(lib: &Library, f: &StoredFace, path: &str, side: u32) -> Option<String> {
    let img = imageio::load_rgb(&lib.abs(path)).ok()?;
    // Face coordinates are in *analysis* space: the image scaled so its long edge is
    // ANALYSIS_LONG_EDGE, or left alone if it was already smaller. Recover that exact
    // factor rather than approximating it, or crops land on empty sky.
    let long = img.width().max(img.height());
    let analysis_long = long.min(crate::faces::pipeline::ANALYSIS_LONG_EDGE);
    let scale = long as f32 / analysis_long as f32;
    let m = f.w * CROP_MARGIN;
    let x0 = ((f.x - m) * scale).max(0.0) as u32;
    let y0 = ((f.y - m) * scale).max(0.0) as u32;
    let w = (((f.w + 2.0 * m) * scale) as u32).min(img.width().saturating_sub(x0)).max(1);
    let h = (((f.h + 2.0 * m) * scale) as u32).min(img.height().saturating_sub(y0)).max(1);
    let sub = image::imageops::crop_imm(&img, x0, y0, w, h).to_image();
    let sq = image::imageops::resize(&sub, side, side, image::imageops::FilterType::Lanczos3);

    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(sq)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .ok()?;
    Some(format!("data:image/jpeg;base64,{}", b64(&buf.into_inner())))
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

pub fn build(lib: &Library, people: &People, opt: &assign::Options, max_distance: f32) -> Result<ReviewPayload> {
    let hash_to_path: std::collections::BTreeMap<String, String> =
        lib.index.all()?.into_iter().map(|r| (r.hash, r.path)).collect();
    let all = lib.all_faces()?;
    let total_faces = all.len();

    let groups = crate::faces::pipeline::cluster_unassigned(lib, people, opt, max_distance)?;
    let unassigned_faces = groups.iter().map(|g| g.len()).sum();

    let mut clusters = Vec::new();
    for (id, g) in groups.iter().enumerate() {
        // Order by detection confidence, not box size. The largest face in a burst is
        // often a motion-blurred near-miss; score tracks how cleanly frontal and sharp
        // the face is, which is what makes a good card hero. Size breaks ties.
        let mut sorted = g.clone();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.w.partial_cmp(&a.w).unwrap_or(std::cmp::Ordering::Equal))
        });

        // Suggestion comes from the group's mean, which is more stable than any
        // single face when the cluster spans poses.
        let (mut suggestion, mut similarity, mut runner_up, mut runner_up_similarity, mut ambiguous) =
            (None, None, None, None, false);
        if !people.is_empty() {
            if let Some(e) = sorted.first().and_then(|f| f.embedding.clone()) {
                let scores = assign::score_all(&e, people);
                if let Some((n, s)) = scores.first() {
                    suggestion = Some(n.clone());
                    similarity = Some(*s);
                }
                if let Some((n, s)) = scores.get(1) {
                    runner_up = Some(n.clone());
                    runner_up_similarity = Some(*s);
                    ambiguous = similarity.unwrap_or(0.0) - s < opt.margin;
                }
            }
        }

        let faces: Vec<ReviewFace> = sorted
            .iter()
            .take(MAX_CROPS)
            .enumerate()
            .filter_map(|(i, f)| {
                let path = hash_to_path.get(&f.hash)?;
                // The first crop is the card's hero and is rendered large.
                let side = if i == 0 { HERO_CROP } else { CROP };
                Some(ReviewFace {
                    hash: f.hash.clone(),
                    idx: f.idx,
                    crop: crop_data_uri(lib, f, path, side)?,
                    score: f.score,
                })
            })
            .collect();

        // Unit-length mean of the group. Shipped to the page so that naming one group
        // can immediately re-suggest the same person for every other group that looks
        // like them, without a round trip.
        let mut centroid = vec![0.0f32; crate::faces::embed::DIM];
        let mut n = 0.0f32;
        for f in g.iter().filter_map(|f| f.embedding.as_ref()) {
            for (c, v) in centroid.iter_mut().zip(f) {
                *c += v;
            }
            n += 1.0;
        }
        if n > 0.0 {
            let norm = centroid.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
            for c in centroid.iter_mut() {
                *c /= norm;
            }
        }

        clusters.push(ReviewCluster {
            id,
            faces,
            photo_count: g.iter().map(|f| &f.hash).collect::<std::collections::BTreeSet<_>>().len(),
            face_count: g.len(),
            suggestion,
            similarity,
            runner_up,
            runner_up_similarity,
            ambiguous,
            centroid,
        });
    }

    Ok(ReviewPayload {
        library: lib.root().display().to_string(),
        clusters,
        known_people: people.people.iter().map(|p| p.name.clone()).collect(),
        total_faces,
        unassigned_faces,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(super::b64(b""), "");
        assert_eq!(super::b64(b"f"), "Zg==");
        assert_eq!(super::b64(b"fo"), "Zm8=");
        assert_eq!(super::b64(b"foo"), "Zm9v");
        assert_eq!(super::b64(b"foob"), "Zm9vYg==");
        assert_eq!(super::b64(b"foobar"), "Zm9vYmFy");
    }
}
