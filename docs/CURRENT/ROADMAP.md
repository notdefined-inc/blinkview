# Roadmap

## Now
Nothing in flight. All three planned phases have shipped; the v1 spec is in
`docs/SPECS/done/`.

## Next
- **Portability.** HEIC is macOS-only (ADR-0005) and the app is untested off macOS.
  A Linux/Windows port needs a real HEIC decoder and exFAT/AppleDouble handling review.
- **Packaging.** `bundle.active` is false, so there is no installable .app yet.
- **Detection thresholds.** The 4% scenery ratio was tuned on the older YuNet export and
  has not been re-confirmed against the current one (ADR-0004).
- **Two `saurabh -> Me` misassignments** in the accuracy fixture, cause unresolved:
  matcher error or a mislabel in a fixture that a semi-automatic process produced.

## Later
RAW, cross-source search, albums, sharing, a real preferences surface.

## Done
Phase 1 — `scan`, `dedupe`, `rename`, `undo`, journal/undo core, exFAT handling.
Phase 2 — faces end to end: detection, embeddings, clustering, assignment, review,
`scenery`, and filing by person.
Phase 3 — Tauri desktop app: multi-folder sources, virtualised date-grouped grid,
lightbox with zoom/pan and folder context, in-app people review, selection and bulk
actions, trash with restore, per-photo rename, per-person untagging, progress
reporting, model fetching, HEIC, video, drag-and-drop folders.

Beyond v1: RAW/HEIC, geotagging, tag hierarchies, plugins.
