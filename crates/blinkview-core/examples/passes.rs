//! Three passes against one: does sharing the decode actually pay (ADR-0013)?
//!
//!     cargo run --release -p blinkview-core --example passes -- <source-library> [n]
//!
//! Copies photographs into a scratch library, so the source is only read.
use anyhow::Result;
use blinkview_core::{analyze, semantic, Library};
use std::path::PathBuf;
use std::time::Instant;

fn build(src: &PathBuf, n: usize, name: &str) -> Result<PathBuf> {
    let lib = Library::open(src)?;
    let photos: Vec<PathBuf> = lib
        .index
        .all()?
        .iter()
        .filter(|r| r.kind == "photo")
        .map(|r| lib.abs(&r.path))
        .filter(|p| p.exists())
        .collect();
    let step = (photos.len() / n.max(1)).max(1);
    let dir = std::env::temp_dir().join(format!("blinkview-passes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    for (i, p) in photos.iter().step_by(step).take(n).enumerate() {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        std::fs::copy(p, dir.join(format!("{i:04}.{ext}")))?;
    }
    Ok(dir)
}

fn main() -> Result<()> {
    let src = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: passes <library> [n]"),
    );
    let n: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);

    let a = build(&src, n, "separate")?;
    let mut lib = Library::open(&a)?;
    blinkview_core::scan::scan(&mut lib, false)?;
    let t = Instant::now();
    blinkview_core::thumbs::build(&lib)?;
    let t_thumbs = t.elapsed().as_secs_f64();
    let t = Instant::now();
    blinkview_core::faces::pipeline::analyze(&lib, blinkview_core::faces::pipeline::DEFAULT_SCORE)?;
    let t_faces = t.elapsed().as_secs_f64();
    let t = Instant::now();
    semantic::analyze(&lib, &blinkview_core::progress::silent)?;
    let t_clip = t.elapsed().as_secs_f64();
    let separate = t_thumbs + t_faces + t_clip;
    drop(lib);

    let b = build(&src, n, "combined")?;
    let mut lib2 = Library::open(&b)?;
    blinkview_core::scan::scan(&mut lib2, false)?;
    let t = Instant::now();
    let st = analyze::run(&mut lib2, analyze::Stages::default())?;
    let combined = t.elapsed().as_secs_f64();

    let per = |s: f64| s * 1000.0 / n as f64;
    println!("{n} photographs\n");
    println!("separate passes");
    println!("  thumbnails      {:>7.1} ms/photo", per(t_thumbs));
    println!("  faces           {:>7.1} ms/photo", per(t_faces));
    println!("  embeddings      {:>7.1} ms/photo", per(t_clip));
    println!("  total           {:>7.1} ms/photo", per(separate));
    println!(
        "\ncombined pass     {:>7.1} ms/photo   ({} decodes for {} photos)",
        per(combined),
        st.decoded,
        st.considered
    );
    println!(
        "\nspeedup           {:>7.2}x",
        separate / combined.max(0.0001)
    );
    for (label, secs) in [("separate", separate), ("combined", combined)] {
        println!(
            "  {label:<9} 200,000 photographs -> {:>5.1} h",
            secs / n as f64 * 200_000.0 / 3600.0
        );
    }
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    Ok(())
}
