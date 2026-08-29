//! End-to-end embedding throughput, for comparison against other libraries'
//! published figures (Immich quotes whole-library wall clock).
use anyhow::Result;
use rayon::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    let n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(24);
    let lib = openfoto_core::Library::open(&root)?;
    let rows = lib.index.all()?;
    let photos: Vec<PathBuf> = rows.iter().filter(|r| r.kind == "photo")
        .map(|r| lib.abs(&r.path)).filter(|p| p.exists()).collect();
    let step = (photos.len() / n.max(1)).max(1);
    let sample: Vec<PathBuf> = photos.iter().step_by(step).take(n).cloned().collect();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    println!("{} photos, {cores} cores\n", sample.len());

    // What ships today: one encoder, one photo at a time.
    let mut enc = openfoto_core::semantic::Encoder::load()?;
    let t = Instant::now();
    for p in &sample { let _ = enc.embed_image(p); }
    let seq = t.elapsed().as_secs_f64();
    println!("sequential   {:>6.1} ms/photo   {:>5.1} photos/s", seq*1000.0/sample.len() as f64,
             sample.len() as f64 / seq);
    drop(enc);

    // One encoder per worker thread.
    let t = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(cores).build()?;
    pool.install(|| {
        sample.par_chunks(sample.len().div_ceil(cores)).for_each(|chunk| {
            if let Ok(mut e) = openfoto_core::semantic::Encoder::load() {
                for p in chunk { let _ = e.embed_image(p); }
            }
        });
    });
    let par = t.elapsed().as_secs_f64();
    println!("parallel     {:>6.1} ms/photo   {:>5.1} photos/s   ({:.1}x)",
             par*1000.0/sample.len() as f64, sample.len() as f64 / par, seq/par);

    for (label, secs) in [("sequential", seq), ("parallel", par)] {
        let per = secs / sample.len() as f64;
        println!("  {label:<11} 80,000 assets -> {:>5.0} min      200,000 -> {:>5.1} h",
                 per*80_000.0/60.0, per*200_000.0/3600.0);
    }
    Ok(())
}
