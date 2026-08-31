//! What a camera-RAW file gives up without being developed.
//!
//! Point it at a folder of RAW files: `cargo run --example rawprobe -p blinkview-core -- ~/RAW`.
//! Prints the preview each container declares and writes the thumbnails beside it, which
//! is the only way to see that a preview is the photograph and not the sensor data.

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: rawprobe <folder>");
    let out = std::path::Path::new(&dir).join("thumbs-out");
    std::fs::create_dir_all(&out)?;
    let mut files: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| blinkview_core::raw::is_raw(p))
        .collect();
    files.sort();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let size = std::fs::metadata(&p)?.len();
        let t = std::time::Instant::now();
        let found = blinkview_core::raw::preview(&p);
        let read = t.elapsed();
        let mtime = std::fs::metadata(&p)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (when, src) = blinkview_core::timesource::resolve(&p, mtime);
        println!(
            "{:12} taken {} via {src:?}",
            "",
            when.format("%Y-%m-%d %H:%M:%S")
        );
        match &found {
            Some(pv) => println!(
                "{name:12} {:5.1} MB -> {}x{} preview, {:4.0} KB read in {:>8.2?}",
                size as f64 / 1e6,
                pv.width,
                pv.height,
                pv.jpeg.len() as f64 / 1e3,
                read
            ),
            None => println!(
                "{name:12} {:5.1} MB -> no preview declared",
                size as f64 / 1e6
            ),
        }
        let t = std::time::Instant::now();
        let dst = out.join(format!("{name}.jpg"));
        match blinkview_core::thumbs::render_to(&p, &dst, false) {
            Ok(()) => println!(
                "{:12} thumbnail in {:>8.2?} -> {}",
                "",
                t.elapsed(),
                dst.display()
            ),
            Err(e) => println!("{:12} thumbnail failed: {e}", ""),
        }
    }
    Ok(())
}
