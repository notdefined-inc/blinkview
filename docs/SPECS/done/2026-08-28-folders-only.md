# Folders only: hierarchy, cascade, self-healing

Status: active · 2026-08-28
Decisions: ADR-0009 (folders are the only grouping), ADR-0010 (per-folder user data),
ADR-0011 (self-healing cache)

## Intent

Collapse the organisational model to one primitive. A folder is where a photograph
lives; nested folders are what albums were trying to be; saved searches are the
cross-cutting views. Metadata lives beside the photographs it describes, so copying a
folder in Finder takes everything with it. The cache converges on its own.

The test of success: **copy `Trip/Greece Day3/` to a machine that has never run
blinkview, and it is still an organised, self-describing thing** — folder name, photos,
ratings, names.

## Scope

Core: prefix folder queries, cascading `blinkview.json`, metadata migration inside moves,
integrity check and staged rebuild, album→folder migration.
App: folder tree sidebar, roll-up grid with subfolder sections, grouping toggle, saved
searches replacing the albums panel.

Out of scope: the filesystem watcher (needs a new dependency — asked separately), and
the natural-language verb layer (its own spec, unblocked by this one).

## Acceptance criteria

1. Selecting a folder shows photographs in it **and every folder beneath it**; folder
   counts in the sidebar are recursive.
2. The sidebar shows a tree with disclosure, indented by depth, expanding to the current
   folder; expansion state is remembered per library.
3. With grouping set to folder, the grid sections by subfolder with sticky headers, the
   way it already sections by date. The toggle switches between folder and date.
4. A rating written to a photograph in `Trip/Greece Day3/` lands in
   `Trip/Greece Day3/blinkview.json`, not the library root.
5. Copying that folder alone to an empty directory and opening it as a library shows the
   rating and the face names, with no other files present.
6. Reading a photograph's metadata consults its folder, then ancestors, nearest wins for
   rating and label; a root-only library from an earlier version still reads correctly.
7. Moving a photograph between folders migrates its metadata entry in the same `Plan`,
   and `undo` restores both the file and the entry.
8. A corrupt `index.sqlite` is detected on open, deleted and rebuilt, with no dialog and
   no loss of anything under `blinkview.json`.
9. Adding photographs to a folder outside the app, then opening it, indexes exactly the
   new files — unchanged files are not rehashed.
10. Deleting `.blinkview/` and reopening rebuilds: the grid is usable before thumbnails
    and embeddings finish.
11. A photograph deleted and restored keeps its CLIP embedding — it is not recomputed.
12. Existing albums are offered as folders to materialise; names containing exFAT's
    reserved characters are slugified and the change is reported.
13. A saved search stores a query string, appears in the sidebar, and re-runs live.

## Tasks

- [x] 1. Prefix folder filter + recursive counts (core, app) — criterion 1
- [x] 2. Cascading `UserData`: resolve on load, write to the owning folder (core) — 4, 5, 6
- [x] 3. Metadata migration inside `Plan`/`Journal` for moves (core) — 7
- [x] 4. Integrity check, staged rebuild, scan on open (core, app) — 8, 9, 10, 11
- [x] 5. Folder tree sidebar with disclosure and remembered state (app) — 2
- [x] 6. Roll-up grid with subfolder sections and grouping toggle (app) — 3
- [x] 7. Saved searches replacing the albums panel (core, app) — 13
- [x] 8. Album migration command and prompt (core, app) — 12
- [x] 9. Doc sync: STATUS.md, DESIGN.md, remove albums from the search grammar

## Risks

**Moves are the dangerous path.** Metadata migration touches the one code path where a
mistake destroys work. Mitigation: it goes inside `Plan`, which validates before writing,
and criterion 7 tests the undo.

**The cascade can be slow if walked naively.** Resolve once per scan into memory; never
open files per photograph.

**Album removal is user-visible loss** if someone has albums and no migration. Task 8
ships before the albums panel is removed.

## Outcome

Shipped. All thirteen acceptance criteria met, verified against a purpose-built nested
library (`Trip/Greece Day1`, `Greece Day2`, `Swiss Day1`, plus two loose files) and the
280-photo demo library.

The criterion the design exists for — copy a folder out and it is still self-describing
— was verified literally: two photographs were rated and one labelled inside
`Trip/Greece Day1`, the folder was copied out on its own with `cp -R`, opened as a
library in its own right, and both ratings and the label were still there. No cache came
along, and none was needed.

Three things surfaced only in the running app:

- The frontend kept its own exact-match folder filter, so the roll-up half worked until
  both ends used the same segment-wise rule.
- `:root[data-theme="light"] .fopt` outweighs `.fopt[aria-pressed="true"]` on
  specificity, so every selected filter chip was invisible in light mode — a pre-existing
  bug in the Aurora Glass overhaul, found by adding one more control to that row.
- `4stars+` fell through to semantic search, because the star regex allowed a leading
  `+` but not a trailing one, while the chip it draws reads `★★★★+`.

One honest asymmetry, surfaced in the migration dialog rather than left to be
discovered: undo restores the moved photographs but not the cleared album names.

## Not done here

The filesystem watcher (FSEvents) is still out of scope — it needs a new dependency.
Scanning on open covers the common case; the watcher would cover photographs arriving
while the window is already open.
