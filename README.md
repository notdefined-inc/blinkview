# openfoto

A local-first photo organizer. Your folders are the database.

Every other free photo manager indexes your library into its own store and expresses
organization as tags inside it. Uninstall it and your folders are still an unsorted heap.
openfoto does the opposite: it moves and renames actual files, so the organization survives
the tool. What Obsidian is to Markdown.

    openfoto scan       index a library (safe, never mutates)
    openfoto dedupe     find burst near-duplicates
    openfoto faces      cluster people, review, sort into folders
    openfoto scenery    split shots with no close-up people
    openfoto rename     bulk date-time filenames
    openfoto undo       reverse any operation

Nothing destructive happens without `--apply`. `.openfoto/` is a disposable cache — delete
it any time and `scan` rebuilds it.

Status: Phase 0. Scaffolded, not yet implemented. See `docs/CURRENT/STATUS.md`.
