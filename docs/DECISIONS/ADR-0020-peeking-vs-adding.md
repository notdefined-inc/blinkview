# ADR-0020: peeking and adding are two commitment levels over one library type

Date: 2026-08-31
Status: Accepted

## Context

The only way into Blinkview was **Add a folder**: register a source, write the marker,
scan recursively, start a watcher, build a cache. That is the right commitment for a
photo library and the wrong one for "what is on this SD card" — the everyday act of
double-clicking a JPEG, or dropping a folder on the window to glance at it. XnView,
FastStone and IrfanView all open a file, show its folder, and write nothing. We had no
equivalent, and faking one through the source list would have collided with
`source_conflict`, persisted itself into `sources.json` on any save, and started
watching a folder the user never offered.

Adding also had a cost problem in the other direction: the scan was recursive with no
exclusions, so pointing at a home directory indexed `~/Library`'s cached PNGs for
hours without the user being told anything beforehand.

## Decision

**A peek is a second commitment level over the same `Library` type, not a separate
code path.**

* `Library::peek(root)` opens markerless and shallow: only files *directly* in the
  folder are indexed (`WalkDir::max_depth(1)`), no watcher starts, and the cache lives
  under `<cache root>/peek/<path-id>/`, deleted by `end_peek`. Looking at a folder
  leaves it byte-identical.
* Every writing command refuses a peek by name at the command layer; the window shows
  the same boundary as a banner with one action — **Keep this folder** — which runs
  `promote_peek`: end the peek (cache deleted), register, scan recursively, watch.
  There is no "peek with a little writing", because that is what an added folder is.
* The `photo://` grant is narrowed, not loosened: a request is allowed inside an added
  source **or** when its parent directory is exactly a granted peek folder
  (`parent() ==`, never `starts_with` — the peek promise not to recurse is also the
  security boundary).
* Opening a path that is already inside a source opens **that library**, positioned on
  the file, keyed by the *stored* source path — canonicalising only for the
  comparison, so a symlinked source never opens twice under two keys.
* `fileAssociations` (`Viewer`, `Alternate` rank) plus `RunEvent::Opened` route
  Finder's Open With through the same `open_path` command as a drop, so there is one
  entry decision, not two.

**Adding shows its size first.** `survey_folder` counts media and subfolders from
directory entries alone — no hashing, no decoding, capped at 200,000 entries, which is
answered honestly as `below: None` — and the dialog offers **This folder only** or
**Include subfolders**, with the safe default focused when recursion is the surprising
choice. Depth is then a property of the source (`sources.json` accepts legacy bare
strings, which stay recursive), changeable from the context menu without losing
ratings, which live beside the photographs (ADR-0007). New sources skip a fixed list
of system directory names (`Library`, `node_modules`, `.git`, …) while *descending*;
a folder added directly is never skipped by its own name. Existing sources keep their
exact old behaviour until edited.

## Consequences

Good: Blinkview is now a viewer you can aim at anything, with the library commitment
made deliberately. The survey makes an 84,000-file accident a conscious choice. One
`Library` type serves both levels, so scan, serve and the grid have no forks.

Costly: depth lives in two places that must agree — `sources.json` and the library's
scan configuration — and a shallow→recursive switch reconciles the index rather than
restarting it, so the reconcile path has its own test. Open With is configured for all
platforms but verified only on macOS; the dev build is not bundle-registered, so the
LaunchServices hop itself is covered by unit tests on `owning_source` plus the config,
not by a packaged double-click.
