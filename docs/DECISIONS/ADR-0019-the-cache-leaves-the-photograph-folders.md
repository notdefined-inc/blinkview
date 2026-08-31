# ADR-0019: the cache leaves the photograph folders

Date: 2026-08-31
Status: Accepted

## Context

`Library::open` created `.blinkview/` beside the photographs. On the reference machine
that put **1.9 GB of regenerable thumbnails inside a 26 GB photo folder** (7.3%), plus
`._.blinkview` AppleDouble sidecars on exFAT. Three costs followed:

* A library kept in Dropbox or iCloud syncs gigabytes of derived data — and ADR-0011
  exists precisely because those services corrupt the SQLite in there.
* A library on read-only media could not be opened at all: opening created the vault.
* Opening a folder to look at it left a permanent artefact in it.

Separately, `blinkview-people.json` sat at the library root on the strength of ADR-0007's
"cannot be recomputed" — and 172 KB of it on the reference library was **58 embedding
vectors the index already held**. The file was where it was for a reason true of three
of its fields and false of the fourth.

This supersedes the placement half of ADR-0001 and amends ADR-0007. Everything else in
both stands.

## Decision

The derived cache lives under one root per machine, and a library finds its own through
a marker.

* **`<cache root>/libraries/<id>/`** holds what `.blinkview/` held. The root is
  `~/Library/Caches/dev.notdefined.blinkview` on macOS, `$XDG_CACHE_HOME`/`~/.cache`
  on Linux, `%LOCALAPPDATA%` on Windows — `Caches`, not Application Support, because
  the contents are disposable and Time Machine and iCloud both skip it. `BLINKVIEW_CACHE`
  overrides for tests.
* **`.blinkview-id`** at the library root, 32 hex characters, about forty bytes. Keyed
  by marker rather than path: renaming a folder in Finder is a first-class event, and a
  path key would orphan the index, the thumbnails, the embeddings and the undo journal
  with it. When the marker cannot be written — read-only media — the cache is keyed by
  a hash of the folder's path, stable across runs. Read-only libraries open; they used
  to be impossible.
* **A marker is not ownership.** Two folders carrying one id means someone duplicated a
  library; the second to be opened mints a fresh id and rewrites its marker, so the copy
  re-indexes rather than fighting the original for one cache. The breadcrumb file inside
  each cache records the path it last served, which is what makes a copy distinguishable
  from a move: a moved folder adopts its cache, a copied one starts fresh.
* **Migration is a rename or nothing.** An existing in-folder vault is renamed into the
  cache, instant and whole — journal included. A rename cannot cross a filesystem, so a
  library on an external drive starts a fresh cache and the old directory is left where
  it is: reported in the log, never deleted from inside someone's photographs. The owner
  settled this: these caches are not worth a 1.9 GB copy.
* **`blinkview-people.json` v2 stores pointers**, `"<hash>:<idx>"` — the idiom
  `dismissed` already used — resolved against the index's `faces` table on load and
  contracted back on save. A vector with no face to point at stays inline, so a v1 file
  whose cache was already deleted loses no identity. A v1 file is converted on first
  load. On the reference library: **172,177 → 5,010 bytes**, and the person it names is
  still recognised across all 66 of their photographs.

Nothing a person typed moves. Ratings, labels, saved searches, sort order, names,
exclusions and dismissals stay at the visible library root and travel when a folder is
copied. `Trash/`, `Originals/`, `Duplicates/` and `Scenery/` are user-visible features
and stay where they are.

## Consequences

Good: a photo folder holds its photographs, plus a marker and two small JSON files.
Sync services stop seeing derived data. Read-only media opens. `blinkview cache list`
names every cache with its library and size; `cache prune` deletes those whose folder
is gone; removing a source with *purge* takes its cache and marker.

Costly: copying a library to another machine no longer carries its cache — that machine
re-indexes. The OS may purge `~/Library/Caches` under disk pressure, which costs the
same rescan. And a cache with no breadcrumb is never pruned automatically, because
unknown is not gone.
