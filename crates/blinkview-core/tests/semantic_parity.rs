//! Text-embedding parity with the Python reference — the ADR-0004 rule.
//!
//! The relevance threshold in ADR-0008 is a single number measured against embeddings
//! from onnxruntime in Python. If the Rust path produces different vectors the number
//! stops meaning anything, and the failure is invisible: results stay plausible while
//! being quietly wrong. That is exactly how the SFace channel-order bug hid.
//!
//! This test is why the text tower is fp32 rather than the 4x smaller int8 build. The
//! int8 graph quantises activations per input at runtime, so two onnxruntime builds
//! disagree by an input-dependent amount — measured at up to cosine 0.989 between
//! ort 1.22 and Python 1.23, enough to move photos across the threshold. fp32 is
//! bit-identical across both.
//!
//! Skipped when the models are absent; they are large and not committed.

use blinkview_core::semantic;

fn fixture() -> serde_json::Value {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/clip_text_reference.json"
    ))
    .expect("reference fixture");
    serde_json::from_str(&raw).expect("valid fixture json")
}

/// The reference is only meaningful for the model it was generated from, so a model
/// swap must fail loudly here rather than silently compare against a stale vector.
#[test]
fn fixture_matches_the_pinned_model() {
    let doc = fixture();
    let want = doc["model_sha256"].as_str().expect("model_sha256");
    let spec = blinkview_core::faces::fetch::specs()
        .into_iter()
        .find(|s| s.name == semantic::TEXT)
        .expect("text model spec");
    assert_eq!(
        spec.sha256, want,
        "the text model changed but tests/fixtures/clip_text_reference.json was not \
         regenerated — the embeddings below are from a different model"
    );
}

#[test]
fn text_embeddings_match_the_python_reference() {
    if !semantic::Encoder::available() {
        eprintln!("skipping: CLIP models not installed (run `blinkview models fetch`)");
        return;
    }
    let doc = fixture();
    let mut enc = semantic::Encoder::load().expect("load encoders");

    for (query, want) in doc["embeddings"].as_object().unwrap() {
        let expected: Vec<f32> = want
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let got = enc.embed_text(query).expect("embed");

        assert_eq!(got.len(), semantic::DIM, "{query:?} wrong dimensionality");
        let norm = got.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "{query:?} not unit length: {norm}");

        let cos = semantic::similarity(&expected, &got);
        assert!(
            cos > 0.999,
            "{query:?} diverges from the reference (cosine {cos:.6}). The threshold in \
             ADR-0008 was measured against the reference, so a mismatch here silently \
             invalidates it. A quantised text model is the usual cause."
        );
    }
}

/// Guards against a degenerate encoder that returns the same vector for everything —
/// which would pass the parity test only if the reference were degenerate too.
#[test]
fn different_phrases_embed_differently() {
    if !semantic::Encoder::available() {
        return;
    }
    let mut enc = semantic::Encoder::load().unwrap();
    let a = enc.embed_text("a photograph of the night sky").unwrap();
    let b = enc.embed_text("a plate of food on a wooden table").unwrap();
    let sim = semantic::similarity(&a, &b);
    assert!(sim < 0.9, "unrelated phrases should not be near-identical: {sim:.3}");
}
