//! The combined pass must produce what the separate passes produced.
//!
//! ADR-0003 and ADR-0008 fixed their thresholds against particular model outputs, so a
//! refactor that quietly changed a box or an embedding would invalidate both without
//! failing anything. These compare the new pass against the old ones directly.

use openfoto_core::{analyze, semantic, Library};
use std::path::PathBuf;

fn fixture(name: &str) -> Option<PathBuf> {
    // Real photographs, since detection on synthetic images finds nothing.
    let src = PathBuf::from("/Users/notdefined/Desktop/openfoto-demo");
    if !src.is_dir() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("openfoto-ap-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let mut n = 0;
    for e in std::fs::read_dir(&src).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("jpg") {
            std::fs::copy(&p, dir.join(p.file_name()?)).ok()?;
            n += 1;
            if n == 12 {
                break;
            }
        }
    }
    (n > 0).then_some(dir)
}

fn models_ready() -> bool {
    openfoto_core::faces::models::find("yunet.onnx").is_ok()
}

#[test]
fn the_combined_pass_finds_the_same_faces_and_embeddings() {
    let Some(dir) = fixture("equiv") else { return };
    if !models_ready() || !semantic::ImageEncoder::available() {
        eprintln!("skipping: models not installed");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    // Reference: the passes as they were, each decoding for itself.
    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();
    openfoto_core::faces::pipeline::analyze(&lib, openfoto_core::faces::pipeline::DEFAULT_SCORE)
        .unwrap();
    semantic::analyze(&lib, &openfoto_core::progress::silent).unwrap();
    let want_faces = lib.all_faces().unwrap();
    let want_clip = lib.index.all_clip().unwrap();
    drop(lib);

    // Same library, cache discarded, run through the combined pass instead.
    std::fs::remove_dir_all(dir.join(".openfoto")).unwrap();
    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();
    let st = analyze::run(&mut lib, analyze::Stages::default()).unwrap();
    let got_faces = lib.all_faces().unwrap();
    let got_clip = lib.index.all_clip().unwrap();

    assert_eq!(got_faces.len(), want_faces.len(), "different number of faces");
    let key = |f: &openfoto_core::faces::store::StoredFace| (f.hash.clone(), f.idx);
    let mut want_sorted = want_faces.clone();
    let mut got_sorted = got_faces.clone();
    want_sorted.sort_by_key(key);
    got_sorted.sort_by_key(key);
    for (a, b) in want_sorted.iter().zip(got_sorted.iter()) {
        assert_eq!((&a.hash, a.idx), (&b.hash, b.idx));
        for (name, x, y) in [("x", a.x, b.x), ("y", a.y, b.y), ("w", a.w, b.w), ("h", a.h, b.h)] {
            assert!((x - y).abs() < 1.0, "{name} moved: {x} vs {y} on {}", a.hash);
        }
        match (&a.embedding, &b.embedding) {
            (Some(u), Some(v)) => {
                let cos = semantic::similarity(u, v);
                assert!(cos > 0.9999, "face embedding drifted: cosine {cos:.5}");
            }
            (None, None) => {}
            _ => panic!("one pass embedded a face the other did not: {}", a.hash),
        }
    }

    assert_eq!(got_clip.len(), want_clip.len(), "different number of embeddings");
    for (hash, v) in &got_clip {
        let w = want_clip.iter().find(|(h, _)| h == hash).expect("same photographs");
        let cos = semantic::similarity(v, &w.1);
        assert!(cos > 0.9999, "image embedding drifted: cosine {cos:.5} — ADR-0008's threshold was measured against the old value");
    }

    // And it really did decode once per photograph rather than three times.
    assert_eq!(st.decoded, st.considered, "expected exactly one decode each");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_is_opened_when_nothing_is_missing() {
    let Some(dir) = fixture("cached") else { return };
    if !models_ready() || !semantic::ImageEncoder::available() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();
    let first = analyze::run(&mut lib, analyze::Stages::default()).unwrap();
    assert!(first.decoded > 0);

    // Second run: everything cached, so no photograph should be opened at all.
    let again = analyze::run(&mut lib, analyze::Stages::default()).unwrap();
    assert_eq!(again.decoded, 0, "re-ran work that was already done");
    assert_eq!(again.from_preview, 0);
    assert_eq!(again.skipped, again.considered);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thumbnail_alone_does_not_force_a_full_decode() {
    let Some(dir) = fixture("preview") else { return };
    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();
    // Thumbnails only, so the camera's embedded preview is enough where there is one.
    let st = analyze::run(&mut lib, analyze::Stages::only_thumbs()).unwrap();
    assert!(st.thumbs > 0, "no thumbnails were produced");
    assert_eq!(st.thumbs, st.from_preview + st.decoded);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An interrupted pass must leave finished work finished.
#[test]
fn a_second_run_finishes_what_the_first_started() {
    let Some(dir) = fixture("resume") else { return };
    if !models_ready() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();

    // Stand in for an interruption: do the thumbnails, then everything.
    let a = analyze::run(&mut lib, analyze::Stages::only_thumbs()).unwrap();
    assert!(a.thumbs > 0);
    let b = analyze::run(&mut lib, analyze::Stages { thumbs: true, faces: true, semantic: false })
        .unwrap();
    assert_eq!(b.thumbs, 0, "thumbnails were redone");
    assert!(b.decoded > 0, "faces still needed a decode");

    let c = analyze::run(&mut lib, analyze::Stages { thumbs: true, faces: true, semantic: false })
        .unwrap();
    assert_eq!(c.decoded, 0, "a third run still found work to do");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file openfoto cannot read must be given up on *once*, not on every pass.
///
/// The symptom this prevents: a library reporting the same "15 photos left" for ever,
/// because fifteen WebP files saved with .jpg extensions failed every time and so never
/// stopped counting as outstanding work.
#[test]
fn an_unreadable_file_is_not_retried_for_ever() {
    let dir = std::env::temp_dir().join(format!("openfoto-unread-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("corrupt.jpg"), b"\xff\xd8\xffnot really a jpeg at all").unwrap();
    std::fs::write(dir.join("nonsense.png"), b"definitely not a png").unwrap();

    let mut lib = Library::open(&dir).unwrap();
    openfoto_core::scan::scan(&mut lib, false).unwrap();

    let first = analyze::run(&mut lib, analyze::Stages::only_thumbs()).unwrap();
    assert_eq!(first.thumbs, 0, "nothing here can be decoded");
    assert_eq!(first.unreadable, 2, "both failures should be recorded");

    // The second pass must find no outstanding work at all.
    let again = analyze::run(&mut lib, analyze::Stages::only_thumbs()).unwrap();
    assert_eq!(again.decoded, 0, "an unreadable file was tried again");
    assert_eq!(again.unreadable, 0, "and recorded again");
    assert_eq!(again.skipped, again.considered, "everything should be accounted for");

    // The reasons are kept, so the count can be explained rather than left mysterious.
    let listed = lib.index.unreadable().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|(_, why)| !why.is_empty()));

    let _ = std::fs::remove_dir_all(&dir);
}
