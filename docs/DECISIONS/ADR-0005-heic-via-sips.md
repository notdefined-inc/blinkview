# ADR-0005: HEIC through macOS, not a bundled decoder

Date: 2026-08-28
Status: Accepted

## Context

iPhones shoot HEIC by default, so a photo tool that cannot read it is missing most of a
modern camera roll. The `image` crate does not decode HEIC, and WKWebView will not
display it either — verified directly rather than assumed: an `<img>` pointed at a HEIC
file reports `naturalWidth: 0`.

Three ways to get it:

- **libheif** via bindings. Cross-platform and proper, but adds a system library the
  user must install, and HEIC is patent-encumbered enough that distributing a decoder
  carries its own questions.
- **A pure-Rust decoder.** None is mature enough to stake a photo library on.
- **`sips`**, which ships with macOS and both reads and writes HEIC.

## Decision

Transcode with `sips`, and cache the result.

Thumbnails convert to a temporary file, decode, and discard it — the thumbnail cache is
already the durable artefact. Full-size viewing writes a JPEG to
`.blinkview/derived/<hash>.jpg`, produced on first view rather than by a pre-pass, so
opening a HEIC library costs nothing until a photo is actually opened.

**`sips` does not bake orientation into the pixels — it carries the EXIF tag across.**
Measured: a 4032x3024 HEIC tagged orientation 6 becomes a 4032x3024 JPEG still tagged 6.
Browsers honour the tag, so a full-size view looks correct while anything decoding the
pixels directly gets a sideways image. `load_rgb` therefore reads the tag off the
converted file and applies it. An earlier version of this ADR asserted the opposite and
shipped rotated thumbnails.

## Consequences

Good: iPhone libraries work with no install step, no bundled decoder, and no patent
surface of our own. The conversion is a process spawn, but it happens once per image
and both caches live in the disposable vault (ADR-0001), so deleting `.blinkview/`
still costs nothing but recomputation.

Costly: **this is the first macOS-only dependency in the project.** Everything else is
portable Rust. A Linux or Windows port will need a real decoder here, and this ADR is
where that decision should be revisited. `imageio::needs_conversion` is the single
place that decides, so the seam is narrow.

Also: `dimensions()` has no cheap header path for HEIC and falls back to a full decode,
which makes it markedly more expensive for these files than for JPEG.
