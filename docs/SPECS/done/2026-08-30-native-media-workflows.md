# Native media workflows
Status: Shipped 2026-08-30 · Owner: notdefined · 2026-08-30

## Problem

Blinkview indexes common videos, accepts Finder folders, and provides timeline sorting,
but the everyday macOS workflows around those capabilities are incomplete: video uses
obtrusive native controls, Live Photo pairs appear as separate files, export has no
system share sheet, the timeline lacks a quick month jump, and releases are invisible.

## Non-goals

- No Photos.app library integration or proprietary Live Photo metadata database.
- No background updater, silent install, telemetry, or photo/library data in update
  requests.
- No duplicate album hierarchy (ADR-0009).
- No permanent export cache. Any rendered share files are disposable derivatives.

## Design

### Video and Live Photo pairs

MOV, MP4, and M4V stay ordinary indexed media. The lightbox replaces browser controls
with a small floating transport pill: play/pause, time, scrubber, mute, and fullscreen.
Only that pill changes on hover; no full-video shade is painted.

A photo and MOV in the same folder with the same filename stem are presented as one
Live Photo tile. The still is always the initial and fallback paint. Pressing for 250ms
starts muted playback behind it; the still fades only after `canplay`, so slow or
unsupported video never creates a blank tile. Release restores the still immediately.

### Timeline navigation

Newest/Oldest is a compact segmented control beside the timeline title. A month menu is
derived from the visible capture dates and scrolls to the existing virtual-layout day
block without re-querying the index.

### Native export

Share presents macOS's standard `NSSharingServicePicker` with file URLs, which supplies
AirDrop, Messages, Mail, and Save to Files according to the apps installed on the Mac.
The adapter is macOS-only and implemented with maintained `objc2` bindings; no shell or
raw path primitive is exposed to the webview. Paths are resolved from selected content
hashes in Rust. Saved edits are already baked into those files; future non-destructive
edits must render a disposable, hash-addressed share derivative first.

Finder outbound drag is a native edge adapter too. The browser starts a drag by content
hash, Rust resolves validated file URLs, and AppKit owns the `NSDraggingSession`. Inbound
folder drop remains the current Tauri event path.

### Release checks

At launch, and when `Check for Updates…` is chosen, the backend requests GitHub's latest
release metadata with an explicit `Blinkview/<version>` user agent. It sends no library,
path, photo, search, or hardware data. A newer semantic version creates an unobtrusive
banner; Download opens that release page. There is no auto-download or auto-install.
Network errors are silent at launch and visible only for a manual check.

## Dependency evaluation

- **objc2 + objc2-app-kit:** maintained Rust bindings for the AppKit APIs Blinkview needs;
  permissive MIT/Apache-2.0/Zlib licensing, already present transitively through Tauri,
  no daemon or runtime dependency. Cost: macOS-specific unsafe boundary and feature
  selection. Chosen behind `cfg(target_os = "macos")` with a tiny public API.
- **Tauri updater plugin:** established, permissively licensed, and excellent for signed
  in-app installs. It is unnecessary here because the requirement explicitly stops at
  notifying and opening a Download page; adopting it would add signing manifests and an
  install surface Blinkview does not use. Not chosen.
- **GitHub Releases REST API through existing ureq:** maintained server contract, no new
  networking library, small integration. Cost: unauthenticated rate limits and GitHub
  contact on launch. Chosen with a short timeout and manual retry.
- **CrabNebula `drag` crate:** maintained by an established Tauri vendor, cross-platform,
  permissively licensed, and purpose-built for real outbound file drags. The companion
  plugin accepts arbitrary absolute paths from JavaScript, which is broader authority
  than Blinkview needs. Chosen as a Rust-only dependency behind a command that resolves
  content hashes itself.

## Acceptance criteria

1. MOV, MP4, and M4V open with custom controls and no hover veil over the video.
2. A stem-matched photo/MOV pair occupies one tile; its still paints immediately and
   press-and-hold never shows a blank frame.
3. Newest/Oldest persists per folder and the month menu lands on the chosen month.
4. Share accepts only hashes from the active library and opens the macOS share picker
   with the resolved real files.
5. Dragging a selected tile into Finder creates real file references without copying
   photo bytes through JavaScript.
6. A newer GitHub release creates one banner with Download; current, malformed, failed,
   and prerelease responses do not create false update notices.
7. Launch checks never block the library and never transmit library-derived data.
8. All new controls work by keyboard and respect reduced motion.

## Tasks

- [x] Add Live Photo pairing and press/hold interaction.
- [x] Replace native video chrome with the floating transport pill.
- [x] Add timeline direction and month controls.
- [x] Add the narrow AppKit share/outbound-drag adapter.
- [x] Add GitHub release check, banner, Download, and manual command.
- [x] Add tests and complete two rendered screenshot critique passes.
