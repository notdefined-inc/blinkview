# Status

_Last updated: 2026-08-28_

## Current work — desktop app

`apps/desktop` runs. Multi-folder sources (remembered across launches), date-grouped
justified grid, lightbox with folder/person context, in-app people review with live
re-suggestion, selection with context menu, delete to a recoverable `Trash/`, per-photo
rename, per-person untagging, and the organize sheet (preview then apply).

The visual layer is **Aurora Glass** (docs/CURRENT/DESIGN.md, spec
docs/SPECS/done/2026-08-28-aurora-glass-ui.md): an ambient cyan/violet/amber gradient
canvas under frosted panels, the lens mark as inline SVG, and the **Ask panel** (✨ in
the titlebar or ⌘K) — natural-language questions parsed like the omnibar, answered with
intent chips, people faces, a thumbnail strip and one-tap actions (Show in library,
Select results, Save this search…). It composes the existing commands only; the thread is
per-session and nothing is persisted. Native `prompt()`/`confirm()` are gone — glass
dialogs instead.

Run it: `cargo run -p openfoto-desktop`.

Deleting moves photos to a `Trash/` folder **inside the library**, not the system
Trash, so it is journalled and undoable like every other operation. The sidebar shows
Trash with a count, selections there offer Restore, and a separate "Empty…" hands the
files to the macOS Trash — the one step openfoto cannot undo, which is why it is
explicit and confirms first.

Videos are indexed, get poster frames via ffmpeg when it is installed, and play in the
lightbox. Without ffmpeg they simply have no thumbnail rather than failing the pass.

### Where your data lives
`openfoto.json` (ratings, labels, saved searches) and `openfoto-people.json` (names) sit
at the library root, not in `.openfoto/` — and since ADR-0010 an `openfoto.json` may
also sit in **any folder**, with the nearest one winning. Writes land in the folder that
holds the photograph, which is what makes copying a folder in Finder carry its ratings
with it. Deleting the cache loses nothing you authored — there
is a test that writes a name and a rating, deletes `.openfoto/` outright, and asserts
both survive. Libraries written by an earlier version are migrated when opened.

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

### Search
The search field parses a query into filters that combine: a date in any combination
("august", "23 aug 2026", "2026-08-23"), a person, a rating, a colour label,
a type, and free text. Chips under the field show how the query was read, so a search
that matches nothing is never a black box.

Free text that is not a filename or folder is also matched **semantically** — against
what the photograph shows rather than what it is called. "a church" finds the church
interior, "green trees" finds foliage, "a laptop computer" finds the desk shots. It
combines with everything else: `a church sam 18 august 2026` intersects scene, person
and date.

Below the 0.18 threshold a query returns nothing rather than the least-bad photograph;
"the sea" against a library with no sea is answered "nothing recognised". A library
that has not been embedded yet offers **Understand these photos** in place of silence,
and missing models say so rather than reporting an empty result.

Focusing an empty field shows who is in the library as faces, and a row of scene
suggestions — a face is easier to recognise than a name is to recall, and nobody
guesses they can type "a church" unless shown. Scene suggestions appear only once the
library has been embedded, since offering a search that cannot run is worse than
offering none.

Embedding runs once per photo, is resumable, and costs about 100 ms per photo. The
text encoder is held open for the life of the window: loading it costs ~270 ms against
~15 ms to embed a phrase, so a fresh load per keystroke would dominate the search.

### Asking, and telling
The Ask panel answers questions *and* carries out instructions. "move the swiss photos
to Trip/Alps" resolves the selector, shows what it is about to do, and does nothing
until the button is pressed; ⌘Z undoes it like any other change.

This is a grammar, not a model (ADR-0012) — `verb selector [to target]`, over the same
`parseQuery` the search field uses, so there is one selector language rather than two.
Verbs are move, rate, label, delete, show and save, matched through a synonym table, and
what a verb does not recognise falls through to scene search.

Three rules keep it safe without a model's judgement:

- **Nothing acts without a preview.** Every command compiles to a `Plan`, and the card
  listing it is the only route to disk. Deleting is a preview like everything else.
- **A missing slot is a question.** "move the swiss photos" with no destination asks
  where, and the answer completes the original instruction.
- **An empty selector is refused.** "move to Trip" does not quietly inherit whatever was
  last on screen; saying "them" does that, deliberately.

Clauses compose — "show the greece day1 photos and rate them 5 stars" runs as two steps,
with "them" bound to what the first found. A mistyped verb is corrected by edit distance
rather than refused, but only when the sentence is shaped like an instruction, so
"movie night photos" stays a search.

The honest limit is phrasing coverage: an unknown wording fails rather than being
guessed at. That is the trade ADR-0012 accepts, and the first response to a phrasing
someone expected to work is a synonym-table entry.

### Folders are the only grouping
There are no albums (ADR-0009). A folder is where a photograph lives, nested folders are
what albums were trying to be — `Trip/Greece Day3/` is how people organise photographs
without any app at all — and cross-cutting views are saved searches.

Selecting a folder shows everything beneath it. The sidebar is a tree with recursive
counts and remembered expansion; the grid can section by subfolder as well as by date,
so standing in `Trip` gives `Greece Day1`, `Greece Day2` and `Swiss Day1` in one scroll.

A library that still has albums is offered a migration: each album becomes a folder,
previewed first and undoable afterwards. Album names were free text and folder names are
not, so reserved characters are replaced and the change is reported. A photograph in two
albums can only live in one folder, so it goes to the first and the rest are listed
rather than guessed at.

**Saved searches** replace what albums were used for across folders. Only the query is
stored, so they stay current as photographs are added. They live in the root
`openfoto.json`; a folder describes its photographs, not how the library is searched.

Person names are matched as whole phrases before the query is tokenised, so a person
called "Anna Maria" is one name rather than two stray words.

### The cache looks after itself
`.openfoto/` is disposable *and* self-correcting (ADR-0011). Libraries scan on open, so
photographs added or reorganised in Finder appear without anyone pressing anything — the
size and mtime fast path keeps that cheap.

Open libraries are also **watched** (FSEvents via `notify`), so photographs arriving
while the window is open show up on their own. Events are debounced: pasting 40 files
produces one rescan, not forty. Anything inside `.openfoto/` is ignored, since reacting
to our own thumbnail and index writes would rescan in a loop. A corrupt index is detected and rebuilt
silently, because nothing user-authored is in it.

The check is `PRAGMA quick_check` plus a count of each table. `quick_check` alone is not
enough, and measurably so: scribbling 512 bytes over a page body still returns "ok",
since it validates b-tree structure rather than page contents.

Embeddings and face detections are keyed by content hash in their own tables and outlive
the file rows that referenced them, so a photograph that disappears and comes back is
not re-analysed. Orphans are only reaped on an explicit vacuum — automatic reaping would
destroy exactly that property.

### Removing a folder
Each source has a visible ✕ rather than a right-click nobody finds. It asks first, and
the dialog leads with the thing being feared: **your photographs are not deleted** — the
folder and everything in it stays where it is, it simply stops appearing in openfoto.

The same dialog offers to delete what openfoto itself wrote, unchecked. It is never the
default because it covers two unlike things and says so: the cache costs a rescan, while
ratings, labels, saved searches and names cannot be reproduced by anything (ADR-0007).
The counts are real — "0.5 MB of thumbnails and index, which would be rebuilt · and 2
rated or labelled, which cannot be recovered" — so the cost is visible before it is paid.

Verified both ways on a real folder: removing plainly left all twelve photographs, the
cache and the metadata; removing with the option ticked left all twelve photographs and
nothing else of openfoto's.

### Stepping through the viewer
Arrow keys walk **exactly what is on screen, in the order it is shown** — which, with a
folder selected, means that folder and everything beneath it.

This used to fall back to the photograph's own folder when nothing was filtered, which
was the Picasa rule. It stopped fitting once the grid began rolling up subfolders:
clicking a clip in a mixed, date-sorted grid walked `WhatsApp Video` — thirty-seven
videos and no photographs — so the arrows appeared to skip every picture. Picasa's grid
*was* per-folder, so "the folder" and "what you are looking at" were the same thing
there; only the first half of that rule survived the move to rolled-up folders.

Both scopes are still wanted, so the viewer says which one it is in and offers the
other. Standing in `Trip` on a picture from `Greece Day3`, **▣ this folder** narrows to
it — including its own children, because a folder always means itself and what is inside
it (ADR-0009) — and **↔ all** goes back. `f` toggles. The control only appears when the
two scopes are actually different sets.

### Selection and people
Selection works the way a file manager's does: click, shift-click for a range, ⌘A for
everything, and **shift with the arrow keys** to extend from the last photograph touched
— stepping up or down crosses the row the cursor is actually in, which varies, since a
day holding two photographs is a two-wide row. A **date or folder heading selects its
whole group**, and selects it off again.

A selection can be moved straight into a folder from its context menu, existing folders
offered first, previewed and undoable like every other move.

People are listed only while they match photographs. A name matching nothing cannot be
browsed to anything, so it is collapsed into one removable row rather than sitting in
the list claiming zero — and untagging a person's last photograph forgets them outright,
since the user has just finished saying none of these are them. Any person can be
forgotten deliberately; the photographs are untouched.

Naming an unrecognised face offers the people already known, one click each. Retyping a
name risks a second spelling of someone who is already there, and merging is usually
what was meant.

### Where the time goes
Measured on a 25GB phone backup with `examples/bench`, mean 7.8 MP:

| | per photo |
|---|---|
| face detection (decode + shrink + detect) | 98 ms |
| — of which inference at 1280px | **15 ms** |
| semantic embedding (decode + embed) | 124 ms |
| — of which inference at 256px | **33 ms** |

**The models are not the cost; the JPEG decode is.** Detecting faces spends 85% of its
time turning twelve megapixels into pixels it immediately shrinks to 1280.

So analysis is **one pass** (ADR-0013): decode each photograph once, and take the
thumbnail, the faces and the embedding from that frame. A photograph needing nothing is
never opened; one needing only a thumbnail uses the camera's embedded preview instead of
a decode. Measured against running the three passes separately, `examples/passes`:

| | per photo | 200,000 photographs |
|---|---|---|
| three separate passes | 262.9 ms | 14.6 h |
| **one pass** | **86.9 ms** | **4.8 h** |

That is 3.0x, and it comes from doing less rather than from more threads — the pass runs
four photographs at a time, not one per core, because ONNX Runtime already threads a
single inference. End to end on a 40-photograph library the release binary takes 4.4s
at 459% CPU, producing every thumbnail, face and embedding.

The equivalence is tested rather than assumed: `tests/analyze_pass.rs` runs both the old
passes and the new one over the same photographs and requires the same face boxes to
within a pixel and the same embeddings to cosine 0.9999 — because ADR-0003 and ADR-0008
fixed their thresholds against those exact outputs.

Two things that do *not* help, both measured rather than assumed:

- **A faster JPEG decoder.** `image` already uses zune-jpeg, and turbojpeg came in at
  60.4 ms against 60.1 ms on 12 MP. Asking for a scaled decode saves only 22%, because
  the entropy coding has to be walked in full whatever size you want out.
- **CoreML.** Slower than CPU on these models: YuNet 32.6 ms against 18.5 ms, MobileCLIP
  42.4 ms against 32.6 ms. They are small enough that partitioning and transfer cost
  more than the acceleration returns. DirectML and CUDA are worth measuring on their own
  platforms before being believed.

### How that compares
Immich publishes whole-library wall clock, so the same unit is used here. Their figures
are user-reported rather than controlled, on other hardware, and at least one thread
mentions a GPU — treat them as a league table, not a photo finish.

| Embedding 80,000 assets | wall clock |
|---|---|
| Immich `ViT-B-32__laion2b_e16` | 80 min |
| **openfoto, parallel (not yet shipped)** | **110 min** |
| **openfoto, as it ships today** | **194 min** |
| Immich `ViT-B-16-SigLIP-384__webli` | 270 min |

So: the same league, faster than their heavier model, behind their light one. Not slow,
and not fast enough to leave alone. Worth noting we run a much smaller model
(MobileCLIP-S0) for those numbers, which means the pipeline around it, not the model, is
where the remaining time sits.

Parallelising the two ML passes is worth about 1.8x on eight cores — less than the core
count, because ONNX Runtime already threads a single inference internally. Face
detection and embedding both still loop one photograph at a time; only thumbnails use
rayon.

### Switching source at scale
A source switch sends every photograph to the window, so the payload *is* the cost. It
was 513 bytes per photograph; it is now 175.

- `thumb` was never read by the frontend — a hundred bytes of dead weight each.
- `path` is relative to the library. The window prepends the source it asked about
  rather than being told the same prefix a hundred thousand times.
- `name`, `folder` and `ext` all live inside `path` and are split out on arrival. That
  is a string split per photograph, against megabytes on the wire.
- Absent things are omitted rather than sent as defaults — no rating, no label, no
  albums, no people, no faces.

Measured across the bridge at real size with `bench_payload`, rather than extrapolated:

| photographs | bridge | deriving the split fields |
|---|---|---|
| 20,000 | 89 ms | 5 ms |
| 100,000 | 410 ms | 10 ms |
| 200,000 | **794 ms** | 32 ms |

The Rust side is not the cost — building and serialising 2,433 photographs takes 4.8 ms,
of which serialisation is 0.9 ms. Reading the whole folder tree for `openfoto.json` costs
200 ms, but only the first time a library is opened.

An earlier projection of 3.8 s for a 200,000-photograph switch was extrapolated from a
measurement taken *before* the metadata cascade was cached — a number that had already
stopped existing.

### Progress reporting
The four slow operations — face detection, thumbnails, face grouping, duplicate
analysis — report `(done, total)` through `progress::Counter`. The app shows a bar in
its toast; the CLI rewrites a single stderr line. Updates are throttled to ~100 per run
regardless of library size, and counted atomically so parallel work never reports a
count going backwards.

### Models
`openfoto models fetch` downloads four ONNX models into `~/.cache/openfoto/models`
(YuNet and SFace for faces, MobileCLIP-S0 vision and text for search, 204 MB total),
and the app offers the same when they are missing. Downloads come from the LFS *media*
endpoint — `raw.githubusercontent.com` returns a 133-byte pointer that loads as a
corrupt model — and are verified against a pinned SHA-256 before install, because a
different model silently invalidates every threshold in ADR-0003, ADR-0004 and ADR-0008.
Files land in `.part` and are renamed only after verification, and an installed file
whose hash no longer matches its spec is re-downloaded rather than trusted.

Both CLIP towers are fp32. The int8 text tower is four times smaller and measurably
wrong: its vectors diverge from fp32 by cosine 0.89, it nearly doubles the matches
clearing the threshold, and it is not reproducible between onnxruntime builds, which
makes the ADR-0004 parity rule unsatisfiable. ADR-0008 has the measurements.

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
