# The cache moves out of your photo folders

Status: Draft   Owner: somesh   Date: 2026-08-31

## Problem

`.blinkview/` sits inside every library. On the reference machine that is **1.9 GB inside
a 26 GB photo folder** (7.3%), plus `._.blinkview` AppleDouble sidecars on exFAT. Three
costs follow:

* A library kept in Dropbox or iCloud syncs gigabytes of regenerable thumbnails — and
  ADR-0011 exists precisely because those services corrupt the SQLite in there.
* A library on read-only media cannot be opened at all: `Library::open` creates the vault
  before it does anything else.
* Opening a folder to look at it leaves a permanent artefact in it.

Separately, `blinkview-people.json` sits at the library root because ADR-0007 says it
cannot be recomputed. Measured: **172 KB, of which ~99.9% is 58 face-embedding vectors
that the cache already holds** in `index.sqlite`'s `faces` table, keyed `(hash, idx)`.
The file is at the root for a reason that is true of three of its fields and false of the
fourth.

## Non-goals

- **Moving anything a person typed.** Ratings, labels, saved searches, sort order, names,
  exclusions and dismissals stay at the visible library root. Copying a folder in Finder
  keeps carrying them; that is a promise on the landing page, not an implementation
  detail.
- **Re-deriving on migration.** An existing library must keep its index, thumbnails,
  faces and undo journal. The journal in particular is the one thing in the vault that is
  *not* reproducible.
- **A cache-management UI.** Removing a source with `purge` already deletes its cache.
  Orphan cleanup gets a command and a listing, not a screen.
- **New dependencies.** Cache-root resolution and id generation are written here.
- **Changing the face model or thresholds.** ADR-0003 stands; `references` changes how it
  is stored, not what it means.

## Design

### Where the cache goes

`<cache root>/libraries/<id>/`, holding exactly what `.blinkview/` holds today —
`index.sqlite`, `thumbs/`, `derived/`, `faces/`, `journal/`.

Cache root, first match wins:
1. `$BLINKVIEW_CACHE` — how tests get an isolated one.
2. macOS `~/Library/Caches/dev.notdefined.blinkview`; Linux `$XDG_CACHE_HOME` or
   `~/.cache/blinkview`; Windows `%LOCALAPPDATA%\Blinkview\cache`.

`~/Library/Caches` is chosen over Application Support deliberately: the contents are
disposable, and Time Machine and iCloud both skip it. The OS may purge it under disk
pressure, which costs a rescan — the same event ADR-0011 already handles.

### How a folder finds its cache

A `.blinkview-id` file at the library root, one line, 32 hex characters. About 40 bytes
where 1.9 GB used to be.

Keying by path was rejected: it is what Claude Code does
(`~/.claude/projects/<path-with-slashes-swapped>`), and renaming the folder orphans the
data. That is acceptable for chat transcripts and not for a cache whose loss costs a
re-analysis of 1,829 photographs including face embeddings. `survives_a_folder_renamed_externally`
is an existing test; it must keep passing.

When the marker cannot be written — read-only media — the library falls back to a
path-derived id, and says so in the log. Read-only libraries then work, and lose their
cache if the folder moves. That is strictly better than today, where they do not open.

### `blinkview-people.json`, version 2

```json
{ "version": 2,
  "people": [ { "name": "Sam", "faces": ["<hash>:0", "<hash>:2"], "excluded": ["<hash>"] } ],
  "dismissed": ["<hash>:1"] }
```

`references: Vec<Vec<f32>>` becomes `faces: Vec<String>` — the same `"<hash>:<idx>"`
idiom `dismissed` already uses, resolved against the `faces` table on load. ADR-0003's
finding that references must be a *set* rather than a centroid is untouched: this changes
where the vectors are kept, not how many there are or how they are compared.

Reading a v1 file matches each stored vector against `faces.embedding` by exact bytes —
they were copied from there — and rewrites it as pointers. A vector that matches nothing
(the cache was deleted at some point) is **kept inline** in a `references` field rather
than discarded, so no identity is lost by upgrading.

### Order of migration, on first open

1. Resolve or mint the id; write `.blinkview-id`.
2. If `<root>/.blinkview/` exists, move it into the cache root. `rename` first; on EXDEV —
   an external drive to the home disk, which is the 1.9 GB case — copy with progress, then
   remove. Same fallback as `move_to_system_trash`.
3. Convert `blinkview-people.json` to v2 against the now-present `faces` table.

Steps 2 and 3 are each idempotent and safe to interrupt: an interrupted copy leaves the
original in place, and a v1 people file is still readable.

## Acceptance criteria

1. A library opened by this build has no `.blinkview/` directory and a `.blinkview-id`
   file of 32 hex characters; its index, thumbnails, faces and journal are all intact and
   no rescan is triggered.
2. Migrating the 26 GB reference library moves 1.9 GB across a filesystem boundary,
   reports progress, and ends with the source vault gone.
3. Interrupting that copy leaves the original `.blinkview/` intact and the library usable.
4. Renaming a library folder in Finder keeps its cache — `survives_a_folder_renamed_externally`
   passes unchanged.
5. Copying a library folder elsewhere and opening the copy re-indexes it, and does not
   adopt or corrupt the original's cache. (Both folders then hold the same id; the second
   to be opened mints a new one and rewrites its marker.)
6. A library on read-only media opens, indexes and displays, with the cache under a
   path-derived id.
7. `blinkview-people.json` for the reference library shrinks from 172 KB to under 8 KB,
   and the same faces are still recognised as the same person afterwards.
8. A v1 people file whose cache has been deleted keeps every reference vector inline and
   still recognises that person.
9. Deleting the whole cache root loses no rating, label, saved search, sort order, name,
   exclusion or dismissal.
10. Removing a source with `purge` deletes that library's cache from the cache root.
11. `blinkview cache list` names every cached library with its last-known path and size;
    `blinkview cache prune` removes those whose folder is gone.
12. Nothing in a photo folder is written except `.blinkview-id`, `blinkview.json` and
    `blinkview-people.json`.

## Tasks

- [ ] 1. `cache.rs`: root resolution, id minting, `.blinkview-id` read/write, path-derived fallback (touches: new `cache.rs`)
- [ ] 2. `Library::open` resolves the cache through it; `VAULT_DIR` stops being a path fragment (touches: `library.rs`, every `VAULT_DIR` caller)
- [ ] 3. Migration: move an existing vault, with cross-filesystem copy and progress (touches: `library.rs`, `lib.rs`)
- [ ] 4. People v2: `faces` pointers, v1 conversion, inline fallback for unmatched vectors (touches: `faces/people.rs`, `faces/assign.rs`)
- [ ] 5. `purge` and `cache list` / `cache prune` (touches: `lib.rs`, `blinkview-cli`)
- [ ] 6. Tests: migration keeps the journal, rename still works, copy does not collide, read-only opens, people round-trip (touches: `tests/lifecycle.rs`, `faces`)
- [ ] 7. ADR-0019 superseding ADR-0001's placement and amending ADR-0007; STATUS, ARCHITECTURE, README, landing page
