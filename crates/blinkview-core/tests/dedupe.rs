//! End-to-end duplicate detection on generated images.
//!
//! Uses synthetic photos so the suite runs without the reference drive. The
//! properties asserted here are the ones that failed during the manual session.

use blinkview_core::{dedupe, scan, Library};
use image::{ImageBuffer, Rgb};
use std::path::{Path, PathBuf};

/// A deterministic "scene": smooth gradients plus scene-specific structure.
///
/// `soft` models motion blur the way a real burst produces it — the same composition
/// with *reduced* high-frequency energy, not different content. Removing detail
/// entirely would push the frame past the RMSE threshold and stop it grouping, which
/// is not what a soft frame in a real burst looks like.
fn scene(seed: u32, jitter: u32, soft: bool) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let (w, h) = (320, 240);
    ImageBuffer::from_fn(w, h, |x, y| {
        let sx = x + jitter;
        let base = ((sx * 7 + y * 3 + seed * 4099) / 5) % 200;
        // Calibrated with `cargo run --example diag`: soft-vs-sharp lands at RMSE
        // ~0.17 (inside the 0.30 threshold, so it still groups) while sharpness drops
        // 2543 -> 1723, which is what the keep-the-sharpest assertion needs.
        let amp = if soft { 35 } else { 45 };
        let detail = ((sx / 3 + y / 3) % 2) * amp;
        let v = (base + detail).min(255) as u8;
        Rgb([v, v.wrapping_add((seed * 31) as u8), 128])
    })
}

fn write(dir: &Path, name: &str, img: &ImageBuffer<Rgb<u8>, Vec<u8>>) {
    std::fs::create_dir_all(dir).unwrap();
    img.save(dir.join(name)).unwrap();
}

/// An isolated cache for a fixture library, beside rather than inside it.
///
/// `Library::open` would use the machine's cache root; a test suite that littered
/// `~/Library/Caches` would be a bug of its own. Beside the fixture keeps it out of
/// the library tree, where `scan` would index its thumbnails as photographs.
fn cache_for(dir: &std::path::Path) -> std::path::PathBuf {
    dir.parent().unwrap().join(format!(
        "{}-cache",
        dir.file_name().unwrap().to_string_lossy()
    ))
}

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("blinkview-dd-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A burst of near-identical frames groups together, and the sharpest one is kept.
#[test]
fn groups_a_burst_and_keeps_the_sharpest() {
    let d = fixture("burst");
    write(&d, "20260816_151201.jpg", &scene(1, 0, false));
    write(&d, "20260816_151202.jpg", &scene(1, 1, false));
    write(&d, "20260816_151203.jpg", &scene(1, 2, true)); // blurred member
    write(&d, "20260816_151204.jpg", &scene(9, 0, false)); // unrelated scene

    let mut lib = Library::open_in(&d, cache_for(&d)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    dedupe::ensure_signatures(&lib).unwrap();

    let groups = dedupe::find_groups(&lib, &dedupe::Options::default()).unwrap();
    assert_eq!(groups.len(), 1, "expected one burst group, got {groups:?}",);
    let g = &groups[0];
    assert_eq!(
        g.duplicates.len() + 1,
        3,
        "the unrelated scene must not join"
    );
    assert!(
        !g.keep.path.contains("151203"),
        "kept the blurred frame: {}",
        g.keep.path
    );
    std::fs::remove_dir_all(&d).ok();
}

/// Distinct scenes must never be grouped, however many there are.
#[test]
fn leaves_distinct_scenes_alone() {
    let d = fixture("distinct");
    for i in 0..6u32 {
        write(
            &d,
            &format!("2026081{}_15120{}.jpg", i % 9, i),
            &scene(i * 13 + 1, 0, false),
        );
    }
    let mut lib = Library::open_in(&d, cache_for(&d)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    dedupe::ensure_signatures(&lib).unwrap();
    let groups = dedupe::find_groups(&lib, &dedupe::Options::default()).unwrap();
    assert!(
        groups.is_empty(),
        "unrelated scenes were grouped: {groups:?}"
    );
    std::fs::remove_dir_all(&d).ok();
}

/// Signatures are cached by content hash, so they survive a rename for free —
/// the second analysis pass must find nothing to do.
#[test]
fn signature_cache_survives_a_rename() {
    let d = fixture("cache");
    write(&d, "20260816_151201.jpg", &scene(3, 0, false));
    let mut lib = Library::open_in(&d, cache_for(&d)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(dedupe::ensure_signatures(&lib).unwrap(), 1);

    std::fs::rename(d.join("20260816_151201.jpg"), d.join("renamed.jpg")).unwrap();
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        dedupe::ensure_signatures(&lib).unwrap(),
        0,
        "a renamed file should reuse its cached signature"
    );
    std::fs::remove_dir_all(&d).ok();
}

/// The dedupe plan must be reversible like any other mutation.
#[test]
fn dedupe_is_undoable() {
    let d = fixture("undo");
    write(&d, "20260816_151201.jpg", &scene(2, 0, false));
    write(&d, "20260816_151202.jpg", &scene(2, 1, false));
    std::fs::create_dir_all(d.join("Duplicates")).unwrap();

    let mut lib = Library::open_in(&d, cache_for(&d)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    dedupe::ensure_signatures(&lib).unwrap();
    let plan = dedupe::plan(&lib, &dedupe::Options::default()).unwrap();
    assert_eq!(plan.len(), 1);
    let j = plan.apply(&mut lib).unwrap();
    assert_eq!(std::fs::read_dir(d.join("Duplicates")).unwrap().count(), 1);

    j.undo(&mut lib).unwrap();
    assert_eq!(std::fs::read_dir(d.join("Duplicates")).unwrap().count(), 0);
    std::fs::remove_dir_all(&d).ok();
}
