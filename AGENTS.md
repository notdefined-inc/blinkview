# Repository Guardrails

Global contract applies (~/.codex/AGENTS.md). Repo specifics below override it.

## Read order
1. docs/CURRENT/STATUS.md
2. docs/CURRENT/ARCHITECTURE.md
3. docs/CURRENT/ROADMAP.md
4. docs/DECISIONS/*
5. docs/SPECS/active/*

## Project rules

**The vault invariant — this is the product.**
- Photos on disk are the only source of truth. `.openfoto/` is 100% derived and must be
  safe to `rm -rf` at any moment; `openfoto scan` rebuilds it fully. Never store anything
  in the index that cannot be recomputed from the photos themselves.
- File identity is the BLAKE3 content hash, never the path. Users rename and move folders
  in Finder while the tool is running; that is normal behavior, not an error state.
- Every mutation writes a journal entry to `.openfoto/journal/` before touching the disk,
  and `openfoto undo` must restore the exact prior tree.

**Safety rules (each was learned by getting it wrong — see ADR-0003).**
- Clustering is complete-linkage. Single-linkage chaining is a correctness bug, not a tuning knob.
- A perceptual-hash match is a *candidate*, never a conclusion — always confirm with pixel comparison.
- Identity assignment is discriminative (nearest-of-many-people), never a bare threshold.
- No destructive command runs without a dry-run path producing the full plan first.
- Filenames must be unique across the whole library, not per-folder.
- Low confidence never auto-moves. Unclear items stay where they are and get reported.

**Platform reality**
- exFAT is a first-class target: reject `" * / : < > ? \ |` in filenames, and carry
  AppleDouble `._` sidecars with their parent file on every move/rename.

## Build / test / run
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
  **`--all-targets` is not optional.** Without it clippy skips test targets and will
  report a clean workspace while a test fails to compile.
- `cargo run -p openfoto-cli -- <cmd>` · `cargo run -p openfoto-desktop`

## Traps hit here before
- cargo silently ignores `cfg(debug_assertions)` when selecting dependencies. Dev-only
  dependencies must be real features, or they ship in release builds.
- Synchronous `#[tauri::command]` functions run on the UI thread. Anything touching the
  disk must be `async` or the window freezes.
- A `display` value in a class rule outranks the `hidden` attribute; overlays need an
  explicit `[hidden]{display:none!important}`.
- `backdrop-filter` on a full-screen container can stop WKWebView painting its children
  altogether — the element measures correctly and never appears.
