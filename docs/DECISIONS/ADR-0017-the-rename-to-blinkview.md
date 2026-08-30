# ADR-0017: the rename to Blinkview, and what it must not cost

_Status: accepted, 2026-08-30_

## Context

`openfoto` is a common name. It collides with other projects, is hard to search for,
and is not defensible. The product is now **Blinkview**.

A rename is cheap in source and expensive on disk. Three names were load-bearing
outside the repository:

* `.openfoto/` — the derived cache, holding the index, thumbnails and journal. Rebuildable,
  but rebuilding it means scanning and rethumbnailing everything: 5.5s + 49s on the
  20,000-photo reference library, before any face or scene pass runs again.
* `openfoto.json` and `openfoto-people.json` — ratings, labels, saved searches, and the
  names people typed. ADR-0007: these exist nowhere but in someone's head until they are
  recorded, and no machine can reproduce them.
* `dev.notdefined.openfoto` — the bundle identifier, which names the directory holding
  `sources.json`, the list of folders someone added.

Renaming any of these without a migration turns an upgrade into apparent data loss: the
library opens unrated, unnamed, with an empty sidebar, and spends minutes
rebuilding what it already had.

## Decision

Rename everything, and adopt what the old names left behind on first open.

* `Library::open` renames `.openfoto/` to `.blinkview/` before touching it, so the index
  is inherited rather than rebuilt. Only when there is no `.blinkview/` already; if both
  exist the current one is the truth and the old one is left alone rather than merged.
* The metadata cascade reads `blinkview.json`, falling back to `openfoto.json` and
  renaming it as it reads. Done in the walk the cascade already performs, so migration
  costs no extra pass. A read-only volume keeps working under the old name.
* `sources.json` is **copied**, not moved, out of the old identifier's directory, so a
  previous install still works if someone goes back to it.

Migration is one-way: a library opened by Blinkview is no longer legible to an OpenFoto
build. That is accepted — there is one supported version, and the alternative is
carrying two names in every path for the life of the program.

`Trash/`, `Originals/`, `Duplicates/` and `Scenery/` were never brand-named and do not
move. Nothing in a user's photo folders changes except the two metadata filenames.

## Consequences

* An existing library upgrades in the time it takes to rename two files and a directory,
  keeping its ratings, names, saved searches, arrangements and index.
* The legacy names stay in three places — `library.rs`, `userdata.rs` and the desktop
  `LEGACY_IDENTIFIER` — and are covered by a lifecycle test that builds a pre-rename
  library and asserts it opens whole with its index intact.
* The GitHub repository, Pages URL and release artefacts change name. GitHub redirects
  the old repository URL, so existing clones and links keep working; the update check
  points at the new one.
