//! Diagnostic: generate the synthetic burst fixture and print its pairwise distances,
//! so test thresholds are calibrated against measurements rather than guesses.
use image::{ImageBuffer, Rgb};
use openfoto_core::imagesig;

fn scene(seed: u32, jitter: u32, amp: u32) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    ImageBuffer::from_fn(320, 240, |x, y| {
        let sx = x + jitter;
        let base = ((sx * 7 + y * 3 + seed * 4099) / 5) % 200;
        let v = (base + ((sx / 3 + y / 3) % 2) * amp).min(255) as u8;
        Rgb([v, v.wrapping_add((seed * 31) as u8), 128])
    })
}

fn main() -> anyhow::Result<()> {
    let d = std::path::Path::new("/tmp/of-fix");
    std::fs::create_dir_all(d)?;
    scene(1, 0, 45).save(d.join("a_sharp.jpg"))?;
    scene(1, 1, 45).save(d.join("b_sharp.jpg"))?;
    scene(1, 2, 35).save(d.join("c_soft.jpg"))?;
    scene(9, 0, 45).save(d.join("d_other.jpg"))?;

    let mut files: Vec<_> = std::fs::read_dir(d)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    files.sort();
    let sigs: Vec<_> = files.iter().map(|p| imagesig::compute(p).unwrap()).collect();
    for (i, f) in files.iter().enumerate() {
        println!("{:<14} sharpness={:>9.1}", f.file_stem().unwrap().to_string_lossy(), sigs[i].sharpness);
    }
    println!("\n{:<10} {:<10} {:>8} {:>8}", "a", "b", "hamming", "rmse");
    for i in 0..files.len() {
        for j in i + 1..files.len() {
            println!("{:<10} {:<10} {:>8} {:>8.3}",
                files[i].file_stem().unwrap().to_string_lossy(),
                files[j].file_stem().unwrap().to_string_lossy(),
                imagesig::hamming(sigs[i].dhash, sigs[j].dhash),
                imagesig::rmse(&sigs[i].thumb, &sigs[j].thumb));
        }
    }
    Ok(())
}
