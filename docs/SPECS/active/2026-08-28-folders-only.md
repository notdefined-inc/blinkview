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
openfoto, and it is still an organised, self-describing thing** — folder name, photos,
ratings, names.

## Scope

Core: prefix folder queries, cascading `openfoto.json`, metadata migration inside moves,
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
   `Trip/Greece Day3/openfoto.json`, not the library root.
5. Copying that folder alone to an empty directory and opening it as a library shows the
   rating and the face names, with no other files present.
6. Reading a photograph's metadata consults its folder, then ancestors, nearest wins for
   rating and label; a root-only library from an earlier version still reads correctly.
7. Moving a photograph between folders migrates its metadata entry in the same `Plan`,
   and `undo` restores both the file and the entry.
8. A corrupt `index.sqlite` is detected on open, deleted and rebuilt, with no dialog and
   no loss of anything under `openfoto.json`.
9. Adding photographs to a folder outside the app, then opening it, indexes exactly the
   new files — unchanged files are not rehashed.
10. Deleting `.openfoto/` and reopening rebuilds: the grid is usable before thumbnails
    and embeddings finish.
11. A photograph deleted and restored keeps its CLIP embedding — it is not recomputed.
12. Existing albums are offered as folders to materialise; names containing exFAT's
    reserved characters are slugified and the change is reported.
13. A saved search stores a query string, appears in the sidebar, and re-runs live.

## Tasks

- [ ] 1. Prefix folder filter + recursive counts (core, app) — criterion 1
- [ ] 2. Cascading `UserData`: resolve on load, write to the owning folder (core) — 4, 5, 6
- [ ] 3. Metadata migration inside `Plan`/`Journal` for moves (core) — 7
- [ ] 4. Integrity check, staged rebuild, scan on open (core, app) — 8, 9, 10, 11
- [ ] 5. Folder tree sidebar with disclosure and remembered state (app) — 2
- [ ] 6. Roll-up grid with subfolder sections and grouping toggle (app) — 3
- [ ] 7. Saved searches replacing the albums panel (core, app) — 13
- [ ] 8. Album migration command and prompt (core, app) — 12
- [ ] 9. Doc sync: STATUS.md, DESIGN.md, remove albums from the search grammar

## Risks

**Moves are the dangerous path.** Metadata migration touches the one code path where a
mistake destroys work. Mitigation: it goes inside `Plan`, which validates before writing,
and criterion 7 tests the undo.

**The cascade can be slow if walked naively.** Resolve once per scan into memory; never
open files per photograph.

**Album removal is user-visible loss** if someone has albums and no migration. Task 8
ships before the albums panel is removed.
