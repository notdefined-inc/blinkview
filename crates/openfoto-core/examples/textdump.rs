// Dumps Rust-path text embeddings as JSON so the parity harness can compare them
// against the Python reference over the real index.
fn main() -> anyhow::Result<()> {
    let mut enc = openfoto_core::semantic::Encoder::load()?;
    let mut out = serde_json::Map::new();
    for q in std::env::args().skip(1) {
        let v = enc.embed_text(&q)?;
        out.insert(q, serde_json::json!(v));
    }
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
