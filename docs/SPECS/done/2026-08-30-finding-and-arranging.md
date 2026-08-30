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
**Find (⌘F)** focuses the search field. The field already matches filename and path
(`matchesQuery`), so a second surface for typing a name was a duplicate — a finder
modal was built first and removed on review for exactly that reason.

What the field could not do was look outside where you are standing: a query is ANDed
with the selected folder, so searching for a file from another folder answered
"nothing" while the file sat in the library. So the chip row gains one chip when a
query matches photographs the current folder or person is hiding — *"2 elsewhere —
search all of lib"* — which clears the narrowing and keeps the query.

Rejected: making search always library-wide. Filtering the folder you are looking at
is the common case and the reason the field is ANDed with it; the fix is to say what
is being hidden, not to stop hiding it.

**Per-folder view** lives in that folder's own `blinkview.json`, beside the ratings of
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
1. ⌘F (and Ctrl+F) focuses and selects the search field, from anywhere including
   another field.
2. A query that matches nothing in the selected folder, but something outside it,
   shows a chip counting the matches elsewhere.
3. Clicking that chip clears the folder and person, keeps the query, and shows them.
4. With nothing hidden by the current view, no chip appears.
5. A folder with no `view` sorts newest-first, as today.
6. Changing the sort while a folder is selected writes `view.sort` to that folder's
   `blinkview.json`, and reopening the folder — or relaunching — restores it.
7. Two folders can hold different sorts at once, and neither inherits the other's.
8. Dragging a cell onto another position reorders it, persists the order, and survives
   a relaunch; the sort control shows `Custom`.
9. A photograph added to a custom-ordered folder afterwards appears at the end rather
   than being dropped from the view.
10. Setting the sort back to Newest leaves `order` on disk untouched, so switching back
    to Custom restores the arrangement.

## Tasks
- [x] 1. `UserData.view` + `FolderView` in core, exact-folder read/write helpers, tests
      (touches: crates/blinkview-core/src/userdata.rs)
- [x] 2. `folder_view` / `set_folder_view` commands (touches: apps/desktop/src-tauri/src/lib.rs)
- [x] 3. Find modal: markup, styles, matching, keyboard (touches: apps/desktop/dist/*)
- [x] 4. Per-folder sort: load on folder select, save on change (touches: apps/desktop/dist/app.js)
- [x] 5. Drag-to-reorder over the virtualised grid (touches: apps/desktop/dist/app.js, app.css)

## Verification notes
Driven in the running app against a seven-photograph fixture (root + `Day1`):

- ⌘F focused the search field (`document.activeElement` = `search`).
- Standing in `Day1`, searching `beach` — a file at the library root — showed 0 in the
  folder and the chip *"1 elsewhere — search all of lib"*. Clicking it gave
  `lib · 1 photos` holding `beach.jpg`, and the chip went away because nothing was
  hidden any more.
- Setting Day1 to Name wrote `{"view":{"sort":"name"}}` to `Day1/blinkview.json` and
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
permission), so the handler was driven with a synthetic `keydown` carrying `metaKey`
— the field takes focus and the event is `defaultPrevented`. The app installs no menu,
so nothing claims ⌘F ahead of the page.
