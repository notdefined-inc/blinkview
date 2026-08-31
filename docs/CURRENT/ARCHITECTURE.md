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
own cache. The list lives in the app config directory and is the only
app-level state; losing it costs nothing but re-adding folders. There is deliberately no
global database — that is the whole premise (ADR-0001).

A source entry carries a depth: recursive (the default, and what every entry written
before 2026-08-31 still means) or *shallow* — only the files directly in the folder.
Depth is chosen at add time from a **survey** (`scan::survey_folder`), which counts
media and subfolders from directory entries alone, cancellable and capped at 200,000
entries, and is changeable afterwards without touching user-authored metadata. New
sources also skip a fixed list of system directory names (`Library`, `node_modules`,
`.git`, …) while descending; the folder chosen directly is never skipped by its own
name.

## Peeking

There are two commitment levels over the same `Library` type (ADR-0020). A **peek** is
a markerless, session-only library: shallow scan, no watcher, a cache under
`<cache root>/peek/<path-id>/` deleted on close, and every writing command refusing it
by name. Keeping the folder promotes it to a source. Peeks are how Open With
(`fileAssociations` → `RunEvent::Opened` → `open_path`) and window drops enter; a path
already inside a source routes to that library instead, keyed by the stored source
path so a symlinked folder is never opened twice.

## What lives where

    <library>/
      blinkview.json          ratings, labels, how the
                             folder is arranged           user-authored
      blinkview-people.json   names + reference faces      user-authored
      Trash/  Originals/     deleted and pre-edit photos  visible, journalled
      .blinkview-id           40 bytes naming the cache (ADR-0019)

The split is the point. Only the two JSON files and the two folders hold anything a
machine cannot reproduce, and all four are visible in Finder and travel with the folder
when it is copied. The cache can be deleted at any time and costs only recomputation —
without qualification. See ADR-0007; the cache's location is ADR-0019.

## The cache

Since ADR-0019 the derived cache is not inside the library at all. A library holds its
photographs, a `.blinkview-id` marker, and the two JSON files below — nothing else.

    <library>/              any folder; photos live in ordinary subfolders
      .blinkview-id          32 hex characters naming this library's cache
      blinkview.json         ratings, labels, saved searches (ADR-0007)
      blinkview-people.json  names, exclusions, dismissals — as face pointers

    ~/Library/Caches/dev.notdefined.blinkview/     the machine's cache root
      libraries/<id>/       everything the marker names — entirely derived
        index.sqlite        hash -> path, EXIF, phash, face embeddings
        thumbs/             content-addressed thumbnail cache
        derived/            lightbox previews and HEIC transcodes, by hash
        faces/              face crops for sidebars and review
        journal/            one entry per applied operation; the undo history
        path                the breadcrumb: the folder this cache last served

The marker, not the path, is the key: a folder renamed in Finder keeps its cache, a
folder copied in Finder starts fresh, and read-only media opens with a path-derived key.
Migration of an in-folder `.blinkview/` is a rename when the filesystems allow it and a
fresh start when they do not — the old directory is never deleted from inside the
photographs. Rationale in ADR-0019, which supersedes the placement half of ADR-0001.

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

Camera RAW — CR3, CR2, NEF, ARW, RAF, DNG — is read from the JPEG the camera embedded,
never developed and never written back (ADR-0018). `raw::preview` follows the tag that
*declares* a preview and accepts only SOF0/1/2, because inside a RAW an SOF3 frame is
sensor data. Pure Rust, so it works on every platform; `sips` catches a file that
declares no usable preview, on macOS.

`imageio` is the single decode seam: everything goes through it, which is what makes
EXIF orientation, HEIC and RAW handling uniform rather than per-caller.
`imageio::camera_preview` is the one call that means "the picture the camera already
made", whichever container it is in.

## Serving pixels to the UI

The desktop app registers its own `photo://` scheme rather than using Tauri's asset
protocol, so the security boundary is explicit: a file is served only if it resolves
inside a folder the user added — or, while a peek is open, if its *parent* is exactly
the peeked folder (never its subtree: the promise not to recurse is also the grant).
The same handler produces thumbnails and HEIC
transcodes on demand, which is what lets a large library paint immediately — the
virtualised grid only ever requests the few dozen images actually on screen.

Because WKWebView cannot be relied on to cache custom-scheme responses, and grid cells
are destroyed offscreen and rebuilt on re-entry, the handler keeps its own 64 MiB
byte-budgeted LRU over the small derived files (thumbnails, previews): a cache hit never
touches the filesystem. Cells ask for their `src` only as they approach the viewport
(IntersectionObserver), so a fast flick no longer queues rows the user never sees.
Videos render their poster frames on a dedicated two-thread pool, dispatched at scheme
dispatch time, so an ffmpeg spawn can never occupy a photograph-decode thread. The
lightbox steps through a `derived/p-<hash>.jpg` in the library's cache — a 2000 px JPEG derived on
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
