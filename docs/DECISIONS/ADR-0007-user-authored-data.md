# ADR-0007: User-authored data lives at the library root

Date: 2026-08-28
Status: Accepted (supersedes the first draft of this ADR, see Correction)

## Context

Two things in this project cannot be recomputed from the photographs:

- **Names.** Clustering faces is derivable; knowing a cluster is called "Alex" is not.
- **Ratings, labels, albums.** A star exists nowhere but in someone's head until it is
  recorded.

Everything else — the index, thumbnails, signatures, face embeddings, transcodes — is
derived, and ADR-0001 promises `.openfoto/` can be deleted and rebuilt.

Options:

- **XMP inside the photo.** Portable; Finder, Lightroom and Bridge all read
  `xmp:Rating`. But rating a photo would rewrite it, re-encoding the JPEG and changing
  its content hash, which invalidates every cache keyed to it. Rating a photograph
  should not modify the photograph.
- **Sidecar files beside each photo.** Non-destructive, but scatters
  `IMG_1234.jpg.openfoto.json` through folders people browse in Finder.
- **Inside `.openfoto/`.** Contradicts the disposability promise outright.
- **At the library root.** Survives deleting the cache, travels with the folder when it
  is copied, visible in Finder, and modifies no photograph.

## Decision

`openfoto.json` (ratings, labels, albums) and `openfoto-people.json` (names and
reference faces) sit at the **library root**, beside `Trash/` and `Originals/` — visible
files the user can see, copy and back up.

`.openfoto/` therefore contains only derived data and is genuinely disposable, exactly as
ADR-0001 says. There are no exceptions to that promise.

Both files are keyed by content hash, so a rating survives renaming or moving a photo,
including in Finder. `Library::open` moves either file out of `.openfoto/` if an older
version left it there, on open rather than on next save — a library upgraded today and
cleaned tomorrow must not lose work.

Two tests hold the line: one writes a name and a rating, deletes `.openfoto/` entirely,
and asserts both survive; the other asserts the paths do not contain `.openfoto`.

## Correction

The first version of this ADR put both files *inside* `.openfoto/` and restated
ADR-0001 as "everything **derived** is disposable, and two files are not". That was
weakening a guarantee to fit an implementation rather than fixing the implementation.
The user challenged it, and they were right: a promise that `rm -rf .openfoto` is safe
must be true without a footnote.

## Consequences

Good: the disposability promise is unqualified again. Deleting the cache costs only
recomputation. Copying the folder carries names and ratings with it. No photograph is
modified. Two visible files, no scattering.

Costly: two files appear in the user's photo folder. Ratings remain invisible to other
applications, unlike XMP — exporting to XMP on demand would give portability without
making every rating a re-encode, and is the obvious next step if that matters.
