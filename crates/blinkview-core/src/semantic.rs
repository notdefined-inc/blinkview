//! Searching photographs by what is in them.
//!
//! Face embeddings cannot answer "the one with the dog" — SFace encodes facial identity
//! and nothing else. This uses MobileCLIP-S0, which embeds images *and* text into one
//! 512-dimensional space, so a typed phrase becomes a vector comparable against every
//! photo. See ADR-0008 for why this model and what it is measurably good and bad at.

use crate::{imageio, Library};
use anyhow::Result;
use ndarray::{Array2, Array4};
use ort::session::Session;
use std::path::Path;

pub const VISION: &str = "clip-vision.onnx";
pub const TEXT: &str = "clip-text.onnx";
pub const TOKENIZER: &str = "clip-tokenizer.json";

/// Both encoders emit this many dimensions.
pub const DIM: usize = 512;
/// The vision model's input side, from its own `preprocessor_config.json`.
const SIDE: u32 = 256;
/// CLIP's fixed context. The text encoder rejects anything shorter, so queries are
/// padded rather than sent at their natural length.
const CTX: usize = 77;
/// Below this, a match is not worth showing.
///
/// Measured on 237 real photos with the fp32 text tower: the lowest true positive scores
/// 0.183 and the highest false positive 0.168, so 0.18 sits at the top of that gap. It
/// errs toward returning nothing over returning a wrong answer, which is the intended
/// bias — a search that confidently shows the wrong photo is worse than one that admits
/// it found nothing. Re-derive it (ADR-0008) if either encoder changes; the earlier
/// int8 text model inflated scores enough to nearly double what cleared this line.
pub const DEFAULT_THRESHOLD: f32 = 0.18;

/// The text half on its own.
///
/// Searching needs no vision tower, and loading one costs 45 MB of resident memory for
/// nothing. Held open across queries by the app: loading costs ~270 ms against ~15 ms
/// to embed a phrase, so a fresh load per keystroke would dominate.
pub struct TextEncoder {
    session: Session,
    tokenizer: tokenizers::Tokenizer,
    input: String,
}

impl TextEncoder {
    pub fn load() -> Result<Self> {
        let tp = crate::faces::models::find(TEXT)?;
        let kp = crate::faces::models::find(TOKENIZER)?;
        let session = Session::builder()?.commit_from_file(&tp)?;
        let tokenizer = tokenizers::Tokenizer::from_file(&kp)
            .map_err(|e| anyhow::anyhow!("loading {}: {e}", kp.display()))?;
        Ok(Self {
            input: session.inputs()[0].name().to_string(),
            session,
            tokenizer,
        })
    }

    pub fn available() -> bool {
        [TEXT, TOKENIZER]
            .iter()
            .all(|n| crate::faces::models::find(n).is_ok())
    }

    /// Embed a phrase into the same 512-d space as [`Encoder::embed_image`].
    pub fn embed(&mut self, query: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(query, true)
            .map_err(|e| anyhow::anyhow!("tokenising {query:?}: {e}"))?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).take(CTX).collect();
        ids.resize(CTX, 0);
        let input = Array2::from_shape_vec((1, CTX), ids)?;
        let out = self
            .session
            .run(ort::inputs![self.input.as_str() => ort::value::Tensor::from_array(input)?])?;
        let (_, data) = out[0].try_extract_tensor::<f32>()?;
        Ok(unit(data))
    }
}

/// The vision half on its own.
///
/// Analysis embeds photographs and never a phrase, so loading the 161 MB text tower
/// alongside it bought nothing — and cost that much again for every worker thread
/// once the pass went parallel (ADR-0013).
pub struct ImageEncoder {
    session: Session,
    input: String,
}

impl ImageEncoder {
    pub fn load() -> Result<Self> {
        let vp = crate::faces::models::find(VISION)?;
        let session = Session::builder()?.commit_from_file(&vp)?;
        Ok(Self {
            input: session.inputs()[0].name().to_string(),
            session,
        })
    }

    pub fn available() -> bool {
        crate::faces::models::find(VISION).is_ok()
    }

    /// Embed a photograph read from disk.
    pub fn embed_path(&mut self, path: &Path) -> Result<Vec<f32>> {
        self.embed(&imageio::load_rgb(path)?)
    }

    /// Embed pixels already in hand — the shared decode of ADR-0013.
    ///
    /// Preprocessing follows the model's own config: shortest edge to 256, centre crop,
    /// scale to 0..1, and **no mean/std normalisation**, which is unusual for CLIP and
    /// silently wrong if the usual defaults are assumed.
    pub fn embed(&mut self, img: &image::RgbImage) -> Result<Vec<f32>> {
        let (w, h) = (img.width(), img.height());
        let scale = SIDE as f32 / w.min(h) as f32;
        let rw = ((w as f32 * scale).round() as u32).max(SIDE);
        let rh = ((h as f32 * scale).round() as u32).max(SIDE);
        let resized = image::imageops::resize(img, rw, rh, image::imageops::FilterType::CatmullRom);
        let cropped =
            image::imageops::crop_imm(&resized, (rw - SIDE) / 2, (rh - SIDE) / 2, SIDE, SIDE)
                .to_image();

        let mut input = Array4::<f32>::zeros((1, 3, SIDE as usize, SIDE as usize));
        for (x, y, p) in cropped.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = f32::from(p[c]) / 255.0;
            }
        }
        let out = self
            .session
            .run(ort::inputs![self.input.as_str() => ort::value::Tensor::from_array(input)?])?;
        let (_, data) = out[0].try_extract_tensor::<f32>()?;
        Ok(unit(data))
    }
}

pub struct Encoder {
    vision: ImageEncoder,
    text: TextEncoder,
}

impl Encoder {
    pub fn load() -> Result<Self> {
        Ok(Self {
            vision: ImageEncoder::load()?,
            text: TextEncoder::load()?,
        })
    }

    /// True when the models are installed, so callers can degrade rather than fail.
    pub fn available() -> bool {
        [VISION, TEXT, TOKENIZER]
            .iter()
            .all(|n| crate::faces::models::find(n).is_ok())
    }

    /// Embed a photograph.
    ///
    /// Preprocessing follows the model's own config: shortest edge to 256, centre crop,
    /// scale to 0..1, and **no mean/std normalisation** — `do_normalize` is false for
    /// this model, which is unusual and silently wrong if CLIP's defaults are assumed.
    pub fn embed_image(&mut self, path: &Path) -> Result<Vec<f32>> {
        self.vision.embed_path(path)
    }

    /// Embed a search phrase into the same space.
    pub fn embed_text(&mut self, query: &str) -> Result<Vec<f32>> {
        self.text.embed(query)
    }
}

fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

/// Cosine similarity between two unit vectors.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[derive(Debug, Default)]
pub struct AnalyzeStats {
    pub embedded: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Embed every photo that has no embedding yet.
///
/// Each result is written as it is produced, so interrupting the run loses only the
/// photo in flight rather than the whole pass.
pub fn analyze(lib: &Library, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<AnalyzeStats> {
    let mut st = AnalyzeStats::default();
    let rows: Vec<_> = lib
        .index
        .all()?
        .into_iter()
        .filter(|r| r.kind == "photo")
        .collect();
    let mut todo = Vec::new();
    for r in &rows {
        if lib.index.get_clip(&r.hash)?.is_some() {
            st.skipped += 1;
        } else {
            todo.push(r.clone());
        }
    }
    if todo.is_empty() {
        return Ok(st);
    }

    let mut enc = Encoder::load()?;
    let counter = crate::progress::Counter::new(todo.len(), progress);
    for r in todo {
        counter.tick();
        match enc.embed_image(&lib.abs(&r.path)) {
            Ok(e) => {
                lib.index.put_clip(&r.hash, &e)?;
                st.embedded += 1;
            }
            Err(e) => st.errors.push(format!("{}: {e}", r.path)),
        }
    }
    counter.finish();
    Ok(st)
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub hash: String,
    pub score: f32,
}

/// Rank photographs against a phrase, dropping anything below `threshold`.
///
/// Returning nothing is the right answer for a query the model cannot serve. Showing
/// the least-bad photograph would present a guess as a result.
pub fn search(lib: &Library, query: &str, threshold: f32, limit: usize) -> Result<Vec<Hit>> {
    search_with(lib, &mut TextEncoder::load()?, query, threshold, limit)
}

/// As [`search`], against an encoder the caller keeps open.
///
/// The app holds one for the life of the window; reloading it per keystroke would cost
/// ~270 ms against the ~15 ms the query itself takes.
pub fn search_with(
    lib: &Library,
    enc: &mut TextEncoder,
    query: &str,
    threshold: f32,
    limit: usize,
) -> Result<Vec<Hit>> {
    let q = enc.embed(query)?;
    let mut hits: Vec<Hit> = lib
        .index
        .all_clip()?
        .into_iter()
        .map(|(hash, e)| Hit {
            score: similarity(&q, &e),
            hash,
        })
        .filter(|h| h.score >= threshold)
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_normalises() {
        let v = unit(&[3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert!((similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn unit_survives_a_zero_vector() {
        let v = unit(&[0.0, 0.0, 0.0]);
        assert!(v.iter().all(|x| x.is_finite()), "must not produce NaN");
    }

    #[test]
    fn threshold_sits_in_the_measured_gap() {
        // ADR-0008, re-measured on 237 photos with the fp32 text tower: the lowest
        // true positive scored 0.183 and the highest false positive 0.168. A threshold
        // outside that band either drops correct results or admits wrong ones.
        const HIGHEST_FALSE_POSITIVE: f32 = 0.168;
        const LOWEST_TRUE_POSITIVE: f32 = 0.183;
        let t = std::hint::black_box(DEFAULT_THRESHOLD);
        assert!(
            t > HIGHEST_FALSE_POSITIVE && t <= LOWEST_TRUE_POSITIVE,
            "threshold {t} is outside the measured separation \
             ({HIGHEST_FALSE_POSITIVE}..={LOWEST_TRUE_POSITIVE}); re-derive it per ADR-0008"
        );
    }
}
