# Mobile responsive — the same UI, usable on a phone

Status: Done (shipped 2026-08-31)   Owner: somesh   Date: 2026-08-31

## Problem

The remote bridge ships the desktop layout to the phone unchanged: a fixed 256px
sidebar and 64px titlebar leave ~134px of grid on a 390px screen (screenshot on
file), zoom and arrows are keyboard/wheel-only, and context menus assume a mouse.
The grid engine itself is already width-agnostic (`justify(view, width, ROW_H)`) —
the work is chrome, not the layout engine.

## Non-goals

- No second UI, no separate mobile code path, no framework. One frontend, made
  responsive; desktop rendering above 760px must not change by one pixel.
- No phone-app chrome (no bottom tab bar, no pull-to-refresh). The desktop
  information architecture carries over: sidebar becomes a drawer, everything else
  adapts in place.
- No native app, no gestures beyond the ones below.

## Design

One breakpoint (`max-width: 760px`) for layout, one pointer query
(`pointer: coarse`) for touch affordances, both token-driven under DESIGN.md.
Below the breakpoint:

- **Drawer**: the sidebar slides off-canvas (`transform`, not `display`, so the
  tree state survives); a hamburger button in the titlebar opens it; a scrim, a
  source choice, or Escape closes it. `--tb` drops 64px → 52px; the library
  header loses its `left: var(--side)`.
- **Touch**: `touch-action: manipulation` and transparent tap highlights sitewide;
  minimum 40×40px targets on coarse pointers; long-press (500ms) opens the same
  context menu right-click does, with a guard against the native event firing too;
  lightbox gains swipe-to-navigate (disabled while zoomed), pinch-to-zoom, and
  double-tap to toggle 1x; `lb-nav` arrows hide on coarse pointers; arrows/space
  keep working where a keyboard exists.
- **Safe areas**: `viewport-fit=cover` plus `env(safe-area-inset-*)` on the
  titlebar and lightbox chrome.
- **Overlays**: dup-review's 224px column stacks; sheets already clamp to 92vw.
- **Dev affordance**: `BLINKVIEW_DIST_DIR` makes the bridge serve the frontend
  from a directory at request time instead of the compile-time embed, so the
  screenshot loop edits CSS and reloads instead of rebuilding the app.

## Acceptance criteria

1. At 390×844: sidebar off-canvas, grid full-width with ≥2 photos per justified
   row, no horizontal scrolling anywhere (verified by screenshot).
2. Drawer: hamburger opens it over a scrim; picking a source, tapping the scrim,
   or pressing Escape closes it.
3. Every interactive element is ≥40×40px at 390px width on coarse pointers.
4. Lightbox on coarse pointer: swipe navigates, pinch zooms (past 1x loads the
   full-res original via the existing zoom path), double-tap toggles zoom, no
   arrow buttons.
5. Long-press on a photo opens the same context menu as desktop right-click;
   desktop right-click is unchanged.
6. At 1440×900 nothing changes versus today (screenshot comparison; no rule
   outside the new media queries touches desktop).
7. Grammar test, workspace tests, clippy `--all-targets -D warnings` green; no
   console errors on the served page.

## Tasks

- [ ] 1. `BLINKVIEW_DIST_DIR` serving in the bridge (touches: remote.rs)
- [ ] 2. Viewport/safe-area/touch-action base + viewport-fit (touches: index.html, app.css)
- [ ] 3. Mobile media query: drawer, hamburger, titlebar, libhead, dup-review (touches: index.html, app.css)
- [ ] 4. JS: drawer toggle + close-on-select, long-press menu, lightbox swipe/pinch/double-tap (touches: app.js)
- [ ] 5. Screenshot passes: 390×844 grid/drawer/lightbox, 1440×900 desktop regression (touches: none)
- [ ] 6. Gates, reinstall the installed app, docs, commit (touches: docs)

## Shipped (2026-08-31)

Verified by rendered screenshots through the bridge, not by reading code. Before:
390×844 showed a 256px sidebar eating the screen with a one-column sliver of grid.
After: full-width justified grid (5-across), drawer with Sources/People/Folders/Trash
over a scrim, compressed titlebar with a hamburger, lightbox full-bleed with the
filmstrip. Desktop at 1440×900 is unchanged (screenshot compared). Two passes were
made; pass 1 found the logo crowding the search pill and the theme button clipping at
the right edge, both fixed by hiding the logo and letting the search flex. Touch
gestures (long-press menu, swipe, pinch, double-tap) are wired through pointer-coarse
guards and need hands-on confirmation on a real phone — the desktop automation
browser cannot synthesize touch. `BLINKVIEW_DIST_DIR` landed with this spec so the
verification loop was edit-and-reload against a running bridge instead of a rebuild
per pass.
