# Organising files
Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem
Folders are the only grouping blinkview has (ADR-0009), but a folder can only be brought
into existence as a side effect of moving photographs into a name typed in the move bar
(`lib.rs:1487`). There is no "new folder". Deleting always lands in the library `Trash/`
(`lib.rs:1632`) with no way to say "put these somewhere else instead". And renaming in
bulk is one fixed format over the whole library — `%I-%M-%S_%p_%d_%b_%Y`
(`rename.rs:18`), no pattern of your own and no way to rename just the twelve you
selected, which STATUS already lists as a known gap.

## Non-goals
- No rename *rules* engine (find/replace, regex, case conversion). One pattern of date
  and counter tokens, previewed before it runs.
- Deleting to a chosen folder is still a move inside the library — never outside it,
  and never the system Trash, which stays the separate explicit "Empty…" step.
- No folder rename or folder delete in this spec.

## Design
**New folder** is a ＋ on the Folders heading, mirroring the ＋ on Sources. It creates
`<selected folder>/<name>` — the selected folder is the visible parent, so the button
means what the sidebar shows. `create_folder` validates through
`fsops::validate_filename` (exFAT reserved characters, ADR-0001's reference drive) and
refuses a name that already exists rather than silently adopting it. An empty folder
holds no photographs, so it is invisible to `describe()`'s index-derived tree until
something is moved in; it is listed from disk instead, marked `own: 0`.

**Delete to a folder** extends `delete_photos` with `dest: Option<String>`, defaulting
to `Trash`. It is the same journalled Move plan either way, so ⌘Z reverses it
identically. The context menu gains "Delete to…" beside "Delete"; the Delete key stays
Trash, because a keystroke should not ask a question.

**Bulk rename** gains a scope and a pattern. `rename::plan_scoped(lib, format, Option<&
[String]>)` renames the given hashes, or the whole library when `None` — the existing
`plan` becomes a call to it with `None`, so the CLI is unchanged. The sheet gains a
pattern field seeded with the default, a live preview of the first five results built
from the same `stem_for` the plan uses (so the preview cannot disagree with the run),
and a scope line saying exactly what it will touch: the selection if there is one, the
current folder otherwise.

Rejected: a free-form pattern language of our own. chrono's tokens already do dates,
are documented, and are what `stem_for` consumes; `%%n` is added on top for a counter,
which is the one thing chrono has no notion of.

## Acceptance criteria
1. ＋ on Folders, with `Trip` selected, creates `Trip/<name>` and the row appears
   without a relaunch; with nothing selected it creates at the library root.
2. A name containing `/` or `:` is refused with the reason, and nothing is created.
3. Creating a name that already exists is refused; the existing folder is untouched.
4. An empty new folder is listed in the sidebar with a count of 0.
5. "Delete to…" offers existing folders and a free-text name, moves the selection
   there, and ⌘Z puts every photograph back where it was.
6. The Delete key and the context menu's "Delete" still go to `Trash`.
7. Renaming with a selection touches only the selected photographs.
8. Renaming with no selection and a folder selected touches only that folder and below.
9. The previewed names equal the applied names for the same input.
10. `%%n` numbers within the plan from 1, zero-padded to the width of the largest index.
11. A pattern producing a name another file already has gets a `_N` suffix, never an
    overwrite — uniqueness is checked library-wide, not within the scope. A pattern
    producing an *invalid* name (a reserved character) is skipped and reported.

## Tasks
- [x] 1. `create_folder` command + sidebar ＋ and dialog (touches: lib.rs, app.js, app.css)
- [x] 2. Disk-listed empty folders in `describe()` (touches: lib.rs)
- [x] 3. `delete_photos` dest + "Delete to…" (touches: lib.rs, app.js)
- [x] 4. `rename::plan_scoped` + `%%n` counter + tests (touches: crates/blinkview-core/src/rename.rs)
- [x] 5. Rename sheet: pattern field, live preview, scope line (touches: lib.rs, app.js)

## Verification notes
Driven in the running app against a five-photograph fixture (root + `Day1`):

- `create_folder` made `Keepers` at the root and `Day1/Best` nested. `bad/name` was
  refused — *filename "bad/name" contains reserved character '/'* — and a second
  `Keepers` was refused as *Keepers already exists here*. Both empty folders then
  appeared in `list_sources` as `Keepers=0` and `Day1/Best=0`, which is the point of
  listing folders from disk as well as from the index.
- Deleting one photograph to `Keepers` reported *Moved 1 to Keepers · undo id …* and
  put it at `Keepers/20260820_120101.jpg`; ⌘Z restored it to the root.
- Renaming with `Day1`'s two hashes as the scope touched only those two; the three
  root files kept their names exactly.
- The previewed names and the applied names were identical
  (`Day1/2026-08-21_1.jpg`, `Day1/2026-08-21_2.jpg`), which is structural: preview and
  apply call the same `plan_scoped`.
- `shot_%%n` over the whole library numbered in capture order across folders —
  `shot_1..shot_3` at the root, `shot_4`, `shot_5` in `Day1`, whose photographs were
  taken a day later. A twelve-file unit test covers the zero-padding (`shot_01`…
  `shot_12`), since a bare counter sorts 10 before 2 in every file browser.
- `%Q` was refused as *"%Q" is not a pattern I can read* rather than panicking inside
  chrono, which is what an unvalidated user-typed pattern does.
