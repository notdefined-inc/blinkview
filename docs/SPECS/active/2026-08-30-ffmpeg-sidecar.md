# Bundle ffmpeg as a sidecar
Status: Draft   Owner: notdefined   Date: 2026-08-30

## Problem

The installed app cannot find ffmpeg. A Finder-launched `.app` inherits launchd's
`/usr/bin:/bin:/usr/sbin:/sbin`, so Homebrew's ffmpeg is invisible to it while working
in a terminal — development never saw the failure. Every video thumbnail failed, and
until commit `b2c44e1` the failure served the whole clip to the webview: 15.7 GB on a
507-video backup, killing the render process repeatedly. See ADR-0014.

## Non-goals

- No transcoding, hover previews, or player changes — those are
  `2026-08-30-video-previews.md`, which depends on this and must not start before it.
- No change to the CLI's behaviour: it keeps using whatever ffmpeg the system provides.
- No ffmpeg build pipeline in CI. Binaries are fetched from a pinned upstream release
  and checksummed, the same contract the ONNX models already use.
- Not a full ffmpeg. Only the codecs openfoto needs (see criterion 7).
- `openfoto-core` does not learn about Tauri.

## Design

`bundle.externalBin: ["binaries/ffmpeg"]` in `tauri.conf.json`. Tauri resolves
`binaries/ffmpeg-<target-triple>` at build time and places it beside the executable,
so each installer carries only its own platform's binary.

Contract between the app and core, which is the whole of the coupling:

| | |
|---|---|
| `OPENFOTO_FFMPEG` | absolute path to an ffmpeg binary. Set by the desktop app at startup from the resolved sidecar path. Unset elsewhere. |
| `thumbs::ffmpeg_bin() -> Option<OsString>` | resolves `OPENFOTO_FFMPEG` → `PATH` → `FFMPEG_FALLBACKS`, returning the first that answers `-version` with success. |

Binaries live in `apps/desktop/src-tauri/binaries/`, are git-ignored, and are fetched by
`tools/fetch-ffmpeg.sh` against `tools/ffmpeg.lock` (URL + SHA-256 + licence + build
flags per triple). CI runs that script before `tauri build`.

Rejected: `tauri-plugin-shell`'s sidecar API — it adds a plugin and a capability
permission to run a subprocess the Rust side already spawns directly.

Rejected: committing the binaries — three static builds is ~100 MB of history for
something reproducible from a pinned URL.

## Acceptance criteria

1. With `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and no ffmpeg installed, the packaged app
   produces a poster frame for an MP4.
2. `ffmpeg_bin()` prefers `OPENFOTO_FFMPEG` over a different ffmpeg on `PATH`.
3. `ffmpeg_bin()` returns `None` when no candidate answers `-version` successfully, and
   `render_video` then fails without spawning anything.
4. A binary whose SHA-256 does not match `tools/ffmpeg.lock` fails the fetch script with
   a non-zero exit and is not written to `binaries/`.
5. The CLI with no `OPENFOTO_FFMPEG` set behaves exactly as it does today.
6. Each installer contains exactly one ffmpeg, for its own target triple.
7. The bundled binary decodes H.264, HEVC, VP9 and AV1, and encodes H.264 and AAC.
   Verified by `-codecs` in a test, not by assertion.
8. The macOS `.dmg` stays under 60 MB.
9. `codesign --verify --deep --strict` passes on the bundled `.app` — a sidecar is a
   Mach-O inside the bundle and is signed with it (ADR-0014, and the v0.1.0 signing bug).

## Tasks

- [ ] 1. `tools/ffmpeg.lock` + `tools/fetch-ffmpeg.sh`, with checksum verification and
      recorded build flags and licence per triple (touches: tools/)
- [ ] 2. `ffmpeg_bin()` honours `OPENFOTO_FFMPEG` first; unit tests for criteria 2 and 3
      (touches: crates/openfoto-core/src/thumbs.rs)
- [ ] 3. `externalBin` config; app exports `OPENFOTO_FFMPEG` at startup from the resolved
      sidecar path (touches: apps/desktop/src-tauri/)
- [ ] 4. Release workflow runs the fetch script before `tauri build`; verify criteria 6,
      8 and 9 on the produced artefacts (touches: .github/workflows/release.yml)
- [ ] 5. Doc sync: ADR-0014 to Accepted, STATUS.md drops the ffmpeg known issue, README
      stops telling users to install ffmpeg (touches: docs/, README.md)
