# Status

_Last updated: 2026-08-27_

## Current work
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
