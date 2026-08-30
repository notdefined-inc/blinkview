# Finding and arranging
Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem
Two gaps in getting to a photograph and keeping it where you put it. Finding a file by
name works — free text already matches filename and folder (`app.js:1529`) — but only
by typing into the omnibar, which also parses dates, people and scenes; there is no
plain "find this file" that a keyboard reaches. And a folder cannot be arranged: the
sort is one in-memory value for the whole window (`S.sort`, `app.js:48`), so it is
forgotten on relaunch and shared across every folder, and there is no way to say "this
one goes first" at all.

## Non-goals
- Not a second query language. The modal matches literal filename and path only; dates,
  people, ratings and scenes stay in the omnibar.
- No cross-source search. The modal searches the open library.
- No manual ordering of a *search result* or a person view — an order belongs to a
  folder, which is the only thing that owns its photographs (ADR-0009).
- Sorting stays client-side; no index changes.

## Design
**Find (⌘F)** is a modal over the grid. It matches every word, case-insensitively,
against `name` and `folder` of `S.photos` — the whole library, not the selected folder,
because "where is that file" is a library-wide question. Rows show thumbnail, name and
folder; ↑/↓ move, Enter opens the lightbox, ⇧Enter reveals the photograph in the grid
(selecting its folder and scrolling to it), Esc closes. Capped at 300 rows: past that
the answer is to type more, not to scroll.

Rejected: extending the omnibar with a `file:` prefix — it keeps the parse ambiguity
that made a dedicated modal worth having, and gives no result list to arrow through.

**Per-folder view** lives in that folder's own `openfoto.json`, beside the ratings of
the photographs it holds (ADR-0010). A folder describing how it is arranged is the same
kind of fact as a folder describing what is in it, and it travels with the folder when
it is copied in Finder. New optional field, absent unless set:

    "view": { "sort": "custom", "order": ["<hash>", "<hash>", …] }

`sort` is one of newest|oldest|name|rating|size|custom. `order` is only read for
`custom`; hashes not listed fall after the listed ones in newest-first order, so a
photograph added later appears without disturbing the arrangement. Reading uses the
*exact* folder's file, never the cascade — an inherited arrangement would silently
reorder a subfolder nobody arranged.

Custom order is set by dragging a cell onto another. Dragging while another sort is
active switches that folder to `custom`, seeded from what is on screen, so the first
drag never scrambles the rest.

## Acceptance criteria
1. ⌘F (and Ctrl+F) opens the modal with the field focused; Esc closes it and returns
   focus to the grid.
2. Typing `img_2` lists only photographs whose filename or folder contains `img_2`,
   from anywhere in the library, including folders that are not selected.
3. Two words match a photograph only if both appear ("day1 jpg" matches
   `Trip/Day1/a.jpg`, "day1 zzz" matches nothing).
4. ↑/↓ move the highlight without leaving the field; Enter opens the highlighted
   photograph in the lightbox; ⇧Enter selects its folder and scrolls it into view.
5. A folder with no `view` sorts newest-first, as today.
6. Changing the sort while a folder is selected writes `view.sort` to that folder's
   `openfoto.json`, and reopening the folder — or relaunching — restores it.
7. Two folders can hold different sorts at once, and neither inherits the other's.
8. Dragging a cell onto another position reorders it, persists the order, and survives
   a relaunch; the sort control shows `Custom`.
9. A photograph added to a custom-ordered folder afterwards appears at the end rather
   than being dropped from the view.
10. Setting the sort back to Newest leaves `order` on disk untouched, so switching back
    to Custom restores the arrangement.

## Tasks
- [x] 1. `UserData.view` + `FolderView` in core, exact-folder read/write helpers, tests
      (touches: crates/openfoto-core/src/userdata.rs)
- [x] 2. `folder_view` / `set_folder_view` commands (touches: apps/desktop/src-tauri/src/lib.rs)
- [x] 3. Find modal: markup, styles, matching, keyboard (touches: apps/desktop/dist/*)
- [x] 4. Per-folder sort: load on folder select, save on change (touches: apps/desktop/dist/app.js)
- [x] 5. Drag-to-reorder over the virtualised grid (touches: apps/desktop/dist/app.js, app.css)

## Verification notes
Driven in the running app against a seven-photograph fixture (root + `Day1`):

- `dsc` from the library root listed both `Day1` files — library-wide, not the
  selected folder. `day1 img` matched only `IMG_9999.jpg`; `day1 zzz` matched nothing.
- ↑/↓ moved the highlight 0→1→2→1 with focus never leaving the field. Enter opened
  `DSC_0001.jpg` and → stepped to `DSC_0002.jpg`. ⇧Enter switched the view to
  `lib › Day1`, selected `IMG_9999.jpg` and scrolled to it.
- Setting Day1 to Name wrote `{"view":{"sort":"name"}}` to `Day1/openfoto.json` and
  left the root with **no file at all** — no inheritance, no litter. Leaving Day1 gave
  Newest; re-entering restored Name.
- Dragging `DSC_0001.jpg` onto the right half of `IMG_9999.jpg` produced
  `DSC_0002, IMG_9999, DSC_0001`, switched the sort control to Custom, disabled the
  grouping control and dropped the headings. The order persisted as three hashes.
- Switching to Newest and back to Custom restored the arrangement, so `order` survives
  a sort change.
- A photograph copied into the arranged folder from outside appeared at the **end**
  (4 photos) rather than disappearing.

Not verified: the literal ⌘F keystroke. Neither the MCP keyboard bridge nor
`osascript` can deliver a modified keystroke to the window here (Accessibility
permission), so the handler was driven with a synthetic `keydown` carrying
`metaKey` — modal opens, field focused, event `defaultPrevented`. The app installs no
menu, so nothing claims ⌘F ahead of the page.
