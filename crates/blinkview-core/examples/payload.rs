//! Where does a source switch spend its time? Reproduces the `photos` command's
//! payload so build and serialisation can be separated, in a release build.
use anyhow::Result;
use serde::Serialize;
use std::time::Instant;

#[derive(Serialize)]
struct PhotoInfo {
    kind: String,
    rating: u8,
    label: Option<String>,
    albums: Vec<String>,
    ext: String,
    bytes: u64,
    hash: String,
    path: String,
    name: String,
    folder: String,
    thumb: String,
    taken_at: Option<i64>,
    faces: usize,
    people: Vec<String>,
    width: u32,
    height: u32,
}

fn main() -> Result<()> {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let mut lib = blinkview_core::Library::open(&root)?;

    let t = Instant::now();
    let rows = lib.index.all()?;
    let rows_ms = t.elapsed().as_secs_f64() * 1000.0;

    // First touch walks every folder for an blinkview.json; after that it is held.
    let t = Instant::now();
    let _ = lib.user_data()?;
    let ud_cold = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let user = lib.user_data()?.clone();
    let ud_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let out: Vec<PhotoInfo> = rows
        .iter()
        .map(|r| {
            let folder = blinkview_core::plan::folder_of(&r.path).to_string();
            let name = r.path.rsplit('/').next().unwrap_or(&r.path).to_string();
            let meta = user.get(&r.hash, &folder);
            PhotoInfo {
                kind: r.kind.clone(),
                rating: meta.rating,
                label: meta.label.clone(),
                albums: meta.albums.clone(),
                ext: name.rsplit('.').next().unwrap_or("").to_uppercase(),
                bytes: r.size as u64,
                hash: r.hash.clone(),
                path: lib.abs(&r.path).display().to_string(),
                name,
                folder,
                thumb: blinkview_core::thumbs::thumb_path_at(&root, &r.hash)
                    .display()
                    .to_string(),
                taken_at: r.taken_at,
                faces: 0,
                people: vec![],
                width: 0,
                height: 0,
            }
        })
        .collect();
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let json = serde_json::to_string(&out)?;
    let ser_ms = t.elapsed().as_secs_f64() * 1000.0;

    let n = out.len();
    println!("{n} photographs\n");
    println!("index.all()          {rows_ms:>8.1} ms");
    println!("user_data first walk {ud_cold:>8.1} ms   (once per library)");
    println!("user_data cached+clone {ud_ms:>6.1} ms   (every query)");
    println!("build PhotoInfo      {build_ms:>8.1} ms");
    println!("serialise to JSON    {ser_ms:>8.1} ms");
    println!(
        "total rust           {:>8.1} ms",
        rows_ms + ud_ms + build_ms + ser_ms
    );
    println!(
        "\npayload              {:>8.1} MB   ({} bytes/photo)",
        json.len() as f64 / 1e6,
        json.len() / n.max(1)
    );
    let at200k = (rows_ms + ud_ms + build_ms + ser_ms) / n as f64 * 200_000.0;
    println!(
        "at 200,000           {:>8.1} s rust   {:>6.0} MB payload",
        at200k / 1000.0,
        json.len() as f64 / n as f64 * 200_000.0 / 1e6
    );
    Ok(())
}
