# ADR-0007: Ratings and labels are user-authored data

Date: 2026-08-28
Status: Accepted

## Context

Ratings, colour labels and album membership cannot be recomputed. A star exists nowhere
but in someone's head until they record it. That puts them in direct tension with
ADR-0001, which promises `.openfoto/` is disposable and rebuildable by rescanning.

`people.json` already carries the same problem: a machine can cluster faces but cannot
know a cluster is called "Nikhil".

Options considered:

- **XMP written into the photo.** Genuinely portable — Finder, Lightroom and Bridge all
  read `xmp:Rating` and `xmp:Label`. But it means rewriting every photo the user rates,
  which re-encodes JPEGs and changes their content hash, invalidating the caches keyed to
  it. Rating a photo should not modify the photograph.
- **Sidecar files beside each photo.** Portable and non-destructive, but scatters
  `IMG_1234.jpg.openfoto.json` through folders the user browses in Finder.
- **A file in the vault.** Not recomputable, so it contradicts the disposability
  promise — unless the promise is stated more precisely.

## Decision

`.openfoto/user.json`, keyed by content hash, holding rating, label and albums.

The ADR-0001 promise is restated as: **everything derived from the photographs is
disposable.** Two files are not derived and are the only things worth backing up —
`people.json` and `user.json`. Both are small, plain, human-readable JSON.

Keying by content hash rather than path means a rating survives renaming or moving a
photo, including when the user does it in Finder — the same property that makes the rest
of the index survive external edits.

Entries that carry no information are pruned, so a star set and then cleared leaves
nothing behind.

## Consequences

Good: rating a photo never modifies the photograph. Nothing is scattered through the
user's folders. The two exception files are small enough to copy anywhere.

Costly: ratings are invisible to other applications, unlike XMP. Deleting `.openfoto/`
now does lose something, which weakens a promise the project has made loudly — the
mitigation is that it is two small files, and both are named here and in STATUS.md.

Exporting to XMP on demand would give portability without making every rating a
re-encode, and is the obvious next step if this becomes a real limitation.
