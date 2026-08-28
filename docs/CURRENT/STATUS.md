# Status

_Last updated: 2026-08-27_

## Current work — desktop app

`apps/desktop` runs. Multi-folder sources (remembered across launches), date-grouped
justified grid, lightbox with folder/person context, in-app people review with live
re-suggestion, selection with context menu, delete to a recoverable `Trash/`, per-photo
rename, per-person untagging, and the organize sheet (preview then apply).

Run it: `cargo run -p openfoto-desktop`.

Deleting moves photos to a `Trash/` folder **inside the library**, not the system
Trash, so it is journalled and undoable like every other operation. The sidebar shows
Trash with a count, selections there offer Restore, and a separate "Empty…" hands the
files to the macOS Trash — the one step openfoto cannot undo, which is why it is
explicit and confirms first.

Videos are indexed, get poster frames via ffmpeg when it is installed, and play in the
lightbox. Without ffmpeg they simply have no thumbnail rather than failing the pass.

### Editing
Rotate, flip, straighten (with auto-trim of the blank corners), crop with handles and
aspect presets, and brightness/contrast/saturation. Nothing is written until Save, which
asks whether to keep the original — keeping it is the default and moves the untouched
file to a visible `Originals/` folder. See ADR-0006.

### Formats
JPEG, PNG and **HEIC** for photos; MP4/MOV/M4V for video. HEIC is transcoded by macOS
`sips` and cached — thumbnails to the usual cache, full-size views to
`.openfoto/derived/<hash>.jpg` on first open. Verified against 12 real iPhone files:
they scan, thumbnail and display. WKWebView genuinely cannot show HEIC (an `<img>`
reports `naturalWidth: 0`), which is why the transcode exists. See ADR-0005 — this is
the project's only macOS-only dependency.

Folders can be added by dragging them onto the window, or with the ＋ button.

### Scale (measured on 20,000 distinct photos, debug build)

| Operation | Time |
|---|---|
| `scan` (BLAKE3 + EXIF) | 5.5s |
| `thumbs` (20,000 built) | 49s |
| `dedupe` | **13.3s**, from 14m49s |

The grid is virtualised: layout is computed for every photo but DOM exists only for
rows near the viewport — 20,000 photos render with ~55 cells mounted. Thumbnails are
produced on demand by the `photo://` handler as cells scroll into view, so nothing
blocks first paint; a background pass backfills.

The dedupe speedup was **not** the O(n^2) pair scan, which is what it looked like.
`rmse` normalised both thumbnails and allocated two 1024-element vectors on every
call, so each image was re-normalised once per candidate pair it appeared in.
Hoisting `normalize` out and abandoning comparisons that cannot beat the threshold
took it from 15 minutes to 13 seconds without touching the pair count.

Two caveats on that number. The synthetic fixture is pathologically self-similar
(5,779 groups from 20,000 photos, against 238 from 2,362 real ones), so it is a worst
case. And its counts shifted very slightly (5778->5779 groups) because `close` now
consults the verified candidate set rather than recomputing RMSE without the Hamming
gate, and because chunked accumulation orders floats differently — both can flip a
pair sitting exactly on the threshold. Real photos give identical results.

### Progress reporting
The four slow operations — face detection, thumbnails, face grouping, duplicate
analysis — report `(done, total)` through `progress::Counter`. The app shows a bar in
its toast; the CLI rewrites a single stderr line. Updates are throttled to ~100 per run
regardless of library size, and counted atomically so parallel work never reports a
count going backwards.

### Models
`openfoto models fetch` downloads the two ONNX models into `~/.cache/openfoto/models`,
and the app offers the same when they are missing. Downloads come from the LFS *media*
endpoint — `raw.githubusercontent.com` returns a 133-byte pointer that loads as a
corrupt model — and are verified against a pinned SHA-256 before install, because a
different model silently invalidates every threshold in ADR-0003 and ADR-0004. Files
land in `.part` and are renamed only after verification.

### Build size
The desktop crate emitted `staticlib` + `cdylib` targets for Tauri mobile that are never
linked (a 532MB `.a` alone), and full debug info across the ort/onnxruntime/wry tree.
Together they took `target/` to **12GB and filled the disk mid-session**. Restricting the
crate to `rlib` and setting `debug = "line-tables-only"` brought it to **2.4GB**.

### Bugs found by driving the real window
Each of these looked fine in code and only appeared on screen:
- `display` in a class rule outranks the `hidden` attribute, so every overlay was
  permanently visible until `[hidden]{display:none!important}` was added.
- Tauri's `asset:` scope glob `**` does not match an absolute path, so every image
  404'd. Replaced with our own `photo://` scheme that only serves inside added sources.
- Synchronous `#[tauri::command]` functions run on the UI thread; building 280
  thumbnails froze the window until the heavy commands were made `async`.
- `backdrop-filter` on the lightbox created a compositing context in WKWebView that
  swallowed the photo entirely — it measured correctly and never painted.
- With a few hundred filmstrip thumbnails in a sibling row, WKWebView laid the image
  out hundreds of pixels below its own `overflow:hidden` parent. Fixed with absolute
  centring plus windowing the filmstrip.

## Previously
Phase 1 done bar polish. Working today: `scan`, `status`, `dedupe`, `rename`, `undo`,
`history`. 32 tests pass (`cargo test --workspace`), clippy clean with `-D warnings`
across all targets.

Verified end-to-end against 25 real photos copied from the reference library:
scan (25 hashed) -> rescan (25 unchanged, fast path) -> rename --apply -> re-run plans
nothing -> undo restores every original name. A folder renamed outside the tool mid-session
was re-identified by content hash: 12 moved, 0 lost.

## Performance (measured, release build, M-series, 200 real photos)
- `scan` (BLAKE3 + EXIF): ~6s / 200 photos, single-threaded.
- `dedupe` cold (decode + signature + cluster): **1.84s / 200 photos** — about 22s for
  a 2500-photo library, roughly 6x faster than the Python prototype.
- `dedupe` warm: ~0.02s. Signatures are cached by content hash, so they survive renames
  and moves and are never recomputed.
- Build profile matters enormously here: the same cold dedupe takes 28.7s in a debug
  build. Benchmark with `--release` or the numbers are meaningless.

## Face assignment accuracy (measured)
120 real photos across three people, solo shots only as ground truth (a group photo
filed under one person also contains other faces, so it cannot label a face):

| References per person | Correct | Wrong | Left in place |
|---|---|---|---|
| 3  | 83% | 0% | 15 |
| 5  | 83% | 2% | 12 |
| 10 | 90% | 3% |  5 |

More references raise recall, as ADR-0003 predicts — a person needs coverage across
poses, not a single canonical face. Errors stay low and the system prefers to leave a
face alone, which is the intended bias. Reproduce with
`cargo run --release --example eval_faces -- <library> <seeds>`.

## Known issues
- Lightbox zoom is transform-based, so at very high zoom the browser upscales the
  already-decoded bitmap rather than re-decoding at native resolution.
- `scenery` is not implemented — spec task 11.
- `faces` cannot yet *move* photos into per-person folders; review teaches identities
  but filing them is still to come.
- The review page holds every face crop as an inlined data URI: ~800KB for 15 clusters.
  A library with hundreds of unnamed groups needs crops served on demand instead.
- Model files are not committed (37MB). `openfoto models fetch` is not implemented yet;
  place them in `models/` or set `OPENFOTO_MODELS`.
- Detection uses the YuNet `2026may` export, not the `2023mar` one the thresholds were
  tuned against (ADR-0004). The 4% scenery ratio in particular is still unconfirmed
  against the new export.
- Two `saurabh -> Me` misassignments persist across seed counts. Not yet established
  whether these are matcher errors or mislabels in the fixture, which was itself
  produced by a semi-automatic process.
- Model files are not committed (37MB). `openfoto models fetch` is not implemented yet;
  for now place them in `models/` or set `OPENFOTO_MODELS`.
- Candidate generation in `dedupe` is O(n^2) over dHash. Fine to ~10k photos; a
  100k-photo library needs a BK-tree or LSH bucket step.
- No thumbnail cache yet; `.openfoto/thumbs/` is created but unused.
- `rename` rewrites the whole library in one plan; no per-folder scoping yet.
- Rollback on a partly-applied plan is best-effort: if the reverse move also fails
  (disk full, volume unmounted), the library is left mid-plan. The journal records
  only fully-applied plans, so such a state is not undoable via `undo`.

## Recently shipped
- 2026-08-27 Repo bootstrapped; ADR-0001..0003 and the v1 spec written.
- 2026-08-27 `openfoto-core`: library/index/scan/plan/journal/fsops/rename/timesource.
- 2026-08-27 `openfoto` CLI: scan, status, rename, undo, history. Nothing mutates
  without `--apply`.
- 2026-08-27 Task 10: `openfoto faces review` — a dark, photo-first review page served
  from localhost, with live re-suggestion. Naming three people cut the remaining review
  from 15 clusters / 110 faces to 6 / 31.
- 2026-08-27 Task 9: people.json, discriminative assignment, EXIF orientation fix.
- 2026-08-27 Task 8: YuNet detection + SFace embeddings via `ort`, with landmark
  alignment. Verified against OpenCV: identical embedding on a shared crop (cosine
  1.000000), 0.9956 across the full pipeline, matching face counts and 0.990 box IoU.
- 2026-08-27 Task 6: perceptual signatures (dHash + normalized RMSE + Laplacian
  sharpness), complete-linkage clustering, `dedupe`. Verified on 22 real burst photos
  drawn from six known groups: all six recovered, no cross-day grouping.

## Corrections
An early probe reported that only 36 of 119 photos carried EXIF timestamps, which nearly
drove filename-first time resolution. It was reading the primary IFD; `DateTimeOriginal`
lives in the Exif sub-IFD (0x8769). A correct 300-photo sample shows **100%** carry it,
disagreeing with the camera filename in 13% of cases by exactly one second. EXIF is
authoritative. See ADR-0003.

## Origin
The workflow this tool automates was first executed by hand against a real 2519-file
library (`/Volumes/Notdefined/Swissgreece`). That library and its CSV manifests in
`tests/fixtures/` are the ground truth for regression tests. `reference/prototype/` holds
the original Python implementation as executable documentation.
