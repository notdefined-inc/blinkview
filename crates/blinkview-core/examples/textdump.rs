//! Dumps Rust-path text embeddings as JSON.
//!
//! Kept because it is how the fp32 text decision was made: it lets a query's embedding
//! be compared against the Python reference *and* scored across a real index, which is
//! what showed that the quantised encoder moved photos across the threshold rather than
//! merely perturbing them. `tests/semantic_parity.rs` covers the routine check; this is
//! for when that test fails and the question is how much the difference matters.
//!
//!     cargo run -p blinkview-core --example textdump -- "a night sky" "a church"
fn main() -> anyhow::Result<()> {
    let mut enc = blinkview_core::semantic::Encoder::load()?;
    let mut out = serde_json::Map::new();
    for q in std::env::args().skip(1) {
        let v = enc.embed_text(&q)?;
        out.insert(q, serde_json::json!(v));
    }
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
