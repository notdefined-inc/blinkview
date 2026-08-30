# Bundle ffmpeg as a sidecar
Status: Agreed   Owner: notdefined   Date: 2026-08-30

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

Binaries live in `apps/desktop/src-tauri/binaries/`, are git-ignored, and are produced
by `tools/build-ffmpeg.sh` against `tools/ffmpeg.lock` (source tarball URL + SHA-256 +
configure flags per triple). CI runs it before `tauri build`, cached on the lock hash.

**Prebuilt binaries were investigated first and rejected on evidence (2026-08-30):**

| source | covers | verdict |
|---|---|---|
| BtbN/FFmpeg-Builds | Windows, Linux (no macOS at all) | verifiable — GitHub publishes a per-asset sha256 digest — but the linux64 GPL archive alone is 121 MB |
| evermeet.cx | macOS **x86_64 only** | wrong architecture for the only Mac target we ship |
| osxexperts.net | macOS arm64 | **integrity claims do not hold.** The page lists two different SHA-256 values for `ffmpeg9arm.zip`, `591260c9…` and `df3f1e3f…`; the served file is `d0c06c5c…` on two independent downloads. A binary whose publisher's own checksum is wrong cannot be pinned, and must not be signed into our bundle. |

That leaves no verifiable prebuilt arm64 macOS binary, and the full builds that do exist
are far over budget — osxexperts' arm64 static ffmpeg is 49.7 MB on its own, against a
16 MB installer today. Building from source fixes both at once: integrity because we
build it, and size because the configure flags carry only the codecs criterion 7 names.

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
7. The bundled binary handles the formats a phone, a camera and a download actually
   produce. Verified by asserting each name appears in `-demuxers`/`-decoders`/
   `-encoders`, not by assertion in prose:
   - **containers**: mov/mp4/m4v, matroska/webm, avi, mpegts, flv, 3gp, asf, mpeg-ps, ogg
   - **video decode**: h264, hevc, vp8, vp9, av1, mpeg2video, mpeg4, mjpeg, prores, vc1,
     theora, dvvideo, wmv1/2/3
   - **audio decode**: aac, mp3, opus, vorbis, flac, ac3, eac3, pcm_*, wmav2
   - **encode**: libx264 and aac (for previews and the playback transcode of
     `2026-08-30-video-previews.md`)
   Only libx264 is an external library; every decoder above is native to ffmpeg, so the
   build has exactly one dependency beyond ffmpeg itself.
8. The bundled binary is under 40 MB per platform, and the macOS `.dmg` under 70 MB.
   Broad format coverage and a small binary pull against each other; these numbers are
   provisional and get re-set to the measured size plus headroom once task 1 has built
   once. A build that misses them is reported, not quietly accepted.
9. `codesign --verify --deep --strict` passes on the bundled `.app` — a sidecar is a
   Mach-O inside the bundle and is signed with it (ADR-0014, and the v0.1.0 signing bug).

## Tasks

- [x] 2. `ffmpeg_bin()` honours `OPENFOTO_FFMPEG` first; unit tests for criteria 2 and 3
      (touches: crates/openfoto-core/src/thumbs.rs) — done, three tests
- [ ] 1. `tools/ffmpeg.lock` + `tools/build-ffmpeg.sh`: pinned sources, checksums, the
      configure flags of criterion 7, CI cache keyed on the lock (touches: tools/)
- [ ] 3. `externalBin` config; app exports `OPENFOTO_FFMPEG` at startup from the resolved
      sidecar path (touches: apps/desktop/src-tauri/)
- [ ] 4. Release workflow runs the fetch script before `tauri build`; verify criteria 6,
      8 and 9 on the produced artefacts (touches: .github/workflows/release.yml)
- [ ] 5. Doc sync: ADR-0014 to Accepted, STATUS.md drops the ffmpeg known issue, README
      stops telling users to install ffmpeg (touches: docs/, README.md)
