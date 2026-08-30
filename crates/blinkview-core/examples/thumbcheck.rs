//! Does the fast thumbnail path produce the same picture as the slow one?
//!
//! Orientation is the risk: the preview is stored unrotated, and the rotation now
//! happens after shrinking rather than before.
use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    let n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let lib = blinkview_core::Library::open(&root)?;
    let rows = lib.index.all()?;
    let photos: Vec<PathBuf> = rows.iter().filter(|r| r.kind == "photo")
        .map(|r| lib.abs(&r.path)).filter(|p| p.exists()).collect();
    let step = (photos.len() / n.max(1)).max(1);

    let tmp = std::env::temp_dir().join("blinkview-thumbcheck");
    std::fs::create_dir_all(&tmp)?;
    let (mut same, mut diff, mut preview, mut oriented) = (0, 0, 0, 0);
    let mut worst: Vec<(String, f64)> = vec![];

    for p in photos.iter().step_by(step).take(n) {
        let o = blinkview_core::imageio::orientation(p);
        if o != 1 { oriented += 1; }
        if std::fs::read(p).ok()
            .and_then(|b| blinkview_core::imageio::embedded_preview(&b, 512)).is_some() { preview += 1; }

        // Reference: decode everything, rotate, then shrink.
        let full = blinkview_core::imageio::load_rgb(p)?;
        let (w, h) = (full.width(), full.height());
        let s = 512.0 / w.max(h) as f32;
        let want = if s < 1.0 {
            image::imageops::resize(&full, (w as f32*s).round() as u32,
                (h as f32*s).round() as u32, image::imageops::FilterType::Triangle)
        } else { full };

        let dst = tmp.join("t.jpg");
        blinkview_core::thumbs::render_to(p, &dst, false)?;
        let got = image::open(&dst)?.to_rgb8();

        if got.dimensions() != want.dimensions() {
            diff += 1;
            worst.push((p.file_name().unwrap().to_string_lossy().into(), -1.0));
            continue;
        }
        // Mean absolute difference: the preview is a different encode, so exact equality
        // is not expected — a rotation mistake, however, is enormous.
        let d: f64 = got.pixels().zip(want.pixels())
            .map(|(a, b)| a.0.iter().zip(b.0.iter())
                .map(|(x, y)| (*x as i32 - *y as i32).abs() as f64).sum::<f64>())
            .sum::<f64>() / (got.width() * got.height() * 3) as f64;
        if d > 24.0 { diff += 1; worst.push((p.file_name().unwrap().to_string_lossy().into(), d)); }
        else { same += 1; }
    }
    println!("checked {}  |  embedded preview used: {preview}  |  EXIF-rotated: {oriented}", same + diff);
    println!("matches reference: {same}   suspicious: {diff}");
    for (f, d) in worst.iter().take(6) {
        println!("   {f}  mean abs diff {d:.1}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
