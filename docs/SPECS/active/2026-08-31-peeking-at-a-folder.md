# Peeking at a folder

Status: Draft   Owner: somesh   Date: 2026-08-31

Executor note: written for someone with no prior context on this codebase. Every file
reference is a real path and every line number was correct at `478c608`.

## Problem

The only way into Blinkview is **Add a folder**, which registers a source, writes a
marker, scans **recursively with no depth limit** (`crates/blinkview-core/src/scan.rs:84`,
`WalkDir::new(&root)`), starts a filesystem watcher and builds a cache. There is no way
to just *look* at some photographs. Double-clicking a JPEG cannot open Blinkview at all:
`tauri.conf.json` declares no `fileAssociations` and nothing handles an opened file.

Picasa's descendants — XnView MP, FastStone, IrfanView — all do the obvious thing: open
a file, see that folder's pictures, arrow through them, nothing is imported and nothing
is written. Blinkview should do that too, and it should be genuinely free of side
effects: no marker in the folder, no cache on disk, no watcher, no recursion.

## Non-goals

- **Recursion.** A peek shows the files *directly in* one folder. Subfolders are not
  walked, not counted, and not shown. Depth is what makes a peek cheap.
- **Persistence of any kind.** No `.blinkview-id`, no cache directory, no entry in
  `sources.json`, no watcher, no journal. Closing the window forgets everything.
- **Analysis.** No face detection, no scene embeddings, no near-duplicate signatures, no
  background thumbnail pass. A peek decodes what it is about to show and no more.
- **Writing.** Rating, labelling, renaming, moving, deleting, editing and metadata writes
  are all unavailable while peeking. A peek is read-only, and says so.
- **Windows and Linux "Open With" registration.** The config is cross-platform and the
  handler is written once, but only macOS is verified here (criterion 12).
- **Choosing recursion when adding a folder.** That is the sibling spec,
  `2026-08-31-adding-a-folder-shows-its-size.md`, and can ship independently.

## Design

### The two ways in

1. **Open With.** `bundle.fileAssociations` in
   `apps/desktop/src-tauri/tauri.conf.json`, listing the extensions
   `crates/blinkview-core/src/scan.rs` already indexes: `scan::PHOTO_EXT`,
   `scan::VIDEO_EXT` and `raw::RAW_EXT`. `role: "Viewer"` and `rank: "Alternate"` —
   Blinkview views these files, and must not seize the default handler from Preview.
2. **A folder dropped on the window, or picked, that is not an added source.** Today
   `addSource` (`apps/desktop/dist/app.js:806`, `:3750`) goes straight to `add_source`.

### Peek is a markerless library

`crates/blinkview-core/src/cache.rs` already has everything needed. Its read-only path
mints no marker and keys the cache by a stable hash of the folder's path
(`cache::path_id`). A peek is that, plus a depth limit, plus a cache under
`<cache root>/peek/<path-id>/` that is **deleted when the peek closes**.

```rust
// crates/blinkview-core/src/library.rs
/// Open a folder to look at, without claiming it: no marker, no watcher, and only the
/// files directly inside it. The cache lives under the peek root and is disposable
/// even by the standards of a cache — `Library::end_peek` removes it.
pub fn peek(root: impl AsRef<Path>) -> Result<Self>;
pub fn end_peek(self) -> Result<()>;
/// Whether this library is a peek. Anything that writes must refuse when true.
pub fn is_peek(&self) -> bool;
```

`scan::scan` gains depth: add `scan_shallow(lib, rehash)` which is the existing function
with `.max_depth(1)` on the `WalkDir`. Do not add a `depth` parameter to `scan` — it has
six call sites and none of the others want one.

### The security boundary is real and must not be loosened

`serve_photo` refuses any path outside an added source
(`apps/desktop/src-tauri/src/lib.rs:3132`, `deny(403)`). A peeked folder is deliberately
not a source, so **every thumbnail and preview in a peek would 403** unless this is
handled. Handle it by granting narrowly, not by relaxing the check:

```rust
// AppState (apps/desktop/src-tauri/src/lib.rs:96)
/// Folders being peeked at. Read access for this session only, never persisted, and
/// never recursive: a grant covers files whose *parent* is the granted folder, so
/// peeking at ~/Desktop does not expose ~/Desktop/Private/diary.jpg.
peeks: Mutex<HashMap<String, Arc<Mutex<Library>>>>,
```

In `serve_photo`, a request is allowed if it is inside an added source **or** its parent
directory is exactly a granted peek folder. `canon.parent() == Some(granted)`, not
`starts_with` — `starts_with` would grant the whole subtree and undo the non-recursion.

### Commands

```rust
#[tauri::command] async fn peek_folder(state, path: String) -> R<PeekInfo>;
#[tauri::command] async fn peek_photos(state, path: String) -> R<Vec<PhotoInfo>>;
#[tauri::command] async fn end_peek(state, path: String) -> R<()>;
#[tauri::command] async fn promote_peek(app, state, path: String) -> R<SourceInfo>;

pub struct PeekInfo { path: String, name: String, photos: usize, videos: usize,
                      /// Subfolders present but not shown, so the window can offer
                      /// "Include subfolders" without having walked them.
                      subfolders: usize }
```

`peek_photos` returns the same `PhotoInfo` (`lib.rs:245`) the grid already consumes, with
`path` relative to the peeked folder — `hydrate` (`app.js:15`) derives `name`, `folder`
and `ext` from it, and `photoUrl` (`app.js:45`) prefixes `S.source`, so **the grid and
lightbox need no changes to render a peek**.

### Ordering, and why it differs

Photographs are ordered by **filename**, not capture date. Sorting by date means reading
EXIF from every file before the first paint; on a Desktop of 300 images that is a visible
wait, and someone who double-clicked a file expects Finder's order. Capture dates are
still read per photograph for the lightbox caption. `promote_peek` re-sorts by date,
because a real source does.

### Stepping

The viewer already does this. `openViewer(list, index)` (`app.js:1292`) takes any ordered
list; `step(±1)` wraps; arrow keys are bound at `app.js:3981-3982`; the filmstrip renders
a ±30 window. A peek calls `openViewer` with the peeked folder's rows and the index of
the file that was opened. **Do not write a second viewer.**

### Rejected

- *Registering the peeked folder as a temporary source.* It would hit `source_conflict`
  (`lib.rs:601`) against real sources, appear in the sidebar, and risk being persisted by
  any `save_sources` call on another code path.
- *Relaxing `serve_photo` to allow any readable path.* That is the app's only boundary
  against a crafted `photo://` URL reading `~/.ssh`.

## Acceptance criteria

1. Double-clicking a `.jpg` in Finder with Blinkview chosen opens Blinkview showing that
   folder's photographs, with the double-clicked one open in the viewer.
2. Left and right arrows step through every photograph in that folder, wrapping at both
   ends; the filmstrip tracks the cursor.
3. Peeking writes nothing into the folder: no `.blinkview-id`, no `blinkview.json`, no
   cache directory. A `find`-based before/after diff of the folder is byte-identical.
4. Peeking a folder with subfolders shows only the files directly inside it, and reports
   the subfolder count without having walked them.
5. A `photo://` request for a file in a *subfolder* of a peeked folder is refused with
   403 while the peek is open.
6. A `photo://` request for a peeked file is refused with 403 after `end_peek`.
7. Closing the peek deletes its cache directory under `<cache root>/peek/`.
8. Rating, deleting, renaming, moving, editing, metadata writes and Organize are
   unavailable during a peek, each with a reason naming the peek — not a silent failure
   and not an error after the fact.
9. Opening a photograph that is already inside an added source opens **that library**,
   not a peek, positioned on that photograph.
10. "Keep this folder" promotes the peek to a real source: marker written, full recursive
    scan, watcher started, and the peek's cache removed.
11. Peeking a folder of 300 photographs paints its grid in under 2 s on the reference
    machine, having decoded no more than the thumbnails on screen.
12. Verified on macOS. Windows and Linux are configured but unverified, and STATUS says so.

## Tasks

- [ ] 1. `scan::scan_shallow`, and `Library::peek` / `end_peek` / `is_peek` on the peek
      cache root (touches: `scan.rs`, `library.rs`, `cache.rs`)
- [ ] 2. `AppState.peeks`, the four commands, and the parent-only grant in `serve_photo`
      (touches: `apps/desktop/src-tauri/src/lib.rs`)
- [ ] 3. Refuse every writing command during a peek, at the command layer, with a message
      naming the peek (touches: `lib.rs`)
- [ ] 4. `fileAssociations` + `RunEvent::Opened`. `lib.rs:3403` currently calls
      `.run(tauri::generate_context!())`; it must become `.build(...)?.run(|app, event|)`
      to receive the event (touches: `tauri.conf.json`, `lib.rs`)
- [ ] 5. Window: peek banner with the folder name, "Keep this folder", and dropping an
      unadded folder peeks rather than adds (touches: `app.js`, `index.html`, `app.css`)
- [ ] 6. Tests: shallow scan depth, nothing written, the 403 boundary both ways, cache
      removed on close, an in-source photograph opening its library
      (touches: `tests/lifecycle.rs`, `lib.rs` unit tests)
- [ ] 7. ADR-0020 (peek vs added: two commitment levels, one library type); STATUS,
      ARCHITECTURE, README, landing page
