//! YuNet face detection via ONNX Runtime.
//!
//! Uses the `2026may` export specifically because its height/width axes are dynamic.
//! The `2023mar` export declares a fixed 640x640 input; OpenCV's DNN module silently
//! reshaped the graph, but `ort` will not. Forcing photos into 640x640 would halve the
//! working resolution and drop small faces below the size where SFace embeddings are
//! reliable (see ADR-0003), so the dynamic export is load-bearing, not incidental.
//!
//! Decoding follows OpenCV's `face_detect.cpp` so scores and boxes stay comparable to
//! the values the thresholds in ADR-0003 were tuned against.

use anyhow::{Context, Result};
use ndarray::Array4;
use ort::session::Session;

const STRIDES: [usize; 3] = [8, 16, 32];

#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Right eye, left eye, nose, right mouth corner, left mouth corner.
    pub landmarks: [(f32, f32); 5],
}

impl Face {
    pub fn area(&self) -> f32 {
        self.w * self.h
    }
    fn iou(&self, o: &Face) -> f32 {
        let x1 = self.x.max(o.x);
        let y1 = self.y.max(o.y);
        let x2 = (self.x + self.w).min(o.x + o.w);
        let y2 = (self.y + self.h).min(o.y + o.h);
        let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
        let union = self.area() + o.area() - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
}

pub struct Detector {
    session: Session,
    input_name: String,
}

impl Detector {
    pub fn load(model: &std::path::Path) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model)
            .with_context(|| format!("loading detector {}", model.display()))?;
        let input_name = session.inputs()[0].name().to_string();
        Ok(Self {
            session,
            input_name,
        })
    }

    /// Detect faces in an RGB image. Coordinates are in that image's pixel space.
    ///
    /// The input is zero-padded on the right and bottom to a multiple of 32. The
    /// network's feature pyramid halves resolution three times, so a size that is not
    /// a multiple of 32 makes the stride-32 branch disagree with its skip connection
    /// (it fails with "broadcast an axis by a dimension other than 1, 36 by 37").
    /// Padding on the far edges leaves the origin untouched, so detected coordinates
    /// need no correction.
    pub fn detect(
        &mut self,
        rgb: &[u8],
        w: usize,
        h: usize,
        score_thr: f32,
        nms_thr: f32,
    ) -> Result<Vec<Face>> {
        let pw = w.div_ceil(32) * 32;
        let ph = h.div_ceil(32) * 32;

        // The model was trained on OpenCV BGR input with no scaling or mean subtraction.
        let mut input = Array4::<f32>::zeros((1, 3, ph, pw));
        for y in 0..h {
            for x in 0..w {
                let p = (y * w + x) * 3;
                input[[0, 0, y, x]] = f32::from(rgb[p + 2]); // B
                input[[0, 1, y, x]] = f32::from(rgb[p + 1]); // G
                input[[0, 2, y, x]] = f32::from(rgb[p]); // R
            }
        }
        let (w, h) = (pw, ph);
        let outputs = self.session.run(
            ort::inputs![self.input_name.as_str() => ort::value::Tensor::from_array(input)?],
        )?;

        let mut faces = Vec::new();
        for stride in STRIDES {
            let cls = outputs[format!("cls_{stride}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            let obj = outputs[format!("obj_{stride}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            let bbox = outputs[format!("bbox_{stride}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            let kps = outputs[format!("kps_{stride}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;

            let cols = w / stride;
            let rows = h / stride;
            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    if i >= cls.len() {
                        continue;
                    }
                    let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                    if score < score_thr {
                        continue;
                    }
                    let b = &bbox[i * 4..i * 4 + 4];
                    let s = stride as f32;
                    let (cx, cy) = ((c as f32 + b[0]) * s, (r as f32 + b[1]) * s);
                    let (fw, fh) = (b[2].exp() * s, b[3].exp() * s);
                    let mut landmarks = [(0.0f32, 0.0f32); 5];
                    for (n, lm) in landmarks.iter_mut().enumerate() {
                        *lm = (
                            (c as f32 + kps[i * 10 + 2 * n]) * s,
                            (r as f32 + kps[i * 10 + 2 * n + 1]) * s,
                        );
                    }
                    faces.push(Face {
                        x: cx - fw / 2.0,
                        y: cy - fh / 2.0,
                        w: fw,
                        h: fh,
                        score,
                        landmarks,
                    });
                }
            }
        }
        Ok(nms(faces, nms_thr))
    }
}

/// Greedy non-maximum suppression, highest score first.
fn nms(mut faces: Vec<Face>, thr: f32) -> Vec<Face> {
    faces.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep: Vec<Face> = Vec::new();
    for f in faces {
        if keep.iter().all(|k| k.iou(&f) <= thr) {
            keep.push(f);
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, s: f32, score: f32) -> Face {
        Face {
            x,
            y,
            w: s,
            h: s,
            score,
            landmarks: [(0.0, 0.0); 5],
        }
    }

    #[test]
    fn nms_drops_overlapping_boxes() {
        let out = nms(
            vec![face(0.0, 0.0, 10.0, 0.9), face(1.0, 1.0, 10.0, 0.8)],
            0.3,
        );
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].score - 0.9).abs() < 1e-6,
            "must keep the higher score"
        );
    }

    #[test]
    fn nms_keeps_separate_faces() {
        let out = nms(
            vec![face(0.0, 0.0, 10.0, 0.9), face(100.0, 100.0, 10.0, 0.8)],
            0.3,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn iou_is_symmetric_and_bounded() {
        let (a, b) = (face(0.0, 0.0, 10.0, 1.0), face(5.0, 5.0, 10.0, 1.0));
        assert!((a.iou(&b) - b.iou(&a)).abs() < 1e-6);
        assert!(a.iou(&b) > 0.0 && a.iou(&b) < 1.0);
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
    }
}
