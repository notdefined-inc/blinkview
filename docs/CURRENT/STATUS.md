# Status

_Last updated: 2026-08-30_

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

Counts follow what you can see. A photograph in Trash is counted by the Trash row and
by nothing else: it used to be counted in the library total as well, so the grid hid it
while the sidebar went on including it, and deleting appeared to change nothing. On the
reference phone backup that was 115 photographs and clips counted twice. The sidebar
also refreshes on the delete itself rather than on the next click of the source —
re-reading committed index state, not rescanning, measured at **79 ms** across all
three reference libraries (3,699 indexed files).

Emptying the Trash works across filesystems. `rename` is a metadata hop and cannot
leave the volume, and `~/.Trash` is on the boot disk, so a library on an external drive
failed with EXDEV for every file and reported moving nothing at all. It falls back to
copy-then-remove, which ends in the same place.

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
aspect presets (the crop reports the pixel size that will actually be written, live as
it is dragged), and brightness/contrast/saturation. Five named presets — Mono, Warm,
Cool, Punch, Faded — are starting points rather than modes: each only sets the three
sliders, which stay editable. They are defined in core, and a test reads `app.js` to
assert the window agrees, so "Warm" cannot come to mean two things.

A whole selection can be coloured at once from the context menu. Nothing is written
until Save, which asks whether to keep the original — keeping it is the default and
moves the untouched file to a visible `Originals/` folder. See ADR-0006.

**Metadata can be read and removed.** The info panel shows camera, lens, exposure, ISO
and whether there are coordinates in the file, because "does this say where I live" is
the question people actually have. Stripping removes EXIF, XMP, IPTC, maker notes and
comments while keeping JFIF, ICC colour profiles and Adobe's colour marker — those
decide how the photograph looks, and stripping metadata is not meant to change the
colours. It is a segment rewrite, never a re-encode: the entropy-coded scan data is
copied byte for byte, so the decoded pixels are bit-identical (verified). HEIC and video
are refused by name. The original is kept by default, because `taken_at` comes from EXIF
(ADR-0003) and a stripped photograph falls back to its filename or mtime for a date —
see ADR-0015.

Anything that rewrites a photograph changes its content hash, and ratings and labels are
keyed by that hash (ADR-0007), so they are carried across explicitly. Until 2026-08-30
they were not: editing silently discarded the rating, the label and any album
membership, and had done since editing existed. ADR-0015 has the account.

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

### Scrolling fast

WKWebView does not reliably cache custom-scheme responses, and cells are rebuilt on
re-entry, so until late August 2026 every scroll-back re-fetched each thumbnail it
could already see — the flicker, and the wait. The handler now keeps a 64 MiB
byte-budgeted LRU over thumbnails and previews, so a scroll-back serves from RAM
without touching the filesystem. Cells also ask for their image only as they approach
the viewport: the lazy-load IntersectionObserver existed but `io.observe` was never
called, so a fast flick fired requests for 1,800 px of rows nobody would see, all
queuing on the image pool. Video posters are built by the analysis pass (it used to
filter to `kind == "photo"` for every stage, leaving 173 of 507 posters missing on a
real backup, each paid mid-scroll), and the rare on-demand render routes to its own
two-thread pool so it can never occupy a photo-decode thread. The lightbox steps
through a derived 2000 px JPEG instead of decoding a 12–48 MP original per keypress.
Cells show a shimmer while loading and all new animation is transform/opacity only.
Spec: docs/SPECS/done/2026-08-30-thumbnail-performance.md.

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

⌘F focuses the field — there is no separate finder, because the field already matches
filename and path. A query is ANDed with the folder you are standing in, which is what
makes it a filter rather than a jump; when that hides matches, a chip counts them
("2 elsewhere — search all of lib") and clears the narrowing in one click.

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

### Where photographs were taken
A map view (the pin in the titlebar) draws every located photograph, clustered, with the
place name under the cursor. **It never fetches a tile.** Every other photo app streams
raster tiles, which tells a tile server where its users have been on every pan — for a
library whose premise is that nothing leaves the machine (ADR-0001), that is the one
leak that would undo the premise. The basemap is Natural Earth outlines bundled at two
levels of detail (149 KB and 1.4 MB), projected Web Mercator onto a canvas. The price is
honest: no streets at high zoom, coastlines and pins instead. The upside is that there
is nothing to wait for.

Coordinates are read once and cached in the index against the content hash, including
the answer "none" — otherwise every map open would re-read every screenshot in the
library. A second pass over an unchanged library takes 3 ms.

Place names come from `crates/openfoto-core/data/places.bin`: 170,860 places from
GeoNames cities1000, packed to 4.35 MB with region and country names interned and
coordinates as integer degrees × 10⁴. Both spellings are searched, so "reykjavik" finds
"Reykjavík" — before that, "Fira" reached Firavitoba in Colombia rather than Firá on
Santorini. Nothing here touches the network. Attribution (GeoNames CC BY 4.0, Natural
Earth) is shown on the map, as the licence requires, and `tools/build-geodata.sh` is the
only way the data is produced.

A photograph with no coordinates can be given some: type a town, and the location is
written into the file itself. That rebuilds the EXIF rather than appending a second APP1
segment — the TIFF is parsed into its entries, the GPS directory replaced, and the whole
block re-serialised with recomputed offsets, keeping the camera, the date and the
embedded thumbnail. Because that is the one operation here that could corrupt a
photograph, it is not trusted: **the rewritten file is read back and re-parsed before it
replaces the original**, and a file that does not read back as what was just written is
left exactly as it was. Verified on a real 4.2 MB phone photograph — camera, model and
`DateTimeOriginal` intact, pixels unchanged, 126 bytes larger. JPEG only; HEIC and video
are refused by name.

### Folders are the only grouping
There are no albums (ADR-0009). A folder is where a photograph lives, nested folders are
what albums were trying to be — `Trip/Greece Day3/` is how people organise photographs
without any app at all — and cross-cutting views are saved searches.

Selecting a folder shows everything beneath it. The sidebar is a tree with recursive
counts and remembered expansion; the grid can section by subfolder as well as by date,
so standing in `Trip` gives `Greece Day1`, `Greece Day2` and `Swiss Day1` in one scroll.
A ＋ on the Folders heading makes an empty folder inside the selected one — folders are
the only grouping there is, so making one before there is anything to put in it is how
you say where things are going to go. Empty folders are listed from disk, since a tree
derived from the index cannot see them.

**Each folder remembers how it is arranged.** The sort — newest, oldest, name, rating,
size, or a custom order you drag by hand — lives in that folder's own `openfoto.json`,
beside the ratings of the photographs it holds, so it survives a relaunch and travels
with the folder when it is copied in Finder. It is read from that folder alone and
never inherited: an arrangement is about the folder, unlike a rating, which is about a
photograph. A custom arrangement draws as one run, because grouping it by day would
re-sort what was placed by hand, and photographs added later fall in at the end rather
than vanishing. Arranging is offered only over a whole folder, never a search result or
a person — there is nowhere honest to record the order of a slice of several folders.

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

### Adding a folder
The row appears immediately and indexes behind itself. `add_source` registers the path
and returns without opening the library, because the first scan of a phone backup takes
minutes and waiting for it before even showing the folder is what made adding one feel
like a hang.

A folder is refused if it overlaps a source you already have — in either direction.
Every source is an independent library with its own `.openfoto/`, so a subfolder added
as its own source would be indexed twice, analysed twice, and removing either copy
could delete `openfoto.json` metadata the other still reads; adding the parent of an
existing source has the same problems in reverse. Re-adding a folder that is already a
source is refused too, so the window says "already in your library" instead of
pretending to add it. Paths are compared canonicalized, so the same folder through a
symlink cannot slip through as two sources. Refusals appear as an error toast; the
drag-and-drop path reports them the same way.

Scanning reports real progress. It walks twice — once reading only directory entries to
learn the total, then again to do the work — because names are cheap next to hashing
contents, and without a total there is only a spinner, which says nothing about whether
to wait.

Progress belongs to a library, not to the window: each source draws its own bar on its
own row, and the banner only speaks for the operation it was opened for. Work on one
folder no longer narrates itself over another.

While a 25GB library rescans from scratch, queries against other sources return in
24-30 ms. They used to block until it finished, because `open_lib` scanned while holding
the lock every command for every library needs.

### Scroll position
Only navigation moves the scroll. A background refresh — a scan finding more
photographs, the watcher noticing a change — redraws the grid under someone who is
reading it, and jumping them to the top for that is maddening. Verified by scrolling
into a library, dropping a file into it from outside, and watching the count go 40 to 41
with the position unmoved.

### While a folder indexes
Switching to a library that is still indexing shows **its** photographs, filling in as
they are found — not the previous source's, and not an empty screen. The index is in
WAL mode and commits as it walks, so a second connection reads what has landed without
waiting on the writer. The sidebar lists every folder immediately with real counts,
including partial ones that climb as the scan proceeds.

Before this, switching left the last library's photographs on screen under the new
library's name, because the query blocked behind the scan while the grid kept what it
had.

Reads never queue behind a write. A library being analysed holds its lock for hours, so
a read tries the lock and, if it is busy, opens a second connection and serves the
committed state instead of waiting. Measured while face detection ran over 1,900
photographs: queries returned in 74-263 ms.

Each source row names the job running on it — "analysing 2%", "indexing 40%" — with the
exact counts on hover. A bar alone left it ambiguous whether a folder was being indexed
or having its faces detected. Scan progress waits 400 ms before appearing, because
rescanning an unchanged library takes 0.38 s and a bar for that reads as "indexing
again" when nothing was reindexed.

### Picking up where a session stopped
Analysis is resumable by construction — every stage commits per photograph — and it now
resumes on its own when a folder is opened, rather than waiting to be asked again.

Only stages already begun are resumed. A folder nobody has asked to analyse does not
start burning CPU by itself the next time the app opens; one that was half-analysed
finishes. Verified by killing a pass at 36 of 40 photographs and reopening the folder:
it completed to 40 without being asked, and a second open started nothing.

### Work belongs to its folder
A banner names the library it is about and is hidden while you are looking at another
one; the source row carries the progress instead. Analysis can be stopped: removing a
folder cancels whatever is running on it, checked per photograph, and adding it back
clears that so it can run again. Verified by removing a folder mid-pass — the embedding
count froze at 204 and stayed there — and re-adding it, which resumed.

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

When that happens anyway, **two named people can be merged**: references are
concatenated (a set, never a centroid — ADR-0003) and exclusions unioned, so the
correction makes recognition better. Forgetting one of them, the only option before,
threw its reference faces away.

**Not every face is someone to name.** A group can be set aside, which records the
faces — `"<photo hash>:<face index>"` — in `openfoto-people.json` rather than the
cluster, because a cluster's id is a position in a list recomputed on every pass. The
photographs are untouched and the faces stay in the index; the sidebar says how many are
set aside and brings them all back in one click. Dismissing deliberately learns nothing:
treating dismissed faces as a hidden identity would need a threshold, and that
threshold's failure mode is swallowing someone real.

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

### Memory (measured, release build, peak RSS sampled every 100ms)
Throughput was the only thing ever measured until a 1,926-photo phone backup drove an
8 GB Mac into swap. The numbers that matter:

| Run | Peak RSS |
|---|---|
| Idle process, no work | 13 MB |
| 250 photos, all stages | 1525 MB |
| 953 photos, all stages | 1596 MB |
| 953 photos, after the allocation fixes below | 1315 MB |
| 200 photos, 1 worker, one resolution | 507 MB |
| 200 photos, 1 worker, 33 resolutions | 646 MB |

Peak is flat in library size, so nothing leaks: the footprint is the work in flight.
What drives it is **allocator retention across image sizes**. macOS keeps freed large
blocks per size class, so a library of one resolution recycles a single block for ever
while a mixed library strands a region per size — during a pass `vmmap` showed 48.8M
live in `MALLOC_LARGE` against 348.0M dirty in 26 *empty* regions. `openfoto-demo` is
uniformly 4000x1848, which is exactly why dev testing never saw this.

Worker count is the lever, and it is now sized from physical memory rather than cores
alone — one worker per 4 GB, capped at four. On 226 photographs across 76 resolutions:

| Workers | Peak RSS | Elapsed |
|---|---|---|
| 1 | 1299 MB | 55s |
| 2 | 1407 MB | **50s** |
| 3 | 1520 MB | 63s |
| 4 | 1599 MB | 95s |

Four workers was the worst setting on both axes at once: most memory *and* slowest,
because an 8 GB machine swaps. The old sizing read core count alone and so handed this
machine four. With the memory-aware default it takes two and peaks at 1143 MB.
`OPENFOTO_WORKERS` still overrides, for a machine that wants less again.

**mimalloc was measured and rejected.** At one worker it is a clear win — six ABBA pairs,
every one negative, mean -211 MB (-23.5%), and order-independent. At the default worker
count it *loses*: four pairs, all positive, mean +122 MB. mimalloc gives each thread its
own heap and segments, so it costs memory exactly where the footprint is already largest.
A C dependency that makes the real workload 8% worse is not worth having.

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
- Nothing tells a new user the models exist until they reach a feature that needs them.
  `models_fetch` is offered from the People pane (`app.js:1852`) and the scene-search
  empty state (`app.js:3221`), but there is no first-run notice and no settings entry to
  download them later or see what is installed. Everything else works without them:
  verified on 25 photographs with the CLIP models absent — 25 thumbnails built, 0 clip
  rows, no error.
- ffmpeg is found by searching known install prefixes as well as `PATH`, because a
  Finder-launched .app inherits launchd's `/usr/bin:/bin:/usr/sbin:/sbin` and not a
  shell's. That covers Homebrew and MacPorts; an ffmpeg installed anywhere else is
  still invisible to the packaged app, and bundling it as a sidecar is the real fix.
- The app is ad-hoc signed but **not notarized**, which needs a paid Apple Developer ID.
  First launch therefore needs right-click → Open on macOS. `spctl` rejects it, as it
  rejects anything unnotarized; what matters is that the signature itself is valid, so
  macOS offers the "unverified developer" prompt rather than calling the app damaged.
- A pass over a library with many distinct resolutions still peaks around 1.1 GB on an
  8 GB machine. Allocator retention across image sizes is the underlying cause (see
  Memory above) and swapping the allocator does not fix it — mimalloc was measured and
  is worse at the default worker count. `OPENFOTO_WORKERS` lowers it further.
- The desktop app's `photo://` image pool is sized independently of the analysis
  workers (`clamp(2, 6)` off core count, `apps/desktop/src-tauri/src/lib.rs`), and every
  in-flight request holds a decoded full-size frame. During a pass the two pools
  compete without either knowing about the other, which is the likeliest remaining
  cause of sluggish scrolling while analysis runs. Video poster renders were moved to
  their own two-thread pool (2026-08-30) and no longer add to the competition; analysis
  workers vs image decodes does. Not yet measured.
- Lightbox zoom is transform-based, so at very high zoom the browser upscales the
  already-decoded bitmap rather than re-decoding at native resolution.
- `scenery` is not implemented — spec task 11.
- `faces` cannot yet *move* photos into per-person folders; review teaches identities
  but filing them is still to come.
- The review page holds every face crop as an inlined data URI: ~800KB for 15 clusters.
  A library with hundreds of unnamed groups needs crops served on demand instead.
- Detection uses the YuNet `2026may` export, not the `2023mar` one the thresholds were
  tuned against (ADR-0004). The 4% scenery ratio in particular is still unconfirmed
  against the new export.
- Two `Sam -> Me` misassignments persist across seed counts. Not yet established
  whether these are matcher errors or mislabels in the fixture, which was itself
  produced by a semi-automatic process.
- Candidate generation in `dedupe` is O(n^2) over dHash. Fine to ~10k photos; a
  100k-photo library needs a BK-tree or LSH bucket step.
- `rename` has no find-and-replace or case conversion: one date-and-counter pattern,
  previewed before it runs. Scope and pattern are both settable now.
- Rollback on a partly-applied plan is best-effort: if the reverse move also fails
  (disk full, volume unmounted), the library is left mid-plan. The journal records
  only fully-applied plans, so such a state is not undoable via `undo`.

## Recently shipped
- 2026-08-30 A map, and places. Photographs with EXIF GPS resolve to "City, Region,
  Country" from a bundled 4.35 MB table of 170,860 places, and are drawn as clusters on
  a canvas map built from bundled vector outlines — no tile is ever fetched, because a
  tile request would tell a server where the photographs were taken. Photographs with no
  coordinates can be given them by name, written into the file itself and read back
  before the write is kept. Spec:
  docs/SPECS/done/2026-08-30-places-and-the-map.md.
- 2026-08-30 Face review can be corrected in both directions it was missing. A group of
  faces can be set aside — on a 40-photograph sample, detection found 8 groups of which
  4 were singletons, which is what the sidebar filling with strangers looks like — and
  set-aside faces stay set aside across re-clustering, restart and a deleted cache.
  Restoring re-creates exactly the groups that were there. Two people who are the same
  person can be merged, keeping both sets of reference faces instead of throwing one
  away. Spec: docs/SPECS/done/2026-08-30-dismiss-and-merge.md.
- 2026-08-30 A pass over the things a photo manager is expected to have. ⌘F focuses the
  search field, and a chip says how many matches the current folder is hiding. Every
  folder remembers its sort, including a custom order dragged by hand, in its own
  `openfoto.json`. Folders can be made from the sidebar. Deleting can go to a folder of
  your choosing rather than only `Trash/`. Bulk rename takes a pattern and a scope — the
  selection, or the folder you are in — with `%%n` numbering in capture order. Colour
  presets apply to one photograph or a whole selection. Metadata is readable in the info
  panel and removable without re-encoding a single pixel. Specs:
  docs/SPECS/done/2026-08-30-{finding-and-arranging,organising-files,colour-and-metadata}.md.
  Along the way this found and fixed a silent data loss: **every in-place rewrite
  discarded the photograph's rating and label**, because they are keyed by content hash
  and the hash changes — true of editing since editing shipped (ADR-0015).
- 2026-08-30 Deleting reads honestly. Three faults, one symptom — the numbers not
  moving. The Trash row and the folder counts only refreshed on the next click of the
  source, because `deleteSelected` was the one mutation path that reloaded the grid
  without refreshing the sidebar. The library total counted trashed photographs, which
  the grid hides, so the count beside a folder stayed the same when you deleted from it
  (115 of them on the reference backup). And "Empty Trash" moved nothing at all for a
  library on an external drive: `std::fs::rename` cannot cross a filesystem, and every
  file failed with EXDEV against `~/.Trash` on the boot disk. Verified end to end on a
  fixture on the external volume — file physically in `~/.Trash`, library Trash empty —
  and against the real libraries for the counts.
- 2026-08-30 Adding a folder that overlaps a source you already have is refused with an
  explanation instead of silently double-indexing the photographs. Re-adding a folder,
  adding a subfolder of a source, and adding the parent of a source all fail with a
  toast saying why; paths are compared canonicalized so the same folder through a
  symlink counts as one folder. Previously a duplicate add was a silent no-op that
  still toasted "added", and a nested add quietly created a second library over
  photographs another library was already indexing and journalling.
- 2026-08-30 Scrolling got its cache back. WKWebView does not reliably cache `photo://`
  responses and grid cells are rebuilt on re-entry, so every scroll-back re-fetched each
  thumbnail — the flicker, and the wait. The scheme handler now holds a 64 MiB
  byte-budgeted LRU over thumbnails and previews; cells load lazily through the
  IntersectionObserver, which existed but was never wired (`io.observe` was dead code);
  the analysis pass builds video posters (it filtered to `kind == "photo"`, so 173 of
  507 posters were missing after analysis, each paid mid-scroll); on-demand poster
  renders route to a dedicated two-thread pool; and the lightbox steps through a derived
  2000 px JPEG (`?preview=`) instead of a 12–48 MP decode per keypress. Loading cells
  shimmer, and every new animation is compositor-only transform/opacity under the
  existing `prefers-reduced-motion` kill. Spec:
  docs/SPECS/done/2026-08-30-thumbnail-performance.md.
- 2026-08-30 The desktop app bundles its own ffmpeg (ADR-0014), so video support no
  longer depends on what the host has installed or on a GUI app inheriting a shell's
  PATH. Built from pinned, checksummed sources by `tools/build-ffmpeg.sh`: 9.6 MB on
  arm64 macOS and 14.4 MB on x86_64 Linux, against 49.7 MB for a full static build of
  the same version, and carrying every container and codec a phone or camera produces.
  `tools/check-ffmpeg.sh` asserts that against the binary on every CI run.
- 2026-08-30 A failed video thumbnail no longer serves the whole clip to the webview.
  On a 507-video phone backup that put 15.7 GB of MP4 into the render process, which
  macOS killed and restarted — the window going black mid-analysis, then re-requesting
  the same thumbnails. Measured at 9.59 GB in `tauri://localhost` against 618 MB for
  the Rust side.
- 2026-08-30 The macOS bundle is signed. The first v0.1.0 build shipped with no
  `_CodeSignature` at all — tauri-bundler only signs when an identity is configured —
  and Gatekeeper reports an invalid signature as *damaged*, which right-click → Open
  cannot clear. `signingIdentity: "-"` fixes it; verified on the published .dmg.
- 2026-08-30 The remove-source button works during an analysis pass. `source_data` runs
  before the confirmation dialog and took the blocking library lock, so the click did
  nothing for the length of the pass and the dialog then appeared all at once.
- 2026-08-30 Analysis workers are sized from physical memory (one per 4 GB, capped at
  four) rather than core count alone. An 8 GB machine now takes two instead of four and
  peaks at 1143 MB instead of ~1600 MB, and finishes faster doing it.
- 2026-08-30 Cut peak RSS on a 953-photo pass from 1596 MB to 1315 MB with
  byte-identical output, by producing the upright image once instead of three times and
  resizing for detection straight from the borrow rather than through a `DynamicImage`
  round-trip.
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
The workflow this tool automates was first executed by hand against a real 2,519-file
photo library — nine days of travel, three recurring people — and the thresholds in
ADR-0003 come from measuring that run rather than from a paper. The library itself is
private, so the numbers are recorded here and in the ADRs while the photographs are not.
