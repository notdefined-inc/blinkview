# Architecture

## Today
Not built yet. Cargo workspace with two empty crates.

## Intended shape

    crates/openfoto-core/   Rust lib. All logic: scan, hash, dedupe, faces, plan/apply/undo.
    crates/openfoto-cli/    Rust bin. Thin argument parsing over core. Ships first.
    (later) openfoto-app/   Tauri v2 desktop viewer. Same core crate, web frontend.

The CLI and the eventual GUI are peers over one engine. The CLI is never demoted to a
legacy interface.

## The vault

    <library>/              any folder; photos live in ordinary subfolders
      .openfoto/            entirely derived — safe to delete, rebuilt by `scan`
        index.sqlite        hash -> path, EXIF, phash, face embeddings
        thumbs/             content-addressed thumbnail cache
        journal/            one entry per applied operation; the undo history
        people.json         identity names + reference embeddings

Rationale in docs/DECISIONS/ADR-0001-vault-format.md.

## Why Rust
The end goal is a shippable desktop app; bundling a Python runtime into one is the usual
route to slow and fragile. `ort` runs the same ONNX models the prototype validated.
See docs/DECISIONS/ADR-0002-rust-core.md.
