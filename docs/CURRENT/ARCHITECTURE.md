# Architecture

## Today

    crates/blinkview-core/       all logic; no UI, no CLI concerns
    crates/blinkview-cli/        `blinkview` binary — thin wrapper over core
    apps/desktop/src-tauri/     Tauri v2 shell — same core crate
    apps/desktop/dist/          frontend: index.html, app.css, app.js (no bundler)

The CLI and the desktop app are peers over one engine. Every Tauri command calls the
same function the CLI does, so the two can never disagree about what a library holds
or what an operation will do.

## Intended shape

    crates/blinkview-core/   Rust lib. All logic: scan, hash, dedupe, faces, plan/apply/undo.
    crates/blinkview-cli/    Rust bin. Thin argument parsing over core. Ships first.
    apps/desktop/           Tauri v2 desktop viewer. Same core crate, web frontend.

The CLI and the eventual GUI are peers over one engine. The CLI is never demoted to a
legacy interface.

## Sources

The desktop app holds a list of *source folders*, each an independent library with its
own disposable `.blinkview/`. The list lives in the app config directory and is the only
app-level state; losing it costs nothing but re-adding folders. There is deliberately no
global database — that is the whole premise (ADR-0001).

## What lives where

    <library>/
      blinkview.json          ratings, labels, how the
                             folder is arranged           user-authored
      blinkview-people.json   names + reference faces      user-authored
      Trash/  Originals/     deleted and pre-edit photos  visible, journalled
      .blinkview/             index, thumbs, faces, …      100% derived, disposable

The split is the point. Only the two JSON files and the two folders hold anything a
machine cannot reproduce, and all four are visible in Finder and travel with the folder
when it is copied. `.blinkview/` can be deleted at any time and costs only recomputation —
without qualification. See ADR-0007.

## The vault

    <library>/              any folder; photos live in ordinary subfolders
      .blinkview/            entirely derived — safe to delete, rebuilt by `scan`
        index.sqlite        hash -> path, EXIF, phash, face embeddings
        thumbs/             content-addressed thumbnail cache
        derived/            lightbox previews and HEIC transcodes, by hash
        journal/            one entry per applied operation; the undo history
        people.json         identity names + reference embeddings

Rationale in docs/DECISIONS/ADR-0001-vault-format.md.

## Rewriting a photograph

Editing and metadata stripping are the only operations that change a photograph's bytes
rather than its name or its folder. Both keep the untouched original in the visible
`Originals/` folder (ADR-0006, ADR-0015), and both go through `edit::keep_original` so
the collision-avoiding rule lives in one place.

Both also change the file's **content hash**, which is what ratings, labels and album
membership are keyed by (ADR-0007). Any such operation therefore has to carry that
metadata onto the new hash with `UserDataSet::rekey` — the alternative, discovered by
writing ADR-0015, is that a five-star photograph comes back unrated.

Stripping is a segment rewrite, not a re-encode: `metadata::strip` copies the
entropy-coded image data through untouched, so the pixels are bit-identical and only
the records of make, model, exposure and location are dropped.

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
lightbox steps through `.blinkview/derived/p-<hash>.jpg` — a 2000 px JPEG derived on
first view — instead of decoding the 12–48 MP original per keypress.

## Places, without a network

    crates/blinkview-core/data/places.bin   170,860 places, 4.35 MB, packed
    apps/desktop/dist/world{110,50}.json   Natural Earth outlines, two detail levels

Both are produced only by `tools/build-geodata.sh`, and both ship inside the binary.
`geo` resolves a coordinate to the nearest place through a one-degree grid index, and
searches the same table by name so the map and the search box can never disagree about
what a place is.

The map is drawn on a canvas from the bundled outlines and never requests a tile. That
is a privacy decision before it is a performance one: a tile request tells a server
where the user has been, every time they pan.

## Why Rust
The end goal is a shippable desktop app; bundling a Python runtime into one is the usual
route to slow and fragile. `ort` runs the same ONNX models the prototype validated.
See docs/DECISIONS/ADR-0002-rust-core.md.
