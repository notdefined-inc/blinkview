//! End-to-end: scan -> rename -> undo, plus the failure modes that motivated the design.

use blinkview_core::{journal::Journal, rename, scan, Library};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Minimal library of files with camera-style names. No EXIF, so capture time comes
/// from the filename — which keeps these tests independent of image fixtures.
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

fn fixture(name: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("blinkview-it-{}-{}", std::process::id(), name));
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
    let dir = fixture(
        "roundtrip",
        &["20260816_151256.jpg", "Me/20260818_170334.jpg"],
    );
    let before = names(&dir);

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.seen, 2);
    assert_eq!(st.hashed, 2);
    assert!(st.errors.is_empty());

    let plan = rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap();
    assert_eq!(plan.len(), 2);
    let j = plan.apply(&mut lib).unwrap();

    let after = names(&dir);
    assert!(after
        .iter()
        .any(|n| n.starts_with("03-12-56_pm_16_aug_2026")));
    assert_ne!(before, after);

    // Re-running plans nothing: the operation is idempotent.
    assert!(rename::plan(&lib, rename::DEFAULT_FORMAT)
        .unwrap()
        .is_empty());

    j.undo(&mut lib).unwrap();
    assert_eq!(names(&dir), before);
    std::fs::remove_dir_all(&dir).ok();
}

/// The year must never be consumed as a burst counter (see ADR-0003).
#[test]
fn rename_preserves_the_year() {
    let dir = fixture("year", &["20260819_131351.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let plan = rename::plan(&lib, rename::DEFAULT_FORMAT).unwrap();
    plan.apply(&mut lib).unwrap();
    assert!(
        names(&dir).iter().all(|n| n.contains("_2026")),
        "{:?}",
        names(&dir)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Same-second bursts get counters, and every name is unique library-wide even
/// across folders — the collision class that affected 130 real files.
#[test]
fn names_are_unique_across_folders() {
    let dir = fixture(
        "unique",
        &[
            "A/20260816_151256.jpg",
            "B/20260816_151256.jpg",
            "C/20260816_151256.jpg",
        ],
    );
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
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
    let dir = fixture(
        "extmove",
        &["Person1/20260816_151256.jpg", "Person1/20260816_151257.jpg"],
    );
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();

    std::fs::rename(dir.join("Person1"), dir.join("Alex")).unwrap();

    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.moved, 2, "files should be re-identified by hash");
    assert_eq!(st.removed, 0, "nothing should be considered deleted");
    assert_eq!(lib.index.count().unwrap(), 2);
    assert!(lib
        .index
        .all()
        .unwrap()
        .iter()
        .all(|r| r.path.starts_with("Alex/")));
    std::fs::remove_dir_all(&dir).ok();
}

/// A plan whose destination directory disappears must abort without touching disk.
#[test]
fn aborts_when_destination_is_missing() {
    let dir = fixture("missingdst", &["a/20260816_151256.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();

    let mut plan = blinkview_core::Plan::new("move");
    let row = &lib.index.all().unwrap()[0];
    plan.ops.push(blinkview_core::Op::Move {
        hash: row.hash.clone(),
        from: row.path.clone(),
        to: "gone/x.jpg".into(),
    });
    let err = plan.apply(&mut lib).unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
    assert!(
        dir.join("a/20260816_151256.jpg").exists(),
        "source must be untouched"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `.blinkview/` is disposable: deleting it and rescanning reproduces the index.
#[test]
fn vault_is_disposable() {
    let dir = fixture(
        "disposable",
        &["20260816_151256.jpg", "Me/20260818_170334.jpg"],
    );
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before: Vec<_> = lib
        .index
        .all()
        .unwrap()
        .iter()
        .map(|r| (r.hash.clone(), r.path.clone()))
        .collect();
    let vault = lib.vault().to_path_buf();
    drop(lib);

    std::fs::remove_dir_all(&vault).unwrap();
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let after: Vec<_> = lib
        .index
        .all()
        .unwrap()
        .iter()
        .map(|r| (r.hash.clone(), r.path.clone()))
        .collect();
    assert_eq!(before, after);
    assert!(Journal::list(&lib).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// Editing must never destroy the original when asked to keep it, and the kept copy
/// must be a real file in a visible folder — not a cache entry.
#[test]
fn editing_keeps_the_original_in_a_visible_folder() {
    use blinkview_core::edit::{Adjust, Crop, Edit, Rotate, ORIGINALS};

    let dir = std::env::temp_dir().join(format!("blinkview-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 60x20 landscape, so a quarter turn is unmistakable.
    image::RgbImage::from_fn(60, 20, |x, _| image::Rgb([(x * 4) as u8, 10, 200]))
        .save(dir.join("20260101_120000.jpg"))
        .unwrap();

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before = std::fs::read(dir.join("20260101_120000.jpg")).unwrap();

    let e = Edit {
        rotate: Rotate::Cw90,
        straighten: 0.0,
        adjust: Adjust {
            brightness: 0.2,
            ..Default::default()
        },
        flip_h: false,
        flip_v: false,
        crop: Some(Crop {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 0.5,
        }),
        keep_original: true,
    };
    let out = blinkview_core::edit::apply(&lib, "20260101_120000.jpg", &e).unwrap();

    // Rotated to 20x60, then cropped to the top half.
    assert_eq!((out.width, out.height), (20, 30), "rotate, then crop");

    // The edited file replaced the original in place...
    let after = std::fs::read(dir.join("20260101_120000.jpg")).unwrap();
    assert_ne!(after, before, "the photo should have been rewritten");

    // ...and the untouched original is a real file in a visible folder.
    let kept = out.original.expect("original path reported");
    assert!(
        kept.starts_with(ORIGINALS),
        "kept in {ORIGINALS}/, got {kept}"
    );
    let kept_bytes = std::fs::read(dir.join(&kept)).unwrap();
    assert_eq!(
        kept_bytes, before,
        "the kept original must be byte-identical"
    );
    assert!(
        !kept.contains(".blinkview"),
        "must not hide the original in the disposable vault"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A destructive save is honoured when explicitly asked for.
#[test]
fn destructive_editing_keeps_nothing() {
    use blinkview_core::edit::{Adjust, Edit, Rotate, ORIGINALS};

    let dir = std::env::temp_dir().join(format!("blinkview-edit2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_pixel(30, 10, image::Rgb([9, 9, 9]))
        .save(dir.join("20260101_130000.jpg"))
        .unwrap();

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
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
    let out = blinkview_core::edit::apply(&lib, "20260101_130000.jpg", &e).unwrap();
    assert!(out.original.is_none());
    assert!(!dir.join(ORIGINALS).exists(), "nothing should be kept");
    std::fs::remove_dir_all(&dir).ok();
}

/// The promise ADR-0001 makes, tested rather than asserted: deleting the cache must
/// lose nothing the user authored.
#[test]
fn deleting_the_cache_preserves_names_and_ratings() {
    use blinkview_core::faces::people::People;
    use blinkview_core::userdata::UserData;

    let dir = fixture("disposable-userdata", &["20260101_100000.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();

    let mut people = People::default();
    people.add_references("Alex", vec![vec![1.0, 0.0, 0.0]]);
    lib.save_people(&people).unwrap();

    let mut user = UserData::default();
    user.set_rating(&hash, 5);
    user.set_label(&hash, Some("red".into()));
    user.save(lib.root()).unwrap();
    let vault = lib.vault().to_path_buf();
    drop(lib);

    // The thing the documentation invites the user to do. Deleting it now also takes
    // the faces the names point at, which is exactly the loss the file must survive.
    std::fs::remove_dir_all(&vault).unwrap();

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        lib.people().unwrap().people[0].name,
        "Alex",
        "names must survive deleting the cache"
    );
    let back = UserData::load(lib.root()).unwrap();
    assert_eq!(
        back.get(&hash).rating,
        5,
        "ratings must survive deleting the cache"
    );
    assert_eq!(back.get(&hash).label.as_deref(), Some("red"));
    std::fs::remove_dir_all(&dir).ok();
}

/// A library written by an older version keeps its data when opened by this one.
#[test]
fn user_data_is_rescued_from_the_old_location() {
    let dir = fixture("rescue", &["20260101_110000.jpg"]);
    let vault = dir.join(".blinkview");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("people.json"),
        br#"{"people":[{"name":"Old","references":[[1.0]]}]}"#,
    )
    .unwrap();

    let lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    assert!(
        dir.join("blinkview-people.json").exists(),
        "moved to the root on open"
    );
    assert!(
        !vault.join("people.json").exists(),
        "no stale copy left behind"
    );
    assert_eq!(lib.people().unwrap().people[0].name, "Old");
    std::fs::remove_dir_all(&dir).ok();
}

/// Neither cache directory is a photo folder. A leftover `.openfoto/` — from an older
/// install, or a copy of the library that carried one along — is full of thumbnails,
/// and indexing it puts a folder of duplicates in the sidebar out of nowhere.
#[test]
fn neither_the_current_nor_the_former_cache_is_indexed() {
    let dir = fixture("caches", &["20260101_110000.jpg"]);
    // Open once, so the library is of the current era: a marker, a cache outside the
    // photographs, and therefore nothing in-folder for a relocation to claim.
    drop(Library::open_in(&dir, cache_for(&dir)).unwrap());
    // Then leave a vault of each name beside the photographs — an older version run
    // after the move, or a copy of a library that carried one along.
    for cache in [".blinkview", ".openfoto"] {
        std::fs::create_dir_all(dir.join(cache).join("thumbs")).unwrap();
        std::fs::write(dir.join(cache).join("thumbs/deadbeef.jpg"), b"thumbnail").unwrap();
    }

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.seen, 1, "only the photograph is a photograph");
    assert!(
        lib.index
            .all()
            .unwrap()
            .iter()
            .all(|r| !r.path.contains("thumbs")),
        "no cache file may reach the index"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_shallow_source_indexes_only_its_direct_files_and_can_change_depth() {
    let dir = fixture(
        "shallow",
        &["root.jpg", "Trip/inside.jpg", "Trip/Day2/deeper.jpg"],
    );
    let cache = cache_for(&dir);
    let mut lib = Library::open_in_configured(&dir, &cache, true, true).unwrap();

    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        lib.index
            .all()
            .unwrap()
            .into_iter()
            .map(|r| r.path)
            .collect::<Vec<_>>(),
        vec!["root.jpg"],
        "a shallow source must not index even one level beneath its root"
    );

    // User-authored data lives beside the photograph, not in the index, and changing
    // scan depth must never remove it.
    std::fs::write(
        dir.join("Trip/blinkview.json"),
        br#"{"photos":{"not-an-index-row":{"rating":5}}}"#,
    )
    .unwrap();
    lib.configure_scan(false, true);
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(lib.index.count().unwrap(), 3);
    lib.configure_scan(true, true);
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        lib.index.count().unwrap(),
        1,
        "deeper rows must leave the index"
    );
    assert!(
        dir.join("Trip/blinkview.json").is_file(),
        "changing depth must not touch ratings or labels"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&cache).ok();
}

#[test]
fn default_exclusions_apply_while_descending_but_never_to_the_chosen_root() {
    let dir = fixture(
        "skip-dirs",
        &["root.jpg", "Trip/inside.jpg", "Library/cached.jpg"],
    );
    let cache = cache_for(&dir);
    let mut lib = Library::open_in_configured(&dir, &cache, false, true).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let paths: Vec<_> = lib
        .index
        .all()
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, vec!["Trip/inside.jpg", "root.jpg"]);
    drop(lib);

    let library_root = dir.join("Library");
    let direct_cache = cache.with_extension("direct");
    let mut direct =
        Library::open_in_configured(&library_root, &direct_cache, false, true).unwrap();
    scan::scan(&mut direct, false).unwrap();
    assert_eq!(
        direct.index.count().unwrap(),
        1,
        "a directory named Library works when it is the folder explicitly chosen"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&cache).ok();
    std::fs::remove_dir_all(&direct_cache).ok();
}

#[test]
fn surveying_counts_media_without_descending_into_excluded_folders() {
    let dir = fixture(
        "survey",
        &[
            "root.jpg",
            "Trip/inside.jpg",
            "Trip/notes.txt",
            "Library/cached.jpg",
        ],
    );
    let survey = scan::survey_folder(&dir).unwrap();
    assert_eq!(survey.here, 1);
    assert_eq!(survey.below, Some(1));
    assert_eq!(survey.subfolders, 2);
    assert_eq!(survey.excluded, vec!["Library"]);

    // The same name is not excluded when it is the root the user chose.
    let direct = scan::survey_folder(dir.join("Library")).unwrap();
    assert_eq!(direct.here, 1);
    assert_eq!(direct.excluded, Vec::<String>::new());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_peek_is_shallow_markerless_and_deletes_its_cache_on_close() {
    let dir = fixture("peek", &["root.jpg", "Nested/hidden.jpg"]);
    let cache = cache_for(&dir);
    let mut lib = Library::peek_in(&dir, &cache).unwrap();
    let vault = lib.vault().to_path_buf();
    assert!(lib.is_peek());
    assert!(lib.is_shallow());
    assert!(vault.starts_with(cache.join("peek")));
    assert!(
        !dir.join(blinkview_core::cache::MARKER).exists(),
        "looking must not claim the folder"
    );

    scan::scan_shallow(&mut lib, false).unwrap();
    assert_eq!(
        lib.index
            .all()
            .unwrap()
            .into_iter()
            .map(|r| r.path)
            .collect::<Vec<_>>(),
        vec!["root.jpg"]
    );
    lib.end_peek().unwrap();
    assert!(!vault.exists(), "ending a peek removes every derived byte");
    assert!(!dir.join(blinkview_core::cache::MARKER).exists());

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&cache).ok();
}

/// Keeping a peeked folder is the full commitment made after the fact: the marker is
/// written, the recursive scan finds what the peek's depth limit hid, and the peek's
/// cache is already gone.
#[test]
fn promoting_a_peek_gives_a_full_recursive_library() {
    let dir = fixture("promote", &["root.jpg", "Nested/hidden.jpg"]);
    let cache = cache_for(&dir);

    let mut peek = Library::peek_in(&dir, &cache).unwrap();
    scan::scan_shallow(&mut peek, false).unwrap();
    assert_eq!(peek.index.all().unwrap().len(), 1, "a peek never descends");
    let peek_vault = peek.vault().to_path_buf();
    peek.end_peek().unwrap();
    assert!(!peek_vault.exists());

    let mut lib = Library::open_in(&dir, &cache).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let mut paths: Vec<String> = lib
        .index
        .all()
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["Nested/hidden.jpg", "root.jpg"]);
    assert!(
        dir.join(blinkview_core::cache::MARKER).exists(),
        "a kept folder is claimed like any added source"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&cache).ok();
}

/// ADR-0017: a library written before the rename opens with everything intact, and
/// keeps its index rather than paying for the new name with a full rescan.
#[test]
fn a_library_from_before_the_rename_is_adopted_whole() {
    use blinkview_core::userdata::UserData;

    let dir = fixture(
        "rename",
        &["20260101_110000.jpg", "Day1/20260102_120000.jpg"],
    );
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();
    let mut u = UserData::load(&dir).unwrap();
    u.set_rating(&hash, 5);
    u.save(&dir).unwrap();
    std::fs::write(
        dir.join("blinkview-people.json"),
        br#"{"people":[{"name":"Alex","references":[[1.0]]}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Day1/blinkview.json"),
        br#"{"albums":{},"photos":{}}"#,
    )
    .unwrap();
    let vault = lib.vault().to_path_buf();
    drop(lib);

    // Put the library back the way the previous name left it: the cache beside the
    // photographs, under the old name, with no marker claiming it.
    std::fs::remove_file(dir.join(blinkview_core::cache::MARKER)).unwrap();
    std::fs::rename(&vault, dir.join(".openfoto")).unwrap();
    for (from, to) in [
        ("blinkview.json", "openfoto.json"),
        ("blinkview-people.json", "openfoto-people.json"),
        ("Day1/blinkview.json", "Day1/openfoto.json"),
    ] {
        std::fs::rename(dir.join(from), dir.join(to)).unwrap();
    }

    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    assert!(
        !dir.join(".openfoto").exists(),
        "the old cache is adopted, not left behind"
    );
    // Adopted, then relocated: nothing of blinkview's stays beside the photographs
    // except the marker.
    assert!(!dir.join(".blinkview").exists());
    assert!(lib.vault().join("index.sqlite").is_file());
    assert!(dir.join(blinkview_core::cache::MARKER).is_file());
    assert_eq!(
        lib.index.all().unwrap().len(),
        2,
        "adopting the cache keeps the index — a rename must not cost a rescan"
    );
    assert_eq!(UserData::load(&dir).unwrap().get(&hash).rating, 5);
    assert_eq!(lib.people().unwrap().people[0].name, "Alex");
    // The cascade renames what it reads, so the second open sees one name only.
    lib.user_data().unwrap();
    assert!(dir.join("Day1/blinkview.json").exists());
    assert!(!dir.join("Day1/openfoto.json").exists());
    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-0019: the derived cache moves out of the photograph folder, and what it held
/// arrives intact — index, journal and all — rather than being rebuilt.
#[test]
fn the_cache_moves_out_of_the_photographs() {
    let dir = fixture(
        "adr19-move",
        &["20260101_090000.jpg", "Trip/20260102_100000.jpg"],
    );
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before: Vec<_> = lib
        .index
        .all()
        .unwrap()
        .iter()
        .map(|r| r.hash.clone())
        .collect();
    // Something only a moved cache could carry: an undo entry, which no rescan
    // reproduces (ADR-0019's amendment to ADR-0001 — the journal is not derivable).
    std::fs::create_dir_all(lib.journal_dir()).unwrap();
    std::fs::write(lib.journal_dir().join("20260101-090000.json"), b"{}").unwrap();
    let vault = lib.vault().to_path_buf();
    drop(lib);

    // Put the library back the way a pre-ADR-0019 version left it: its cache beside
    // the photographs, and no marker claiming it.
    std::fs::remove_file(dir.join(blinkview_core::cache::MARKER)).unwrap();
    std::fs::rename(&vault, dir.join(".blinkview")).unwrap();

    let lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    assert!(
        dir.join(blinkview_core::cache::MARKER).is_file(),
        "the folder is named"
    );
    assert!(
        !dir.join(".blinkview").exists(),
        "nothing of the cache stays behind"
    );
    assert_eq!(
        lib.index
            .all()
            .unwrap()
            .iter()
            .map(|r| r.hash.clone())
            .collect::<Vec<_>>(),
        before,
        "the index arrived — a rename, not a rescan"
    );
    assert!(
        lib.journal_dir().join("20260101-090000.json").is_file(),
        "the journal arrived"
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(cache_for(&dir)).ok();
}

/// Renaming a library folder in Finder keeps its cache: the marker travels with it.
#[test]
fn a_renamed_folder_keeps_its_cache() {
    let dir = fixture("adr19-rename", &["20260101_090000.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before: Vec<_> = lib
        .index
        .all()
        .unwrap()
        .iter()
        .map(|r| r.hash.clone())
        .collect();
    let vault = lib.vault().to_path_buf();
    drop(lib);

    let moved = dir.parent().unwrap().join("adr19-rename-moved");
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(&dir, &moved).unwrap();

    let lib = Library::open_in(&moved, cache_for(&dir)).unwrap();
    assert_eq!(
        lib.vault(),
        vault,
        "the same cache, found by marker not by path"
    );
    assert_eq!(
        lib.index
            .all()
            .unwrap()
            .iter()
            .map(|r| r.hash.clone())
            .collect::<Vec<_>>(),
        before,
        "no rescan for a folder that only changed name"
    );
    std::fs::remove_dir_all(&moved).ok();
    std::fs::remove_dir_all(cache_for(&dir)).ok();
}

/// A library duplicated in Finder re-indexes, and never touches the original's cache.
#[test]
fn a_copied_folder_re_indexes_rather_than_shares() {
    let dir = fixture("adr19-copy", &["20260101_090000.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let original = lib.vault().to_path_buf();
    drop(lib);

    // A Finder-style duplicate: the whole folder, marker and all.
    let copy = dir.parent().unwrap().join("adr19-copy-dup");
    let _ = std::fs::remove_dir_all(&copy);
    copy_dir(&dir, &copy);

    let lib2 = Library::open_in(&copy, cache_for(&dir)).unwrap();
    assert_ne!(
        lib2.vault(),
        original,
        "the copy must not share the original's cache"
    );
    assert!(
        lib2.index.all().unwrap().is_empty(),
        "the copy starts fresh rather than inheriting photographs it has not scanned"
    );
    // And the original still has what it had.
    let lib1 = Library::open_in(&dir, cache_for(&dir)).unwrap();
    assert_eq!(lib1.vault(), original);
    assert_eq!(lib1.index.all().unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&copy).ok();
    std::fs::remove_dir_all(cache_for(&dir)).ok();
}

/// A library on read-only media opens — it used to be impossible, because opening one
/// created a directory beside the photographs.
#[cfg(unix)]
#[test]
fn a_read_only_library_opens() {
    let dir = fixture("adr19-ro", &["20260101_090000.jpg"]);
    // Everything blinkview needs to write now lives outside the folder.
    assert!(std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).is_ok());
    // Both opens happen read-only, which is the situation being tested; restoring
    // writability in between would let the second open mint a marker and legitimately
    // claim a different cache.
    let lib = Library::open_in(&dir, cache_for(&dir)).expect("a read-only folder is a library");
    assert!(
        !dir.join(blinkview_core::cache::MARKER).is_file(),
        "no marker could be written, and that is survivable"
    );
    // With no marker the cache is keyed by where the folder is — which must be stable,
    // or a read-only library starts from scratch on every open.
    let again = Library::open_in(&dir, cache_for(&dir)).unwrap();
    assert_eq!(
        lib.vault(),
        again.vault(),
        "the path-derived key is stable across opens"
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(cache_for(&dir)).ok();
}

/// `std::fs::copy`, recursively — what Finder does when someone duplicates a folder.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// ADR-0010: a rating lives beside its photograph, so moving the photograph has to
/// carry the rating with it — and undoing the move has to carry it back.
///
/// Without this the rating is not corrupted, it simply stops being found, which looks
/// exactly like losing it.
#[test]
fn a_move_carries_metadata_and_undo_brings_it_back() {
    use blinkview_core::plan::{Op, Plan};
    use blinkview_core::userdata::UserDataSet;

    let dir = fixture("meta-move", &["Day1/a.jpg", "Day3/keep.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();

    let hash = lib
        .index
        .all()
        .unwrap()
        .into_iter()
        .find(|r| r.path == "Day1/a.jpg")
        .expect("scanned")
        .hash;

    let mut set = UserDataSet::load(&dir).unwrap();
    set.edit(&hash, "Day1", |u| {
        u.set_rating(&hash, 5);
        u.set_label(&hash, Some("red".into()));
    });
    set.save(&dir).unwrap();
    assert!(dir.join("Day1/blinkview.json").exists());

    let mut p = Plan::new("move");
    p.ops.push(Op::Move {
        hash: hash.clone(),
        from: "Day1/a.jpg".into(),
        to: "Day3/a.jpg".into(),
    });
    let journal = p.apply(&mut lib).unwrap();

    let after = UserDataSet::load(&dir).unwrap();
    let m = after.get(&hash, "Day3");
    assert_eq!(m.rating, 5, "the rating did not follow the photograph");
    assert_eq!(m.label.as_deref(), Some("red"), "the label did not follow");
    assert!(
        !dir.join("Day1/blinkview.json").exists(),
        "the old folder kept a stale entry"
    );

    journal.undo(&mut lib).unwrap();
    let back = UserDataSet::load(&dir).unwrap();
    assert_eq!(
        back.get(&hash, "Day1").rating,
        5,
        "undo did not bring the rating back"
    );
    assert_eq!(
        back.get(&hash, "Day3").rating,
        0,
        "undo left the rating in the folder the photograph no longer occupies"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR-0011: a library in a synced folder will corrupt its SQLite sooner or later.
/// Because the cache is reproducible and holds nothing user-authored, that must cost a
/// rescan and nothing else — no error, and above all no loss of ratings.
#[test]
fn a_corrupt_index_is_rebuilt_without_losing_user_data() {
    use blinkview_core::userdata::UserDataSet;

    let dir = fixture("corrupt", &["Day1/a.jpg", "Day1/b.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();

    let mut set = UserDataSet::load(&dir).unwrap();
    set.edit(&hash, "Day1", |u| u.set_rating(&hash, 5));
    set.save(&dir).unwrap();
    let db = lib.vault().join("index.sqlite");
    drop(lib);

    // Damage the header, which is what a truncated or conflicted sync copy looks like.
    // Scribbling over a page body is *not* enough: quick_check reports "ok" for that,
    // because it validates b-tree structure rather than page contents.
    let mut bytes = std::fs::read(&db).unwrap();
    bytes[..16].copy_from_slice(b"NotADatabase\0\0\0\0");
    std::fs::write(&db, &bytes).unwrap();

    let mut lib =
        Library::open_in(&dir, cache_for(&dir)).expect("a corrupt index must not be fatal");
    // Proves the rebuild actually happened rather than the damage going unnoticed:
    // a rebuilt index is empty until it is scanned again.
    assert!(
        lib.index.all().unwrap().is_empty(),
        "the index was not rebuilt — the corruption went undetected"
    );
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        lib.index.all().unwrap().len(),
        2,
        "the library did not come back"
    );

    let after = UserDataSet::load(&dir).unwrap();
    assert_eq!(
        after.get(&hash, "Day1").rating,
        5,
        "the rating went with the cache"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The primitive the command layer compiles to. A typed instruction has already chosen
/// its photographs, so this must move exactly those and nothing else.
#[test]
fn move_into_plans_only_the_chosen_photographs() {
    use blinkview_core::plan;

    let dir = fixture("move-into", &["a.jpg", "b.jpg", "Trip/c.jpg", "Trip/a.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let rows = lib.index.all().unwrap();
    let by_path = |p: &str| rows.iter().find(|r| r.path == p).unwrap().hash.clone();

    // b.jpg moves; c.jpg is already there; a.jpg collides with Trip/a.jpg.
    let hashes = vec![by_path("b.jpg"), by_path("Trip/c.jpg"), by_path("a.jpg")];
    let p = plan::move_into(&lib, &hashes, "Trip").unwrap();

    assert_eq!(p.ops.len(), 1, "only b.jpg should move");
    assert_eq!(p.ops[0].to(), "Trip/b.jpg");
    assert_eq!(
        p.skipped.len(),
        1,
        "the name collision must be reported, not overwritten"
    );
    assert!(p.skipped[0].1.contains("already exists"));

    // Nothing outside the chosen set is touched.
    assert!(!p.ops.iter().any(|o| o.from() == "Trip/c.jpg"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn move_into_refuses_a_destination_the_filesystem_would_reject() {
    use blinkview_core::plan;
    let dir = fixture("move-bad-dest", &["a.jpg"]);
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let h = vec![lib.index.all().unwrap()[0].hash.clone()];

    assert!(
        plan::move_into(&lib, &h, "").is_err(),
        "an empty destination is not a folder"
    );
    assert!(plan::move_into(&lib, &h, "  ").is_err());
    assert!(
        plan::move_into(&lib, &h, "Trip: Greece").is_err(),
        "a colon is reserved on exFAT and must be refused before anything moves"
    );
    assert!(
        plan::move_into(&lib, &h, "Trip/Greece Day3").is_ok(),
        "nesting is fine"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An operation that cannot be recorded must not stand.
///
/// This was a real failure, not a hypothetical: a plan labelled "move 12 to Trip/Alps"
/// put a `/` into the journal filename, the write failed, and twenty-three photographs
/// had already moved with no journal entry — unreachable by undo, while the app
/// reported the operation as failed. Files first and journal last is the wrong order
/// unless a journal failure rolls the files back.
#[test]
fn a_move_that_cannot_be_journalled_is_rolled_back() {
    use blinkview_core::plan::{Op, Plan};

    let dir = fixture("journal-fail", &["Day1/a.jpg", "Day1/b.jpg"]);
    std::fs::create_dir_all(dir.join("Day3")).unwrap();
    let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let rows = lib.index.all().unwrap();

    // Make the journal directory unwritable by replacing it with a file.
    let jdir = lib.journal_dir();
    std::fs::remove_dir_all(&jdir).unwrap();
    std::fs::write(&jdir, b"not a directory").unwrap();

    let mut p = Plan::new("move");
    for r in &rows {
        p.ops.push(Op::Move {
            hash: r.hash.clone(),
            from: r.path.clone(),
            to: format!("Day3/{}", r.path.rsplit('/').next().unwrap()),
        });
    }
    let err = p
        .apply(&mut lib)
        .expect_err("an unrecordable move must fail");
    assert!(
        format!("{err:#}").contains("could not be recorded"),
        "unexpected error: {err:#}"
    );

    // The photographs must be exactly where they started.
    assert!(
        dir.join("Day1/a.jpg").exists(),
        "a.jpg was left moved with no way back"
    );
    assert!(
        dir.join("Day1/b.jpg").exists(),
        "b.jpg was left moved with no way back"
    );
    assert!(!dir.join("Day3/a.jpg").exists());
    // And the index must agree with the disk, or the next scan would report phantom moves.
    let after = lib.index.all().unwrap();
    assert!(
        after.iter().all(|r| r.path.starts_with("Day1/")),
        "index left pointing at Day3"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
