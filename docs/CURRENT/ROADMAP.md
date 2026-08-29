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

## Platforms

**Now: macOS, Windows, Linux.** One desktop application, three targets.

**Later: phone apps as *clients*.** The phone does not run the library — it connects to
the desktop app over the network, the way Immich and Ente do. That matters because it
leaves ADR-0009 and ADR-0010 untouched: folders and `openfoto.json` stay on a real
filesystem on the machine that holds the photographs, and the phone never needs one.
A native mobile port would have contradicted both decisions, since iOS has no
user-visible filesystem and Android's scoped storage is not one either.

Known work before Windows and Linux are real:

- **HEIC decoding** shells out to macOS `sips` (ADR-0005). Needs libheif, or an
  equivalent, on the other two.
- **Video poster frames** need ffmpeg present; it is optional today, so they degrade to
  no poster rather than failing.
- **Windows filename rules** go beyond exFAT's character set: reserved device names
  (`CON`, `NUL`, `COM1`), no trailing dots or spaces, and a 260-character path limit
  unless long paths are enabled. `fsops::RESERVED` covers the characters only.
