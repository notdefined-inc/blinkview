//! End-to-end: scan -> rename -> undo, plus the failure modes that motivated the design.

use openfoto_core::{journal::Journal, rename, scan, Library};
use std::path::{Path, PathBuf};

/// Minimal library of files with camera-style names. No EXIF, so capture time comes
/// from the filename — which keeps these tests independent of image fixtures.
fn fixture(name: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openfoto-it-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    for f in files {
        let p = dir.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, f.as_bytes()).unwrap(); // distinct bytes => distinct hashes
    }
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = walk(dir)
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(|s| s.to_string()))
        .collect();
    v.sort();
    v
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "jpg") {
            out.push(e.path().to_path_buf());
        }
    }
    out
}

#[test]
fn scan_rename_undo_round_trips() {
    let dir = fixture("roundtrip", &["20260816_151256.jpg", "Me/20260818_170334.jpg"]);
    let before = names(&dir);

    let mut lib = Library::open(&dir).unwrap();
    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.seen, 2);
    assert_eq!(st.hashed, 2);
    assert!(st.errors.is_empty());

    let plan = rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap();
    assert_eq!(plan.len(), 2);
    let j = plan.apply(&mut lib).unwrap();

    let after = names(&dir);
    assert!(after.iter().any(|n| n.starts_with("03-12-56_pm_16_aug_2026")));
    assert_ne!(before, after);

    // Re-running plans nothing: the operation is idempotent.
    assert!(rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap().is_empty());

    j.undo(&mut lib).unwrap();
    assert_eq!(names(&dir), before);
    std::fs::remove_dir_all(&dir).ok();
}

/// The year must never be consumed as a burst counter (see ADR-0003).
#[test]
fn rename_preserves_the_year() {
    let dir = fixture("year", &["20260819_131351.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let plan = rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap();
    plan.apply(&mut lib).unwrap();
    assert!(names(&dir).iter().all(|n| n.contains("_2026")), "{:?}", names(&dir));
    std::fs::remove_dir_all(&dir).ok();
}

/// Same-second bursts get counters, and every name is unique library-wide even
/// across folders — the collision class that affected 130 real files.
#[test]
fn names_are_unique_across_folders() {
    let dir = fixture(
        "unique",
        &["A/20260816_151256.jpg", "B/20260816_151256.jpg", "C/20260816_151256.jpg"],
    );
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let plan = rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap();
    plan.apply(&mut lib).unwrap();
    let n = names(&dir);
    let uniq: std::collections::HashSet<_> = n.iter().collect();
    assert_eq!(n.len(), uniq.len(), "duplicate names across folders: {n:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A folder renamed outside the tool must be re-identified by content hash, not
/// treated as deletion + new files. This is the bug that broke the manual session.
#[test]
fn survives_a_folder_renamed_externally() {
    let dir = fixture("extmove", &["Person1/20260816_151256.jpg", "Person1/20260816_151257.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();

    std::fs::rename(dir.join("Person1"), dir.join("Nikhil")).unwrap();

    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.moved, 2, "files should be re-identified by hash");
    assert_eq!(st.removed, 0, "nothing should be considered deleted");
    assert_eq!(lib.index.count().unwrap(), 2);
    assert!(lib.index.all().unwrap().iter().all(|r| r.path.starts_with("Nikhil/")));
    std::fs::remove_dir_all(&dir).ok();
}

/// A plan whose destination directory disappears must abort without touching disk.
#[test]
fn aborts_when_destination_is_missing() {
    let dir = fixture("missingdst", &["a/20260816_151256.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();

    let mut plan = openfoto_core::Plan::new("move");
    let row = &lib.index.all().unwrap()[0];
    plan.ops.push(openfoto_core::Op::Move {
        hash: row.hash.clone(),
        from: row.path.clone(),
        to: "gone/x.jpg".into(),
    });
    let err = plan.apply(&mut lib).unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
    assert!(dir.join("a/20260816_151256.jpg").exists(), "source must be untouched");
    std::fs::remove_dir_all(&dir).ok();
}

/// `.openfoto/` is disposable: deleting it and rescanning reproduces the index.
#[test]
fn vault_is_disposable() {
    let dir = fixture("disposable", &["20260816_151256.jpg", "Me/20260818_170334.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before: Vec<_> = lib.index.all().unwrap().iter().map(|r| (r.hash.clone(), r.path.clone())).collect();
    drop(lib);

    std::fs::remove_dir_all(dir.join(".openfoto")).unwrap();
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let after: Vec<_> = lib.index.all().unwrap().iter().map(|r| (r.hash.clone(), r.path.clone())).collect();
    assert_eq!(before, after);
    assert!(Journal::list(&lib).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// Editing must never destroy the original when asked to keep it, and the kept copy
/// must be a real file in a visible folder — not a cache entry.
#[test]
fn editing_keeps_the_original_in_a_visible_folder() {
    use openfoto_core::edit::{Adjust, Crop, Edit, Rotate, ORIGINALS};

    let dir = std::env::temp_dir().join(format!("openfoto-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 60x20 landscape, so a quarter turn is unmistakable.
    image::RgbImage::from_fn(60, 20, |x, _| image::Rgb([(x * 4) as u8, 10, 200]))
        .save(dir.join("20260101_120000.jpg"))
        .unwrap();

    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before = std::fs::read(dir.join("20260101_120000.jpg")).unwrap();

    let e = Edit {
        rotate: Rotate::Cw90,
        straighten: 0.0,
        adjust: Adjust { brightness: 0.2, ..Default::default() },
        flip_h: false,
        flip_v: false,
        crop: Some(Crop { x: 0.0, y: 0.0, w: 1.0, h: 0.5 }),
        keep_original: true,
    };
    let out = openfoto_core::edit::apply(&lib, "20260101_120000.jpg", &e).unwrap();

    // Rotated to 20x60, then cropped to the top half.
    assert_eq!((out.width, out.height), (20, 30), "rotate, then crop");

    // The edited file replaced the original in place...
    let after = std::fs::read(dir.join("20260101_120000.jpg")).unwrap();
    assert_ne!(after, before, "the photo should have been rewritten");

    // ...and the untouched original is a real file in a visible folder.
    let kept = out.original.expect("original path reported");
    assert!(kept.starts_with(ORIGINALS), "kept in {ORIGINALS}/, got {kept}");
    let kept_bytes = std::fs::read(dir.join(&kept)).unwrap();
    assert_eq!(kept_bytes, before, "the kept original must be byte-identical");
    assert!(!kept.contains(".openfoto"), "must not hide the original in the disposable vault");

    std::fs::remove_dir_all(&dir).ok();
}

/// A destructive save is honoured when explicitly asked for.
#[test]
fn destructive_editing_keeps_nothing() {
    use openfoto_core::edit::{Adjust, Edit, Rotate, ORIGINALS};

    let dir = std::env::temp_dir().join(format!("openfoto-edit2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_pixel(30, 10, image::Rgb([9, 9, 9]))
        .save(dir.join("20260101_130000.jpg"))
        .unwrap();

    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let e = Edit {
        rotate: Rotate::Cw180,
        straighten: 0.0,
        adjust: Adjust::default(),
        flip_h: false,
        flip_v: false,
        crop: None,
        keep_original: false,
    };
    let out = openfoto_core::edit::apply(&lib, "20260101_130000.jpg", &e).unwrap();
    assert!(out.original.is_none());
    assert!(!dir.join(ORIGINALS).exists(), "nothing should be kept");
    std::fs::remove_dir_all(&dir).ok();
}
