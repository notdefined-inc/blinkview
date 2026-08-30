//! End-to-end: scan -> rename -> undo, plus the failure modes that motivated the design.

use blinkview_core::{journal::Journal, rename, scan, Library};
use std::path::{Path, PathBuf};

/// Minimal library of files with camera-style names. No EXIF, so capture time comes
/// from the filename — which keeps these tests independent of image fixtures.
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

    std::fs::rename(dir.join("Person1"), dir.join("Alex")).unwrap();

    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.moved, 2, "files should be re-identified by hash");
    assert_eq!(st.removed, 0, "nothing should be considered deleted");
    assert_eq!(lib.index.count().unwrap(), 2);
    assert!(lib.index.all().unwrap().iter().all(|r| r.path.starts_with("Alex/")));
    std::fs::remove_dir_all(&dir).ok();
}

/// A plan whose destination directory disappears must abort without touching disk.
#[test]
fn aborts_when_destination_is_missing() {
    let dir = fixture("missingdst", &["a/20260816_151256.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
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
    assert!(dir.join("a/20260816_151256.jpg").exists(), "source must be untouched");
    std::fs::remove_dir_all(&dir).ok();
}

/// `.blinkview/` is disposable: deleting it and rescanning reproduces the index.
#[test]
fn vault_is_disposable() {
    let dir = fixture("disposable", &["20260816_151256.jpg", "Me/20260818_170334.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let before: Vec<_> = lib.index.all().unwrap().iter().map(|r| (r.hash.clone(), r.path.clone())).collect();
    drop(lib);

    std::fs::remove_dir_all(dir.join(".blinkview")).unwrap();
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
    use blinkview_core::edit::{Adjust, Crop, Edit, Rotate, ORIGINALS};

    let dir = std::env::temp_dir().join(format!("blinkview-edit-{}", std::process::id()));
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
    let out = blinkview_core::edit::apply(&lib, "20260101_120000.jpg", &e).unwrap();

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
    assert!(!kept.contains(".blinkview"), "must not hide the original in the disposable vault");

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
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();

    let mut people = People::default();
    people.add_references("Alex", vec![vec![1.0, 0.0, 0.0]]);
    people.save(lib.root()).unwrap();

    let mut user = UserData::default();
    user.set_rating(&hash, 5);
    user.set_label(&hash, Some("red".into()));
    user.save(lib.root()).unwrap();
    drop(lib);

    // The thing the documentation invites the user to do.
    std::fs::remove_dir_all(dir.join(".blinkview")).unwrap();

    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(
        People::load(lib.root()).unwrap().people[0].name,
        "Alex",
        "names must survive deleting the cache"
    );
    let back = UserData::load(lib.root()).unwrap();
    assert_eq!(back.get(&hash).rating, 5, "ratings must survive deleting the cache");
    assert_eq!(back.get(&hash).label.as_deref(), Some("red"));
    std::fs::remove_dir_all(&dir).ok();
}

/// A library written by an older version keeps its data when opened by this one.
#[test]
fn user_data_is_rescued_from_the_old_location() {
    let dir = fixture("rescue", &["20260101_110000.jpg"]);
    let vault = dir.join(".blinkview");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join("people.json"),
        br#"{"people":[{"name":"Old","references":[[1.0]]}]}"#).unwrap();

    let lib = Library::open(&dir).unwrap();
    assert!(dir.join("blinkview-people.json").exists(), "moved to the root on open");
    assert!(!vault.join("people.json").exists(), "no stale copy left behind");
    assert_eq!(
        blinkview_core::faces::people::People::load(lib.root()).unwrap().people[0].name,
        "Old"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Neither cache directory is a photo folder. A leftover `.openfoto/` — from an older
/// install, or a copy of the library that carried one along — is full of thumbnails,
/// and indexing it puts a folder of duplicates in the sidebar out of nowhere.
#[test]
fn neither_the_current_nor_the_former_cache_is_indexed() {
    let dir = fixture("caches", &["20260101_110000.jpg"]);
    for cache in [".blinkview", ".openfoto"] {
        std::fs::create_dir_all(dir.join(cache).join("thumbs")).unwrap();
        std::fs::write(dir.join(cache).join("thumbs/deadbeef.jpg"), b"thumbnail").unwrap();
    }

    let mut lib = Library::open(&dir).unwrap();
    let st = scan::scan(&mut lib, false).unwrap();
    assert_eq!(st.seen, 1, "only the photograph is a photograph");
    assert!(
        lib.index.all().unwrap().iter().all(|r| !r.path.contains("thumbs")),
        "no cache file may reach the index"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-0017: a library written before the rename opens with everything intact, and
/// keeps its index rather than paying for the new name with a full rescan.
#[test]
fn a_library_from_before_the_rename_is_adopted_whole() {
    use blinkview_core::userdata::UserData;

    let dir = fixture("rename", &["20260101_110000.jpg", "Day1/20260102_120000.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();
    let mut u = UserData::load(&dir).unwrap();
    u.set_rating(&hash, 5);
    u.save(&dir).unwrap();
    std::fs::write(dir.join("blinkview-people.json"),
        br#"{"people":[{"name":"Alex","references":[[1.0]]}]}"#).unwrap();
    std::fs::write(dir.join("Day1/blinkview.json"), br#"{"albums":{},"photos":{}}"#).unwrap();
    drop(lib);

    // Put the library back the way the previous name left it.
    std::fs::rename(dir.join(".blinkview"), dir.join(".openfoto")).unwrap();
    for (from, to) in [
        ("blinkview.json", "openfoto.json"),
        ("blinkview-people.json", "openfoto-people.json"),
        ("Day1/blinkview.json", "Day1/openfoto.json"),
    ] {
        std::fs::rename(dir.join(from), dir.join(to)).unwrap();
    }

    let mut lib = Library::open(&dir).unwrap();
    assert!(!dir.join(".openfoto").exists(), "the old cache is adopted, not left behind");
    assert!(dir.join(".blinkview/index.sqlite").is_file());
    assert_eq!(
        lib.index.all().unwrap().len(),
        2,
        "adopting the cache keeps the index — a rename must not cost a rescan"
    );
    assert_eq!(UserData::load(&dir).unwrap().get(&hash).rating, 5);
    assert_eq!(
        blinkview_core::faces::people::People::load(lib.root()).unwrap().people[0].name,
        "Alex"
    );
    // The cascade renames what it reads, so the second open sees one name only.
    lib.user_data().unwrap();
    assert!(dir.join("Day1/blinkview.json").exists());
    assert!(!dir.join("Day1/openfoto.json").exists());
    std::fs::remove_dir_all(&dir).ok();
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
    let mut lib = Library::open(&dir).unwrap();
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
    assert_eq!(back.get(&hash, "Day1").rating, 5, "undo did not bring the rating back");
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
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let hash = lib.index.all().unwrap()[0].hash.clone();

    let mut set = UserDataSet::load(&dir).unwrap();
    set.edit(&hash, "Day1", |u| u.set_rating(&hash, 5));
    set.save(&dir).unwrap();
    drop(lib);

    // Damage the header, which is what a truncated or conflicted sync copy looks like.
    // Scribbling over a page body is *not* enough: quick_check reports "ok" for that,
    // because it validates b-tree structure rather than page contents.
    let db = dir.join(".blinkview/index.sqlite");
    let mut bytes = std::fs::read(&db).unwrap();
    bytes[..16].copy_from_slice(b"NotADatabase\0\0\0\0");
    std::fs::write(&db, &bytes).unwrap();

    let mut lib = Library::open(&dir).expect("a corrupt index must not be fatal");
    // Proves the rebuild actually happened rather than the damage going unnoticed:
    // a rebuilt index is empty until it is scanned again.
    assert!(
        lib.index.all().unwrap().is_empty(),
        "the index was not rebuilt — the corruption went undetected"
    );
    scan::scan(&mut lib, false).unwrap();
    assert_eq!(lib.index.all().unwrap().len(), 2, "the library did not come back");

    let after = UserDataSet::load(&dir).unwrap();
    assert_eq!(after.get(&hash, "Day1").rating, 5, "the rating went with the cache");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The primitive the command layer compiles to. A typed instruction has already chosen
/// its photographs, so this must move exactly those and nothing else.
#[test]
fn move_into_plans_only_the_chosen_photographs() {
    use blinkview_core::plan;

    let dir = fixture("move-into", &["a.jpg", "b.jpg", "Trip/c.jpg", "Trip/a.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let rows = lib.index.all().unwrap();
    let by_path = |p: &str| rows.iter().find(|r| r.path == p).unwrap().hash.clone();

    // b.jpg moves; c.jpg is already there; a.jpg collides with Trip/a.jpg.
    let hashes = vec![by_path("b.jpg"), by_path("Trip/c.jpg"), by_path("a.jpg")];
    let p = plan::move_into(&lib, &hashes, "Trip").unwrap();

    assert_eq!(p.ops.len(), 1, "only b.jpg should move");
    assert_eq!(p.ops[0].to(), "Trip/b.jpg");
    assert_eq!(p.skipped.len(), 1, "the name collision must be reported, not overwritten");
    assert!(p.skipped[0].1.contains("already exists"));

    // Nothing outside the chosen set is touched.
    assert!(!p.ops.iter().any(|o| o.from() == "Trip/c.jpg"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn move_into_refuses_a_destination_the_filesystem_would_reject() {
    use blinkview_core::plan;
    let dir = fixture("move-bad-dest", &["a.jpg"]);
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let h = vec![lib.index.all().unwrap()[0].hash.clone()];

    assert!(plan::move_into(&lib, &h, "").is_err(), "an empty destination is not a folder");
    assert!(plan::move_into(&lib, &h, "  ").is_err());
    assert!(
        plan::move_into(&lib, &h, "Trip: Greece").is_err(),
        "a colon is reserved on exFAT and must be refused before anything moves"
    );
    assert!(plan::move_into(&lib, &h, "Trip/Greece Day3").is_ok(), "nesting is fine");

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
    let mut lib = Library::open(&dir).unwrap();
    scan::scan(&mut lib, false).unwrap();
    let rows = lib.index.all().unwrap();

    // Make the journal directory unwritable by replacing it with a file.
    let jdir = dir.join(".blinkview/journal");
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
    let err = p.apply(&mut lib).expect_err("an unrecordable move must fail");
    assert!(
        format!("{err:#}").contains("could not be recorded"),
        "unexpected error: {err:#}"
    );

    // The photographs must be exactly where they started.
    assert!(dir.join("Day1/a.jpg").exists(), "a.jpg was left moved with no way back");
    assert!(dir.join("Day1/b.jpg").exists(), "b.jpg was left moved with no way back");
    assert!(!dir.join("Day3/a.jpg").exists());
    // And the index must agree with the disk, or the next scan would report phantom moves.
    let after = lib.index.all().unwrap();
    assert!(after.iter().all(|r| r.path.starts_with("Day1/")), "index left pointing at Day3");

    let _ = std::fs::remove_dir_all(&dir);
}
