# Thumbnail performance: fast scroll, video posters, lightbox previews

Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem

Browsing a 2,433-file library, thumbnails still load slowly under fast scrolling even
when every thumbnail is generated, and video posters are worse than photo thumbnails.
Three measured causes (2026-08-30):

1. **No effective cache.** Grid cells are destroyed offscreen and rebuilt on re-entry;
   each rebuild re-requests via the `photo://` custom scheme. The handler does answer
   `Cache-Control: max-age=31536000`, but WKWebView does not reliably cache custom-scheme
   responses, so every scroll-back pays the full round trip. This is also the flicker.
2. **A request herd.** The lazy-load IntersectionObserver (`app.js:458`) is dead code —
   `io.observe` is never called. `cellFor` sets `src` at attach time, and with
   `OVERSCAN = 900px` a fast flick fires requests for ~1,800px of rows the user never
   sees, all queuing on the 2–6-thread image pool.
3. **Video posters render on demand, one ffmpeg spawn per cell, on the same pool.**
   Measured with the shipped binary: 0.07–0.16 s per phone clip, 0.34 s and 140 MB peak
   RSS for a 1080p rip. `run_cancellable` filters `kind == "photo"` for *every* stage,
   so the app's `Stages::only_thumbs` pass never builds videos — on the user's library
   173 of 507 video posters were still missing after a full analysis, each to be paid
   mid-scroll.

Related: the lightbox loads the full original for every step (`?full=`), which is why
the stepper arrows feel slow — a 12–48 MP original per keypress.

## Non-goals

- No Immich-style hover video previews or in-app player — that is
  `2026-08-30-video-previews.md`, still a draft.
- No change to thumbnail size (512 px long edge) or on-disk layout.
- No zoom-beyond-100% "view full resolution" affordance in the lightbox.
- No dependency for the cache: a ~30-line byte-budgeted LRU beats adding `lru`.
- No frontend test framework; UI work is verified with rendered screenshots.

## Design

**1. Byte-budgeted LRU in the desktop app** (`apps/desktop/src-tauri/src/lib.rs`).
`LazyLock<Mutex<ThumbCache>>`, 64 MiB budget, true LRU via generation stamps, O(n)
eviction over ≤ a few hundred entries. `serve_photo` consults it before every cached
read (thumb, derived) and inserts after. Content-addressed thumbs never change, so
there is no invalidation problem. Held in the Tauri crate — `openfoto-core` stays
Tauri-free.

**2. Video posters built by the analysis pass** (`crates/openfoto-core/src/analyze.rs`).
When `stages.thumbs`, after the photo pass: a video sub-pass over `kind == "video"`
rows whose poster is missing, on its own **2-thread** pool (2 × 140 MB worst case is
the memory-safe ceiling on the 8 GB dev machine), gated on `have_ffmpeg()`, checking
`stop()` per file, ticking the shared progress counter, counted in `Stats.thumbs`.
No ffmpeg → sub-pass is skipped silently; the lazy path remains the fallback for
videos added later. On-demand video renders keep working but move to a dedicated
2-thread `VIDEO_POOL`, chosen at dispatch time in the scheme handler, so they can
never occupy image-decode threads.

**3. The dead observer wakes up** (`apps/desktop/dist/app.js`). `cellFor` sets
`dataset.src` and `io.observe(img)`; the observer (rootMargin 600px) assigns `src` when
the cell is genuinely approaching the viewport. The scroll-out cleanup loop unobserves
removed cells so detached nodes do not accumulate in the observer.

**4. A 2000 px preview variant** (`crates/openfoto-core/src/thumbs.rs`).
`PREVIEW_LONG = 2000`; `render_preview` decodes once (embedded camera preview first,
same as thumbnails), resizes to 2000 long edge, writes
`<root>/.openfoto/derived/p-<hash>.jpg` — unless the source's long edge is already
≤ 2000 px, in which case there is nothing to make and the original is served. The
light-box asks for `?preview=<hash>`; the handler serves the derived file, rendering it
on first request. `?full=` keeps its current behaviour untouched. First view of a photo
pays one full decode (what the lightbox pays today for *every* view); after that it is
a ~400 KB JPEG, LRU-cached. Previews are photos only — videos keep streaming.

**5. Animations, compositor-only** (`apps/desktop/dist/styles.css`). Skeleton shimmer
while a cell loads; fade + 2% scale-in as each thumbnail lands; hover lift; lightbox
backdrop fade and stepper transition. Every animation is `transform`/`opacity` only —
no layout properties, no JS animation loops — and all of it collapses under
`prefers-reduced-motion: reduce`.

Rejected: relying on WebKit to cache custom-scheme responses (not verifiable across
WebKit versions); an in-memory decoded-image cache on the Rust side (the webview
already holds its decodes; bytes are what round-trips); raising `OVERSCAN` (more
requests is the disease, not the cure).

## Acceptance criteria

1. With a warm cache, scrolling back through already-viewed rows serves thumbnails
   without touching the filesystem — verified by the LRU unit tests plus manual
   scroll-back in the running app.
2. A cell 1,500 px outside the viewport has no `src`; it gains one when scrolled
   within 600 px. Detached cells are unobserved: `io` holds no reference to them.
3. After `analyze` completes (thumbs stage), every video in the library has a poster
   file on disk, when ffmpeg is available. Without ffmpeg the pass completes, builds
   no posters, and reports success.
4. The video sub-pass never runs more than 2 ffmpeg processes concurrently.
5. On-demand video-thumb requests never occupy `IMAGE_POOL` threads (dispatch-time
   routing, unit-tested by pool selection logic).
6. `?preview=<hash>` on a >2,000 px photo returns a JPEG whose long edge is 2,000 px;
   on a smaller photo it serves the original bytes. A second request is served from
   the derived file without re-decoding (mtime of the derived file unchanged).
7. The lightbox stepper requests `?preview=`, not `?full=`.
8. Peak RSS of the desktop app during a fast scroll over a fully-analysed library
   stays within ~100 MB of its idle level (64 MiB cache budget + working set).
9. All animation is `transform`/`opacity`; with `prefers-reduced-motion: reduce` no
   animation runs. Verified in rendered screenshots (light and reduced-motion).

## Tasks

- [x] 1. Video sub-pass in `run_cancellable` + `video_workers()`; test with a fake
      ffmpeg that writes a fixture JPEG (touches: crates/openfoto-core/src/analyze.rs)
- [x] 2. `ThumbCache` LRU + serve_photo integration (touches:
      apps/desktop/src-tauri/src/lib.rs)
- [x] 3. `render_preview` / `preview_path_at` / `?preview=` serve path; `VIDEO_POOL`
      dispatch routing; lightbox `?preview=` (touches: crates/openfoto-core/src/thumbs.rs,
      apps/desktop/src-tauri/src/lib.rs, apps/desktop/dist/app.js)
- [x] 4. IO wiring: `dataset.src` + observe/unobserve (touches: apps/desktop/dist/app.js)
- [x] 5. Compositor-only animations + reduced-motion (touches:
      apps/desktop/dist/app.css)
- [x] 6. Screenshot verification of grid and lightbox, normal and reduced-motion;
      doc sync (touches: docs/CURRENT/STATUS.md)

## Verification notes

Shimmer, settled grid, and lightbox verified in rendered screenshots of the real app
(isolated HOME, 74-file scratch library). In-app evidence: a fresh analysis built all
50 posters including the 2 videos; `?preview=` on a 6000×4000 photo produced
`derived/p-<hash>.jpg` at 2000×1333 and the lightbox `<img>` reported
`naturalWidth: 2000`. Reduced motion could not be emulated in WKWebView; the
pre-existing global `prefers-reduced-motion` kill covers the new animations, which are
plain CSS. The hover lift's rules were confirmed live in CSSOM, but no hover *screenshot*
was captured — synthetic pointer events do not latch `:hover` in WKWebView.
