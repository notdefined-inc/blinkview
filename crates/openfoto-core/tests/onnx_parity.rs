//! Parity with OpenCV's reference implementation.
//!
//! These guard a bug that is invisible without a numerical check: feeding SFace BGR
//! instead of RGB still produces plausible embeddings that cluster roughly correctly,
//! but they sit at ~0.91 cosine against the reference rather than ~1.0 — which would
//! quietly invalidate every similarity threshold in ADR-0003.
//!
//! Skipped when the models are absent, since they are 37MB and not committed.

use openfoto_core::faces::{embed, models};

fn model_or_skip() -> Option<std::path::PathBuf> {
    match models::find(models::SFACE) {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("skipping: sface.onnx not found (run `openfoto models fetch`)");
            None
        }
    }
}

#[test]
fn sface_embedding_matches_opencv() {
    let Some(model) = model_or_skip() else { return };
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/sface_input.png");
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sface_expected.json"
        ))
        .expect("fixture json"),
    )
    .unwrap();
    let want: Vec<f32> = expected["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();

    let img = image::ImageReader::open(fixture).unwrap().decode().unwrap().to_rgb8();
    let mut e = embed::Embedder::load(&model).unwrap();
    let got = e.embed(img.as_raw()).unwrap();

    assert_eq!(got.len(), embed::DIM);
    let norm = got.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "embedding must be L2-normalized, got {norm}");

    let cos = embed::cosine(&want, &got);
    assert!(
        cos > 0.999,
        "SFace embedding diverges from OpenCV (cosine {cos:.5}). \
         A value near 0.91 means the RGB/BGR channel order is swapped."
    );
}
