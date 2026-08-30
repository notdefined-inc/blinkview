# ADR-0009: Folders are the only grouping

Date: 2026-08-28
Status: Accepted (supersedes albums-as-metadata, shipped 2026-08-28)

## Context

The app shipped two ways to group photographs: **folders**, which are real directories
and exclusive, and **albums**, which are strings in `blinkview.json` and many-to-many.

Two concepts, and the second was never load-bearing:

- `set_album` writes `UserData` directly. It does not build a `Plan`, so album edits are
  **not journalled and cannot be undone**, while every folder move can.
- Albums are invisible in Finder. Copy a folder out of the library and its album
  membership does not come with it.
- Asking "what is the difference?" was a question the product could not answer briefly,
  which is itself the cost of a second concept.

The case for albums is many-to-many membership: a photo of Sam at the church in Greece
could be in *Greece 2026*, *Churches* and *Favourites* at once. Folders cannot express
that without copies — which duplicate bytes, break content-hash identity, and would be
flagged by our own dedupe — or symlinks, which exFAT does not have at all. The reference
library is exFAT.

But the premise is wrong. **Albums exist to compensate for weak search.** Apple and
Google Photos have them because their libraries are black boxes with no filing at all;
Lightroom has Collections because re-filing is expensive there. blinkview searches by
date, person, rating, label and — since ADR-0008 — by what a photograph shows. When
anything can be found, the cost of filing it in exactly one place collapses.

And the way people organise photographs without any app is already hierarchical and
exclusive: `Trip/Greece Day3/`. That is a folder.

## Decision

**One organisational primitive: the folder.** Albums are removed.

The two roles albums played are taken over by things that do them better:

- **Grouping** → nested folders. `Trip/Greece Day3/` is what an album was trying to be,
  and it survives the app being deleted.
- **Cross-cutting views** → saved searches. *Churches* becomes the query `a church`;
  *Best* becomes `4stars+`. These beat albums on their own terms because they stay
  current: a photo added next week joins automatically.

Existing albums migrate by being offered as folders to materialise into.

Consequently the folder filter matches by **prefix**, not exact parent: selecting `Trip`
shows everything beneath it. Nesting was already scanned and tracked with depth; only
the filter treated the tree as flat.

## Consequences

Good: one mutation path (`Plan` → `Journal` → undo) for all organisation, so grouping
becomes undoable where it previously was not. `blinkview.json` shrinks to ratings and
labels — the things that genuinely have no location. The natural-language layer loses a
verb and an ambiguity: "move these to Greece" needs no album-versus-folder question.

Costly: the hand-picked set that no query describes — *"these thirty for printing"* —
must become a real folder, which moves those photographs out of their filing. This is
judged acceptable, and arguably correct: a folder is transferable, and the move is
journalled and reversible.

Also costly: exclusivity forces a filing decision that cannot be deferred. Search is the
mitigation — it stops mattering much which folder a photograph is in.

Folder names now carry exFAT's restrictions (`" * / : < > ? \ |` are rejected), where
album names were free text. Album names containing those characters are slugified on
migration, and the user is shown what changed.
