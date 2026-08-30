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

## What lives where

    <library>/
      openfoto.json          ratings, labels, albums      user-authored
      openfoto-people.json   names + reference faces      user-authored
      Trash/  Originals/     deleted and pre-edit photos  visible, journalled
      .openfoto/             index, thumbs, faces, …      100% derived, disposable

The split is the point. Only the two JSON files and the two folders hold anything a
machine cannot reproduce, and all four are visible in Finder and travel with the folder
when it is copied. `.openfoto/` can be deleted at any time and costs only recomputation —
without qualification. See ADR-0007.

## The vault

    <library>/              any folder; photos live in ordinary subfolders
      .openfoto/            entirely derived — safe to delete, rebuilt by `scan`
        index.sqlite        hash -> path, EXIF, phash, face embeddings
        thumbs/             content-addressed thumbnail cache
        derived/            lightbox previews and HEIC transcodes, by hash
        journal/            one entry per applied operation; the undo history
        people.json         identity names + reference embeddings

Rationale in docs/DECISIONS/ADR-0001-vault-format.md.

## Image formats

JPEG and PNG decode in-process. HEIC is transcoded by macOS `sips` and cached — see
ADR-0005, which is also the project's only macOS-only dependency. Video thumbnails come
from ffmpeg when it is installed, and simply do not exist when it is not.

`imageio` is the single decode seam: everything goes through it, which is what makes
EXIF orientation and HEIC handling uniform rather than per-caller.

## Serving pixels to the UI

The desktop app registers its own `photo://` scheme rather than using Tauri's asset
protocol, so the security boundary is explicit: a file is served only if it resolves
inside a folder the user added. The same handler produces thumbnails and HEIC
transcodes on demand, which is what lets a large library paint immediately — the
virtualised grid only ever requests the few dozen images actually on screen.

Because WKWebView cannot be relied on to cache custom-scheme responses, and grid cells
are destroyed offscreen and rebuilt on re-entry, the handler keeps its own 64 MiB
byte-budgeted LRU over the small derived files (thumbnails, previews): a cache hit never
touches the filesystem. Cells ask for their `src` only as they approach the viewport
(IntersectionObserver), so a fast flick no longer queues rows the user never sees.
Videos render their poster frames on a dedicated two-thread pool, dispatched at scheme
dispatch time, so an ffmpeg spawn can never occupy a photograph-decode thread. The
lightbox steps through `.openfoto/derived/p-<hash>.jpg` — a 2000 px JPEG derived on
first view — instead of decoding the 12–48 MP original per keypress.

## Why Rust
The end goal is a shippable desktop app; bundling a Python runtime into one is the usual
route to slow and fragile. `ort` runs the same ONNX models the prototype validated.
See docs/DECISIONS/ADR-0002-rust-core.md.
