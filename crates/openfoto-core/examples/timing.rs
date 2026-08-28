fn main() -> anyhow::Result<()> {
    let t = std::time::Instant::now();
    let mut e = openfoto_core::semantic::Encoder::load()?;
    println!("Encoder::load (vision+text)  {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _ = e.embed_text("a church")?;
    println!("first embed_text             {:?}", t.elapsed());
    let t = std::time::Instant::now();
    for _ in 0..10 { let _ = e.embed_text("a church")?; }
    println!("embed_text x10               {:?}", t.elapsed());
    Ok(())
}
