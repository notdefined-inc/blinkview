//! SFace embeddings, with the landmark alignment OpenCV's `alignCrop` performs.
//!
//! Alignment is not optional. SFace expects a face warped onto a fixed 5-point
//! template; feeding it a plain crop produces embeddings that do not match the
//! similarity thresholds in ADR-0003. The template below is ArcFace's canonical
//! 112x112 layout, identical to the one in OpenCV's `face_recognize.cpp`.

use anyhow::{Context, Result};
use ndarray::Array4;
use ort::session::Session;

pub const SIDE: usize = 112;
pub const DIM: usize = 128;

/// Canonical landmark positions for a 112x112 aligned face.
const TEMPLATE: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

/// Least-squares similarity transform (rotation + uniform scale + translation)
/// mapping `src` onto `dst`. This is the Umeyama estimator restricted to the
/// similarity group, which is what `getSimilarityTransformMatrix` computes.
///
/// Returns the 2x3 affine matrix as [a, b, tx, c, d, ty].
fn similarity_transform(src: &[(f32, f32); 5], dst: &[(f32, f32); 5]) -> [f32; 6] {
    let n = src.len() as f32;
    let mean = |p: &[(f32, f32); 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(ax, ay), (x, y)| (ax + x, ay + y));
        (sx / n, sy / n)
    };
    let (smx, smy) = mean(src);
    let (dmx, dmy) = mean(dst);

    // Variance of the source, and the cross-covariance terms. For a similarity
    // transform in 2D these collapse to two scalars (a rotation and a scale).
    let mut var_s = 0.0;
    let (mut cov_a, mut cov_b) = (0.0, 0.0);
    for i in 0..5 {
        let (sx, sy) = (src[i].0 - smx, src[i].1 - smy);
        let (dx, dy) = (dst[i].0 - dmx, dst[i].1 - dmy);
        var_s += sx * sx + sy * sy;
        cov_a += sx * dx + sy * dy;
        cov_b += sx * dy - sy * dx;
    }
    if var_s.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    }
    let (a, b) = (cov_a / var_s, cov_b / var_s);
    // [ a -b ] is scale*rotation; translation places the source centroid on the target.
    [a, -b, dmx - (a * smx - b * smy), b, a, dmy - (b * smx + a * smy)]
}

/// Invert a 2x3 affine so the destination grid can be sampled from the source.
fn invert(m: &[f32; 6]) -> Option<[f32; 6]> {
    let det = m[0] * m[4] - m[1] * m[3];
    if det.abs() < 1e-12 {
        return None;
    }
    let (ia, ib, ic, id) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    Some([ia, ib, -(ia * m[2] + ib * m[5]), ic, id, -(ic * m[2] + id * m[5])])
}

/// Warp the face onto the 112x112 template with bilinear sampling.
pub fn align(rgb: &[u8], w: usize, h: usize, landmarks: &[(f32, f32); 5]) -> Vec<u8> {
    let fwd = similarity_transform(landmarks, &TEMPLATE);
    let inv = invert(&fwd).unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut out = vec![0u8; SIDE * SIDE * 3];
    for oy in 0..SIDE {
        for ox in 0..SIDE {
            let (fx, fy) = (ox as f32, oy as f32);
            let sx = inv[0] * fx + inv[1] * fy + inv[2];
            let sy = inv[3] * fx + inv[4] * fy + inv[5];
            let (x0, y0) = (sx.floor(), sy.floor());
            let (tx, ty) = (sx - x0, sy - y0);
            for ch in 0..3 {
                let mut acc = 0.0f32;
                for (dy, wy) in [(0.0, 1.0 - ty), (1.0, ty)] {
                    for (dx, wx) in [(0.0, 1.0 - tx), (1.0, tx)] {
                        let px = (x0 + dx).clamp(0.0, (w - 1) as f32) as usize;
                        let py = (y0 + dy).clamp(0.0, (h - 1) as f32) as usize;
                        acc += wx * wy * f32::from(rgb[(py * w + px) * 3 + ch]);
                    }
                }
                out[(oy * SIDE + ox) * 3 + ch] = acc.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

pub struct Embedder {
    session: Session,
    input_name: String,
}

impl Embedder {
    pub fn load(model: &std::path::Path) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model)
            .with_context(|| format!("loading recognizer {}", model.display()))?;
        let input_name = session.inputs()[0].name().to_string();
        Ok(Self { session, input_name })
    }

    /// Embed an aligned 112x112 RGB face. Result is L2-normalized, so a dot product
    /// between two of them is cosine similarity.
    ///
    /// Channel order is RGB, and that is not arbitrary. OpenCV's `FaceRecognizerSF`
    /// calls `blobFromImage(..., swapRB = true)` on its BGR image, so the network is
    /// fed RGB — whereas `FaceDetectorYN` uses the default `swapRB = false` and is fed
    /// BGR. Getting this backwards still yields plausible-looking embeddings that
    /// cluster roughly correctly, but they only reach ~0.91 cosine against the
    /// reference implementation instead of ~1.0, which would silently invalidate every
    /// similarity threshold in ADR-0003.
    pub fn embed(&mut self, aligned_rgb: &[u8]) -> Result<Vec<f32>> {
        let mut input = Array4::<f32>::zeros((1, 3, SIDE, SIDE));
        for y in 0..SIDE {
            for x in 0..SIDE {
                let p = (y * SIDE + x) * 3;
                input[[0, 0, y, x]] = f32::from(aligned_rgb[p]);     // R
                input[[0, 1, y, x]] = f32::from(aligned_rgb[p + 1]); // G
                input[[0, 2, y, x]] = f32::from(aligned_rgb[p + 2]); // B
            }
        }
        let out = self
            .session
            .run(ort::inputs![self.input_name.as_str() => ort::value::Tensor::from_array(input)?])?;
        let (_, data) = out[0].try_extract_tensor::<f32>()?;
        let norm = data.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
        Ok(data.iter().map(|v| v / norm).collect())
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_maps_template_onto_itself() {
        let m = similarity_transform(&TEMPLATE, &TEMPLATE);
        for (x, y) in TEMPLATE {
            assert!((m[0] * x + m[1] * y + m[2] - x).abs() < 1e-3);
            assert!((m[3] * x + m[4] * y + m[5] - y).abs() < 1e-3);
        }
    }

    #[test]
    fn transform_recovers_a_known_scale_and_shift() {
        // Landmarks at half scale, offset by (10, 20), must map back onto the template.
        let src: [(f32, f32); 5] = std::array::from_fn(|i| (TEMPLATE[i].0 * 0.5 + 10.0, TEMPLATE[i].1 * 0.5 + 20.0));
        let m = similarity_transform(&src, &TEMPLATE);
        for i in 0..5 {
            let (x, y) = src[i];
            assert!((m[0] * x + m[1] * y + m[2] - TEMPLATE[i].0).abs() < 1e-2);
            assert!((m[3] * x + m[4] * y + m[5] - TEMPLATE[i].1).abs() < 1e-2);
        }
    }

    #[test]
    fn inverse_round_trips() {
        let m = [1.5, -0.3, 4.0, 0.3, 1.5, -2.0];
        let i = invert(&m).unwrap();
        let (x, y) = (7.0f32, -3.0f32);
        let (fx, fy) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
        assert!((i[0] * fx + i[1] * fy + i[2] - x).abs() < 1e-3);
        assert!((i[3] * fx + i[4] * fy + i[5] - y).abs() < 1e-3);
    }

    #[test]
    fn cosine_of_identical_unit_vectors_is_one() {
        let v: Vec<f32> = (0..DIM).map(|i| ((i as f32) - 64.0) / 100.0).collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let u: Vec<f32> = v.iter().map(|x| x / n).collect();
        assert!((cosine(&u, &u) - 1.0).abs() < 1e-5);
    }
}
