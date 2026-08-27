# Architecture

## Today

    crates/openfoto-core/       all logic; no UI, no CLI concerns
    crates/openfoto-cli/        `openfoto` binary — thin wrapper over core
    apps/desktop/src-tauri/     Tauri v2 shell — same core crate
    apps/desktop/dist/          frontend: index.html, app.css, app.js (no bundler)

The CLI and the desktop app are peers over one engine. Every Tauri command calls the
same function the CLI does, so the two can never disagree about what a library holds
or what an operation will do.

## Intended shape

    crates/openfoto-core/   Rust lib. All logic: scan, hash, dedupe, faces, plan/apply/undo.
    crates/openfoto-cli/    Rust bin. Thin argument parsing over core. Ships first.
    apps/desktop/           Tauri v2 desktop viewer. Same core crate, web frontend.

The CLI and the eventual GUI are peers over one engine. The CLI is never demoted to a
legacy interface.

## Sources

The desktop app holds a list of *source folders*, each an independent library with its
own disposable `.openfoto/`. The list lives in the app config directory and is the only
app-level state; losing it costs nothing but re-adding folders. There is deliberately no
global database — that is the whole premise (ADR-0001).

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
