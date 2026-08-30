# ADR-0014: ffmpeg ships with the app

Date: 2026-08-30
Status: Accepted

## Context

Video poster frames come from ffmpeg, invoked as `Command::new("ffmpeg")` and
documented as "optional by design: a missing one degrades to a video with no poster
frame rather than failing the whole thumbnail pass".

It does not degrade that way. An app launched from Finder inherits launchd's
`/usr/bin:/bin:/usr/sbin:/sbin`, not a shell's, so `/opt/homebrew/bin/ffmpeg` is
invisible to the installed `.app` while working perfectly in a terminal — which is why
development never saw it. Every video thumbnail then failed, and the failure path in
`serve_photo` read the whole original and handed it to an `<img>`. On a 507-video phone
backup that is 15.7 GB of MP4 in the render process, which macOS answers by killing it:
the window goes black, reloads, asks for the same thumbnails, and does it again.

Measured: `tauri://localhost` at 9.59 GB against 618 MB for the Rust process, and 1,925
cached thumbnails for 1,926 photographs with none at all for 507 videos.

Searching known install prefixes fixes the reported case and leaves the shape of the
bug intact — an ffmpeg installed anywhere else is still invisible, and a user with no
ffmpeg still silently gets no video thumbnails. Planned work makes this worse rather
than better: hover previews and a codec fallback both need ffmpeg on every machine, not
on the machines that happen to have it.

## Decision

**Bundle ffmpeg with the application** as a Tauri `externalBin` sidecar, one static
build per shipped target.

`openfoto-core` must not learn about Tauri: it is used by the CLI, which has no bundle.
The desktop app resolves its sidecar at startup and exports `OPENFOTO_FFMPEG`; core
resolves ffmpeg as `OPENFOTO_FFMPEG` → `PATH` → known install prefixes. The CLI is
unchanged and keeps using whatever the system provides.

Rejected: **platform-native frame extraction** (AVFoundation, Media Foundation, ffmpeg
on Linux only) — no size cost on macOS and Windows, but three implementations to write
and maintain for one poster frame, and it does not help the codec fallback.

Rejected: **keep it optional, warn loudly** — free, and honest, but it makes a core
feature conditional on a dependency most users will not have, and the planned player
cannot be built on a maybe.

## Consequences

Each installer grows by roughly 30–40 MB for a static build carrying only the codecs
openfoto needs; the macOS `.dmg` goes from about 16 MB to about 50 MB. Only the matching
platform's binary is bundled, so no installer carries all three.

ffmpeg's GPL builds are GPLv2-or-later, which is compatible with this project's
GPL-3.0-or-later. The build configuration used for each binary is recorded with it, so
the licence position is auditable rather than assumed.

Video features stop being conditional. A poster frame, a hover preview and a transcode
of an unplayable codec can all be relied upon to exist, which is what makes
docs/SPECS/active/2026-08-30-video-previews.md implementable at all.

The cost is a third-party binary in the release artefacts, which must be updated when
ffmpeg publishes a security fix — a maintenance obligation the project did not have
when ffmpeg was the user's problem.
