# Video previews and playback
Status: Draft   Owner: notdefined   Date: 2026-08-30

## Problem

A video in the grid is a static poster frame with a play badge, and opening one plays
the original through a bare `<video>` element (`app.js:1046`). Two gaps: there is no way
to see what a clip contains without opening it, and WKWebView plays H.264/AAC reliably
but is patchy on VP9 and absent on AV1 and Matroska — an unplayable file currently just
shows nothing, with no message.

Depends on `2026-08-30-ffmpeg-sidecar.md`. Do not start before it lands: every
derivative here needs an ffmpeg that is known to exist.

## Non-goals

- No video editing, trimming, or filters.
- No audio in hover previews. They are muted and have no audio track at all.
- No adaptive streaming or HLS. Range requests over `photo://` already work
  (`serve_file`, 4 MB chunks) and are sufficient for local files.
- No transcode-everything pass. A transcode happens on demand, once, and is cached.
- No preview for a video shorter than 2s — the poster frame is the preview.

## Design

Two derivatives per video, both in `.openfoto/derived/`, beside the HEIC JPEGs that
already live there. Both are disposable cache and rebuildable (ADR-0001, ADR-0011).

| file | what | budget |
|---|---|---|
| `<hash>-preview.mp4` | 3s from 10% in, ≤480px long edge, muted, H.264 | ≤ 400 KB |
| `<hash>-play.mp4` | full clip transcoded to H.264/AAC, only when the original will not play | — |

Served by the existing `photo://` scheme with new query parameters, matching the
`?t=` / `?full=` pattern already there:

| request | serves |
|---|---|
| `?t=<hash>` | poster frame (unchanged) |
| `?preview=<hash>` | hover clip, generated on first request |
| `?play=<hash>` | original, or the cached transcode when the original is unplayable |

Playability is decided by container and codec read from the index, not by trying and
failing: H.264 or HEVC in MP4/MOV plays directly; anything else routes through `?play=`.
Codec is recorded at scan time via ffprobe.

Hover: after **200 ms** on a video cell, the cell swaps its `<img>` for a muted looping
`<video>` on `?preview=`. Leaving the cell removes the element and releases the buffer.
At most **3** preview elements exist at once — a fourth hover evicts the oldest. This
cap is the whole lesson of ADR-0014: uncapped video in the render process is what put
15.7 GB there.

Rejected: animated WebP previews — smaller and simpler, but 3s at 480p is several MB as
WebP against ~300 KB as H.264, and the webview decodes it less efficiently.

Rejected: playing the original on hover — that is the bug of ADR-0014 with a nicer name.

## Acceptance criteria

1. Hovering a video cell for 200 ms plays a muted looping preview; leaving it within
   200 ms never starts one.
2. Preview generation is p95 < 1.5 s for a 30 s 1080p clip on the reference machine.
3. Every generated preview is ≤ 400 KB. A clip whose preview exceeds it is re-encoded at
   lower quality rather than served.
4. Hovering 50 cells in succession leaves at most 3 `<video>` elements in the DOM, and
   `tauri://localhost` RSS returns to within 150 MB of its pre-hover value within 5 s.
5. Scrolling the grid with previews playing holds 60 fps on the reference library.
6. An AV1 or Matroska file plays in the lightbox via a cached transcode; the second open
   uses the cache and starts in under 500 ms.
7. A video whose preview cannot be generated keeps its poster frame and play badge, and
   never causes the original to be served to an `<img>` (ADR-0014).
8. Deleting `.openfoto/derived/` loses no user-authored data and everything regenerates.
9. Previews respect `prefers-reduced-motion`: hover does nothing when it is set.

## Tasks

- [ ] 1. Record codec and container at scan time via ffprobe; migrate the index
      (touches: crates/openfoto-core/src/scan.rs, index.rs)
- [ ] 2. `thumbs::preview_clip()` writing `<hash>-preview.mp4` to `derived/`, with the
      size budget enforced (touches: crates/openfoto-core/src/thumbs.rs)
- [ ] 3. `?preview=` in `serve_photo`, generated on first request (touches:
      apps/desktop/src-tauri/src/lib.rs)
- [ ] 4. Hover interaction with the 200 ms delay and the 3-element cap (touches:
      apps/desktop/dist/app.js)
- [ ] 5. `?play=` with on-demand transcode and playability decided from the index
      (touches: lib.rs, app.js)
- [ ] 6. Doc sync: ADR for the derivative cache shape if it diverges from ADR-0001;
      STATUS.md; move this spec to done/ (touches: docs/)
