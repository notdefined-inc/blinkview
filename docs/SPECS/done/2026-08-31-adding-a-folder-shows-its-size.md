# Adding a folder shows its size first

Status: Shipped   Owner: somesh   Date: 2026-08-31
Sequenced after `2026-08-31-peeking-at-a-folder.md`, but independent of it: either can
ship alone.

Executor note: written for someone with no prior context on this codebase. Every file
reference is a real path and every line number was correct at `478c608`.

## Problem

`add_source` (`apps/desktop/src-tauri/src/lib.rs:642`) takes a path, saves it, and
returns. The scan that follows is `WalkDir::new(&root)` with **no depth limit and no
exclusions** (`crates/blinkview-core/src/scan.rs:84`). Point it at a home directory or a
whole disk and it walks everything, indexing for as long as that takes, having told the
user nothing beforehand. `source_conflict` (`lib.rs:601`) only refuses folders that
overlap an existing source; size is never considered.

Two specific harms, not hypotheticals:

* **No warning proportional to the cost.** Adding `~` and adding `~/Pictures/Trip` look
  identical at the moment of choosing, and differ by four orders of magnitude.
* **`~/Library` gets indexed.** On macOS a home directory contains tens of thousands of
  cached PNGs and JPEGs under `~/Library/Caches`, `Containers` and
  `Application Support`. They are not photographs, and they would fill the grid.

Picasa asked this question at first run — [scan the whole computer, or just
Documents/Pictures/Desktop](https://sites.google.com/site/picasaresources/picasa/getting-started)
— and every guide recommended the narrow answer. Its Folder Manager then made the choice
**per folder**, three ways: [Scan Always, Scan Once, Remove from
Picasa](https://sites.google.com/site/picasaresources/picasa/how-picasa-works). Immich
took the other route: always recursive, with [glob exclusion
patterns](https://docs.immich.app/features/libraries/) as the only escape.

## Non-goals

- **Picasa's "Scan Once" state.** Blinkview watches every source with a debounce
  (`apps/desktop/src-tauri/src/watch.rs`), which is the better version of "Scan Always";
  a third never-recheck state is a separate feature and not needed to fix this.
- **A global preference.** Depth belongs to the folder, as it did in Picasa's Folder
  Manager. A setting that governs every future add is the thing that becomes wrong.
- **A user-editable exclusion box.** Ship the defaults; a pattern editor is its own spec
  with its own UI questions.
- **Changing anything about how an already-added source scans.** Existing sources keep
  their current behaviour — recursive, no exclusions — unless the user edits them.
- **Blocking a large add.** The user may add their whole disk; the requirement is that
  they do it knowingly, not that they are prevented.

## Design

### Count before committing

A new command, run when a folder is chosen and before it is added:

```rust
#[tauri::command] async fn survey_folder(path: String) -> R<Survey>;

pub struct Survey {
    /// Photographs and videos directly inside the folder.
    here: usize,
    /// Everything beneath it, excluded paths already removed. `None` when the walk hit
    /// `SURVEY_LIMIT` — the honest answer for a disk is "more than 200,000", not a
    /// number arrived at after ninety seconds of walking.
    below: Option<usize>,
    subfolders: usize,
    /// Excluded directory names actually encountered, so the dialog can say which.
    excluded: Vec<String>,
}
```

`survey_folder` walks with the same `kind_of` filter `scan` uses, stops at
`SURVEY_LIMIT: usize = 200_000` entries, and is cancellable. It reads directory entries
only — no hashing, no EXIF, no decoding.

### The dialog

The count is the warning; a warning without a number is noise.

> **Add “Desktop”?**
> 340 photographs directly in this folder.
> About 84,000 more in 12,000 subfolders — skipping `Library`.
>
> [This folder only]  [Include subfolders]  [Cancel]

Rules: **This folder only** is the default focus when `below > 20 × here` or
`below > 50_000` — the cases where recursion is the surprising choice. Otherwise
**Include subfolders** is focused, matching today's behaviour. When `below` is `None` the
line reads "More than 200,000 more" and **This folder only** is always the default.
When there are no subfolders the dialog does not appear at all; adding proceeds.

### Depth is a property of the source

`sources.json` holds bare strings today (`SourcesFile`, `lib.rs:131`). It becomes a list
that may hold either, so an existing file still loads:

```rust
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SourceEntry {
    /// Written before this spec: recursive, which is what it did.
    Legacy(String),
    Full { path: String, #[serde(default)] shallow: bool },
}
```

`Library` carries the flag; `scan` uses `scan_shallow` when it is set. A source's depth is
changeable afterwards from its context menu — switching to recursive scans the rest,
switching to shallow drops the rows below the root, both without losing ratings, which
live beside the photographs (ADR-0007) and not in the index.

### Exclusions

```rust
// crates/blinkview-core/src/scan.rs
/// Directory names never walked. Not photographs, and on macOS a home directory holds
/// tens of thousands of cached images under `Library` alone.
pub const SKIP_DIRS: &[&str] = &[
    "Library", "Applications", "System", "Volumes", "private", "node_modules",
    ".git", ".Trash", "$RECYCLE.BIN", "Windows", "Program Files",
];
```

Matched on the **directory name at any depth**, the same way `scan` already skips
`VAULT_DIR` and `LEGACY_VAULT_DIR` in its `filter_entry`. A folder the user adds
*directly* is never skipped by its own name — adding `~/Library/Photos` on purpose must
work; the rule applies to directories encountered while descending.

### Rejected

- *A size warning without counting.* "This looks like a large folder" tells the user
  nothing they did not already know, and would fire on a 3 GB trip folder that is
  perfectly reasonable to add.
- *Counting during the scan and offering to abort.* By then the index has rows in it and
  the cache exists; the choice has to come first.

## Acceptance criteria

1. Choosing a folder with subfolders shows the dialog with the direct count, the
   recursive count, the subfolder count, and any excluded directory names encountered.
2. Choosing a folder with no subfolders adds it with no dialog.
3. **This folder only** adds a source that indexes exactly the files directly in it; a
   file one level down never appears in the grid.
4. **Include subfolders** reproduces today's behaviour exactly.
5. Surveying a folder of over 200,000 entries returns `below: None` within 5 s, and the
   dialog says "more than 200,000" rather than a number.
6. Cancelling the dialog adds nothing: `sources.json` is unchanged and no cache is made.
7. Surveying `~` reports `Library` among `excluded`, and adding `~` recursively indexes
   nothing from `~/Library`.
8. Adding `~/Library/Photos` directly indexes it: the skip list applies while descending,
   never to the chosen folder itself.
9. A `sources.json` written before this change loads, and every source in it stays
   recursive.
10. Switching a source from shallow to recursive indexes the rest without a full rehash;
    switching back removes the deeper rows and keeps every rating and label.
11. The survey reads directory entries only — no file is opened, hashed or decoded.
    Verified by surveying a folder on a read-only volume.

## Tasks

- [x] 1. `scan::SKIP_DIRS` and the descend-only skip rule, with the chosen root exempt
      (touches: `crates/blinkview-core/src/scan.rs`)
- [x] 2. `survey_folder` with `SURVEY_LIMIT` and cancellation (touches: `scan.rs`,
      `apps/desktop/src-tauri/src/lib.rs`)
- [x] 3. `SourceEntry` with untagged legacy strings; `shallow` through `Library` into
      `scan` (touches: `lib.rs`, `library.rs`, `scan.rs`)
- [x] 4. The dialog, its default-focus rule, and wiring `addSource` (`app.js:806`,
      `:3750`, `:2189`) through the survey (touches: `app.js`, `index.html`, `app.css`)
- [x] 5. Change a source's depth from its context menu (touches: `lib.rs`, `app.js`)
- [x] 6. Tests: depth honoured both ways, skip list on descent but not on the root,
      legacy `sources.json` loads, survey opens no files, limit returns `None`
      (touches: `tests/lifecycle.rs`, `scan.rs` unit tests)
- [x] 7. Docs: STATUS, ARCHITECTURE, README, landing page
