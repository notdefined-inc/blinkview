//! Where the time actually goes, measured on a real library.
//!
//! Read-only: thumbnails are written to a temp directory, never into the library.
//!
//!     cargo run --release -p openfoto-core --example bench -- <library> [sample]

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Time `f` over each path, returning (mean ms, total).
fn timed<T>(label: &str, paths: &[PathBuf], mut f: impl FnMut(&Path) -> Result<T>) -> f64 {
    let mut ok = 0usize;
    let t = Instant::now();
    for p in paths {
        if f(p).is_ok() {
            ok += 1;
        }
    }
    let per = ms(t.elapsed()) / ok.max(1) as f64;
    println!("{label:<34} {per:>8.1} ms/photo   ({ok}/{} ok)", paths.len());
    per
}

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).expect("usage: bench <library> [n]"));
    let n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(40);

    let lib = openfoto_core::Library::open(&root)?;
    let rows = lib.index.all()?;
    let photos: Vec<PathBuf> = rows
        .iter()
        .filter(|r| r.kind == "photo")
        .map(|r| lib.abs(&r.path))
        .filter(|p| p.exists())
        .collect();
    println!("library {} — {} photos indexed\n", root.display(), photos.len());
    if photos.is_empty() {
        return Ok(());
    }

    // Spread the sample across the library rather than taking the first N, which on a
    // phone backup would all be the same day and the same camera mode.
    let step = (photos.len() / n.max(1)).max(1);
    let sample: Vec<PathBuf> = photos.iter().step_by(step).take(n).cloned().collect();

    let mp: f64 = sample
        .iter()
        .filter_map(|p| imagesize(p))
        .map(|(w, h)| (w * h) as f64 / 1e6)
        .sum::<f64>()
        / sample.len() as f64;
    println!("sample of {} photos, mean {:.1} MP\n", sample.len(), mp);

    let tmp = std::env::temp_dir().join(format!("openfoto-bench-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;

    println!("--- raw I/O ---");
    let mut bytes = 0u64;
    let t = Instant::now();
    for p in &sample {
        bytes += std::fs::read(p).map(|b| b.len() as u64).unwrap_or(0);
    }
    let secs = t.elapsed().as_secs_f64();
    println!("{:<34} {:>8.1} ms/photo   ({:.1} MB/s, mean {:.1} MB)",
        "read file bytes", secs * 1000.0 / sample.len() as f64,
        bytes as f64 / 1e6 / secs, bytes as f64 / 1e6 / sample.len() as f64);

    // Again, now that the OS cache is warm: separates disk from decode.
    let t = Instant::now();
    for p in &sample { let _ = std::fs::read(p); }
    println!("{:<34} {:>8.1} ms/photo   (cache warm)",
        "read again", t.elapsed().as_secs_f64() * 1000.0 / sample.len() as f64);

    println!("\n--- decode alone ---");
    timed("full decode", &sample, openfoto_core::imageio::load_rgb);
    let preview = timed("embedded preview (no full decode)", &sample, |p| {
        let b = std::fs::read(p)?;
        openfoto_core::imageio::embedded_preview(&b, 512)
            .ok_or_else(|| anyhow::anyhow!("no usable preview"))
    });

    println!("\n--- thumbnails (one core) ---");
    let full = timed("full decode + resize + encode", &sample, |p| {
        let dst = tmp.join("t.jpg");
        openfoto_core::thumbs::render_to(p, &dst, false)
    });
    let _ = preview;

    println!("\n--- face detection ---");
    if let Ok(model) = openfoto_core::faces::models::find("yunet.onnx") {
        let mut det = openfoto_core::faces::detect::Detector::load(&model)?;
        // What the pipeline really does: decode, shrink to ANALYSIS_LONG_EDGE, detect.
        let long = openfoto_core::faces::pipeline::ANALYSIS_LONG_EDGE;
        let shrink = |img: image::RgbImage| {
            let (w, h) = (img.width(), img.height());
            if w.max(h) <= long { return img; }
            let s = long as f32 / w.max(h) as f32;
            image::imageops::resize(&img, (w as f32*s) as u32, (h as f32*s) as u32,
                                    image::imageops::FilterType::Triangle)
        };
        timed("decode + shrink + detect (real)", &sample, |p| {
            let img = shrink(openfoto_core::imageio::load_rgb(p)?);
            let (w, h) = (img.width() as usize, img.height() as usize);
            det.detect(img.as_raw(), w, h, 0.6, 0.3).map(|v| v.len())
        });
        // Inference alone, on an image already the right size.
        let ready: Vec<image::RgbImage> = sample.iter()
            .filter_map(|p| openfoto_core::imageio::load_rgb(p).ok())
            .map(&shrink).collect();
        let t = Instant::now();
        for img in &ready {
            let _ = det.detect(img.as_raw(), img.width() as usize, img.height() as usize, 0.6, 0.3);
        }
        println!("{:<34} {:>8.1} ms/photo   (inference only)", "detect at 1280px",
            ms(t.elapsed()) / ready.len().max(1) as f64);
    } else {
        println!("(models not installed)");
    }

    println!("\n--- semantic embedding ---");
    if openfoto_core::semantic::Encoder::available() {
        let mut enc = openfoto_core::semantic::Encoder::load()?;
        timed("decode + embed (real)", &sample, |p| enc.embed_image(p));
    } else {
        println!("(models not installed)");
    }

    println!("\n--- the query path (what a source switch costs) ---");
    let t = Instant::now();
    let rows = lib.index.all()?;
    let all_ms = ms(t.elapsed());
    println!("{:<34} {:>8.1} ms   ({} rows)", "index.all()", all_ms, rows.len());

    let t = Instant::now();
    let people = openfoto_core::faces::people::People::load(lib.root())?;
    println!("{:<34} {:>8.1} ms   ({} people)", "People::load", ms(t.elapsed()), people.people.len());

    let t = Instant::now();
    let faces = lib.all_faces()?;
    let faces_ms = ms(t.elapsed());
    println!("{:<34} {:>8.1} ms   ({} faces)", "all_faces()", faces_ms, faces.len());

    let t = Instant::now();
    let opt = openfoto_core::faces::assign::Options::default();
    let mut hits = 0usize;
    for f in &faces {
        if let Some(e) = f.embedding.as_ref() {
            if openfoto_core::faces::assign::assign(e, &people, &opt).person().is_some() {
                hits += 1;
            }
        }
    }
    let assign_ms = ms(t.elapsed());
    println!("{:<34} {:>8.1} ms   ({hits} assigned)", "assign every face", assign_ms);

    let t = Instant::now();
    let ud = openfoto_core::userdata::UserDataSet::load(lib.root())?;
    println!("{:<34} {:>8.1} ms", "UserDataSet::load", ms(t.elapsed()));
    let _ = ud;

    let total = all_ms + faces_ms + assign_ms;
    println!("{:<34} {:>8.1} ms  -> {:.1} s at 200k photos",
        "sum", total, total / rows.len().max(1) as f64 * 200_000.0 / 1000.0);

    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    println!("\n--- projection for 200,000 photos on {cores} cores ---");
    println!("thumbnails  {:>8.1} min", full * 200_000.0 / 1000.0 / 60.0 / cores as f64);

    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn imagesize(p: &Path) -> Option<(u32, u32)> {
    let f = std::fs::File::open(p).ok()?;
    let mut d = jpeg_decoder::Decoder::new(std::io::BufReader::new(f));
    d.read_info().ok()?;
    let i = d.info()?;
    Some((i.width as u32, i.height as u32))
}
