# Colour in batches, and metadata
Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem
Brightness, contrast and saturation exist (`edit.rs:43`) but reach exactly one
photograph, through the editor, one slider at a time — so "warm these forty up" is
forty visits. And metadata is readable only in the small subset the info panel shows
(`app.js:2440`): no camera, no exposure, no GPS, and no way to remove any of it before
sending a photograph to someone.

## Non-goals
- No raw processing, curves, or per-channel colour. Three adjustments and named presets
  built from them.
- Stripping does not touch openfoto's own files: ratings and names live in
  `openfoto.json`, not in the photograph (ADR-0007), so they survive stripping.
- No stripping of HEIC or video in this spec — refused with a reason, not attempted.

## Design
**Presets** are named `Adjust` values, defined once in core so the CLI and the app
cannot disagree: Mono, Warm, Cool, Punch, Faded. The editor shows them as chips that
set the sliders, so a preset is a starting point that can then be adjusted rather than
a mode.

**Batch** is `edit_photos(path, hashes, edit)`, looping the existing `edit::apply` and
reporting progress through the same `progress::Counter` every other long pass uses.
`keep_original` defaults to true exactly as the single-photo path does, so a batch that
is regretted is recoverable from `Originals/` — this matters more in a batch, where the
mistake is multiplied. Each photograph's content hash changes, so the pass rescans once
at the end rather than per photograph.

**Metadata** gains an inspect and a strip. Inspect extends `photo_detail` with camera
make and model, lens, ISO, exposure, f-number, focal length and whether GPS is present
— read with `kamadak-exif`, already a dependency. GPS is reported as present/absent
with the coordinates shown, since "does this say where I live" is the question people
actually have.

Strip rewrites the file without its metadata segments: for JPEG, every APPn and COM
segment is dropped and the entropy-coded image data is copied through byte for byte, so
the pixels are bit-identical and nothing is re-encoded. For PNG, the text and eXIf
chunks are dropped. Anything else is refused by name. Like editing, it keeps the
original in `Originals/` by default.

Rejected: re-encoding through the image crate to drop metadata. It would recompress
every JPEG — a quality loss to remove data that is not in the pixels — and would be
slower by two orders of magnitude.

**Stripping is destructive in a way editing is not**: `taken_at` comes from EXIF
(ADR-0003), so a stripped photograph falls back to filename or mtime for its date, and
that is a decision worth recording — see the ADR this spec adds.

## Acceptance criteria
1. Each preset sets the three sliders and the preview updates; the sliders remain
   editable afterwards.
2. Applying a preset to a selection of N writes N files and reports progress.
3. With `keep_original`, every original lands in `Originals/`. They are counted there,
   like any other folder — `Originals/` is visible and real, and only `Trash/` is
   excluded from the library's counts. That is what editing has always done (ADR-0006).
4. A batch that fails on one file completes the rest and reports how many failed.
5. The info panel shows camera, lens, ISO, exposure and focal length when present, and
   says so plainly when absent.
6. GPS present is stated with coordinates; absent is stated as absent.
7. Stripping a JPEG removes every APPn and COM segment: a re-read finds no EXIF.
8. The stripped JPEG's decoded pixels are identical to the original's, byte for byte.
9. Stripping a HEIC or a video is refused with a message naming the format.
10. After stripping, the library still shows the photograph, re-identified by its new
    content hash, with its rating and label intact — which requires carrying the
    metadata onto the new hash, since it is keyed by content (ADR-0007, ADR-0015).

## Tasks
- [x] 1. `Adjust::PRESETS` in core + preset chips in the editor (touches: edit.rs, app.js)
- [x] 2. `edit_photos` batch command with progress (touches: lib.rs, app.js)
- [x] 3. EXIF detail read + info panel rows (touches: lib.rs, app.js)
- [x] 4. `metadata::strip` for JPEG and PNG + tests (touches: crates/openfoto-core/src/metadata.rs)
- [x] 5. `strip_metadata` command + UI, ADR on the date consequence (touches: lib.rs, app.js, docs/DECISIONS)

## Verification notes
Driven in the running app against JPEG fixtures carrying real EXIF (make, model, ISO)
and GPS coordinates:

- The info panel read *Camera: TestMake TestModel*, *ISO 400*, and
  *Location: 51 deg 30 min 0 sec N, 0 deg 7 min 0 sec W*. The first implementation
  printed the reference letter twice ("N N") because `with_unit` already appends it.
- After stripping, every EXIF field came back `null` with `present: false`, and the
  file carried no EXIF at all when re-read with an independent decoder.
- **The decoded pixels are byte-identical** between the original kept in `Originals/`
  and the stripped file (compared by decoding both), while the file shrank
  10,646 → 10,360 bytes. That is the claim the segment rewrite exists to make.
- A video was refused by name — *0 stripped · 1 left alone — openfoto cannot strip MP4
  files* — and a mixed batch finished the rest: *2 stripped · originals kept in
  Originals/ · 1 left alone — openfoto cannot strip MP4 files*.
- A batch colour over two photographs reported *2 changed* and kept both originals.
- Ratings and labels survive both operations even though the content hash changes
  (verified: five stars and a red label on a stripped photograph, five stars on a
  coloured one, with both hashes confirmed different afterwards).

**A bug this found.** Writing ADR-0015 raised the question of whether a rating survives
a rewrite. It did not: `PhotoMeta` is keyed by content hash, so stripping — *and
editing, ever since editing existed* — silently discarded the rating, the label and any
album membership. `UserDataSet::rekey` now carries them across, and all three rewrite
paths (single edit, batch edit, strip) use it.
