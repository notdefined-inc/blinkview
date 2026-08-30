# ADR-0010: User-authored data cascades per folder

Date: 2026-08-28
Status: Accepted (extends ADR-0007)

## Context

ADR-0007 moved ratings, labels and names out of the disposable cache to a single
`blinkview.json` at the library root. That fixed data loss on `rm -rf .blinkview`, but
left a hole the root placement cannot close:

**Copy `Trip/` out of the library in Finder and the metadata does not come with it.**
The photographs and the folder names travel; the ratings, labels and face names stay
behind in a file one level up. The organisation is portable, what you said about it is
not.

This matters because folders are now the only grouping (ADR-0009), so a folder is the
unit people hand to each other.

The precedent is `AGENTS.md` in this repository: a global file sets defaults, a more
specific one overrides it, and the nearest wins. Applying that shape to photo metadata
makes a folder self-describing.

## Decision

**`blinkview.json` may exist in any folder, and the nearest one wins.**

- **Reads cascade.** For a photograph, consult the `blinkview.json` in its own folder,
  then each ancestor up to the library root. Nearest wins for scalars (rating, label);
  union for sets (people).
- **Writes go to the folder that directly contains the photograph.** This is the rule
  that makes copy-paste work: whatever you say about a photograph is stored beside it.
- **Settings cascade the same way** — sort order, grouping, cover photo — with the
  *most specific* winning, as in `AGENTS.md`. A folder that says "sort oldest first"
  beats the library default; it does not lose to it.

Entries stay keyed by content hash, so a rename in Finder still does not break them.

### Moves must carry metadata

Root-level metadata made moves free: a photograph moved between folders and its rating
followed, because the rating referenced no location. Per-folder metadata ends that —
moving a photograph from `Day1/` to `Day3/` has to migrate its entry between two files.

This rides in the existing transaction. Moves already go through `Plan` → `Journal`, so
the metadata migration is part of the same plan and is undone by the same undo. A move
that cannot migrate its metadata is not applied.

## Consequences

Good: a folder is self-describing and portable. Copying `Trip/Greece Day3/` to a friend
takes the ratings and names with it. Many small text files also **conflict far more
gracefully under cloud sync** than one large file — two machines editing different
folders never touch the same file, and a genuine collision is readable text rather than
a binary blob.

Costly: every move is now file-plus-metadata, which is real new complexity in the one
code path where mistakes destroy work. It is contained by going through `Plan`, which
already validates before touching the disk.

Also costly: reading one photograph's rating may open several files. Cascades are
resolved once per scan and held in memory, not walked per photograph.

A library written by an earlier version has a single root `blinkview.json`. It keeps
working unchanged — the root is simply the outermost level of the cascade — and entries
migrate down to their photograph's folder the next time they are written.
