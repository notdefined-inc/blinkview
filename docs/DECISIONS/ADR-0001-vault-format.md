# ADR-0001: The vault format — folders are the database

Date: 2026-08-27
Status: Accepted

Status note (2026-08-31): the *disposability* below stands; the *placement* — beside the photographs — was superseded by ADR-0019. The journal's move to non-derivable is also ADR-0019's amendment.

## Context

Every free photo manager surveyed owns the library through a database. digiKam, Immich,
PhotoPrism and Apple Photos all index files and then express organization as *tags* inside
their own store. Uninstall the app and the organization evaporates; the folders on disk are
untouched and unsorted.

The user's stated goal is "what Obsidian is to Markdown": the files on disk are the product,
the app is a fast lens over them. A survey found no existing product in this niche.

There is also direct evidence for the constraint. While the prototype was mid-run, the user
renamed `Person1/` to `Alex/` in Finder. A path-keyed tool treats that as corruption — the
run crashed and silently failed to move two files. A folder-as-truth tool must treat it as
ordinary.

## Decision

The library is any folder of photos in ordinary subfolders. Organization *is* the folder
layout — there is no tag layer.

`.blinkview/` inside the library root holds `index.sqlite`, `thumbs/`, `journal/` and
`people.json`. It is **entirely derived**. `rm -rf .blinkview && blinkview scan` must rebuild
everything with no loss of user-visible state.

File identity is the **BLAKE3 content hash**, never the path. External moves and renames are
re-detected on the next scan.

Every mutation writes a journal entry before touching disk; `undo` replays it backwards.

## Consequences

Good: no lock-in — walking away leaves a sorted library. Finder and the tool can be used
interchangeably. Sync tools (Dropbox, git-annex) work, since photos are just files. Undo is
a real feature rather than a manifest we hand-roll each time.

Costly: rescanning must hash files to detect external changes, which is I/O-bound on large
libraries (mitigated by size+mtime fast-path, hashing only on mismatch). Anything users want
preserved must live in the folder structure or in filenames, since the index is disposable.
Names of people in `people.json` are the one soft spot — they are cheap to re-enter, and the
review step exists to make that quick.
