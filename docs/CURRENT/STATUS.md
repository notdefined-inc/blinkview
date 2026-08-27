# Status

_Last updated: 2026-08-27_

## Current work
Phase 1 partly done. Working today: `scan`, `status`, `rename`, `undo`, `history`.
20 tests pass (`cargo test --workspace`), clippy clean with `-D warnings`.

Verified end-to-end against 25 real photos copied from the reference library:
scan (25 hashed) -> rescan (25 unchanged, fast path) -> rename --apply -> re-run plans
nothing -> undo restores every original name. A folder renamed outside the tool mid-session
was re-identified by content hash: 12 moved, 0 lost.

## Known issues
- `dedupe`, `faces` and `scenery` are not implemented — spec tasks 6-11.
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
