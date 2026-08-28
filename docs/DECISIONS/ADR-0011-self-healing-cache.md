# ADR-0011: The cache heals itself

Date: 2026-08-28
Status: Accepted (extends ADR-0001)

## Context

ADR-0001 promises `.openfoto/` is disposable. Disposable is not the same as
self-correcting, and three things break the promise in practice:

- Photographs added, removed or reorganised in Finder while the app is closed.
- A deleted cache that then needs rebuilding before anything is usable.
- **Cloud sync corrupting `index.sqlite`.** A library in iCloud or Dropbox will corrupt
  its SQLite eventually; this is well documented and not preventable from inside.

Obsidian is the cautionary case. Its sync pain comes from `.obsidian/` mixing precious
state — workspace layout, plugin settings — with cache, so a conflict can lose real work
and the folder cannot simply be deleted. openfoto already separated those (ADR-0007,
ADR-0010): everything in `.openfoto/` is reproducible, everything precious is not in it.

That separation makes a strong policy available.

## Decision

**Cache corruption is a normal event, not an error.** On open, `PRAGMA quick_check`; on
failure, delete the cache and rebuild. No dialog, no complaint — there is nothing to
lose, which is the entire point of ADR-0001.

Four rules follow:

1. **Scan on open, always.** Not a button. `scan` already skips hashing when size and
   mtime match, so the common case is cheap enough to be invisible.
2. **The filesystem is the truth; the index is a cache; reconciliation is a diff.**
   Never a rebuild, except when integrity fails.
3. **Rebuild in stages.** Index first, so the grid is usable in seconds; thumbnails on
   demand as cells scroll; embeddings in the background. Never a modal wait.
4. **Never reap orphans automatically.** `clip`, `faces` and `signatures` are keyed by
   content hash and outlive the file rows that referenced them, so a photograph that
   disappears and comes back keeps its embedding — around 100 ms per photo not spent
   again. Reaping happens only on an explicit vacuum.

## Consequences

Good: a library reorganised in Finder, or synced between two machines, or whose cache
was deleted outright, converges without the user being asked to do anything. The
expensive derived work — embeddings and face detection — survives file churn.

Costly: orphaned rows accumulate for hashes no longer present. This is the deliberate
trade for rule 4; a vacuum command reclaims the space when someone wants it.

The undo history in `journal/` is the accepted casualty. It is a record of user actions
rather than a derived artefact, so a silent rebuild loses something real — but it loses
the ability to undo, never a photograph, and keeping it would mean the cache is no
longer disposable, which would give back exactly the problem this decision solves.
