#!/usr/bin/env bash
# Assert the bundled ffmpeg is what the spec says it is.
#
# Criteria 7 and 8 of docs/SPECS/active/2026-08-30-ffmpeg-sidecar.md, checked against the
# built binary rather than against the configure line it was built from. That distinction
# is not pedantry: `--disable-autodetect` silently dropped zlib once, taking png and
# Matroska's compressed headers with it, and the configure line still looked correct.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$(ls "$ROOT"/apps/desktop/src-tauri/binaries/ffmpeg-* 2>/dev/null | head -1)}"
[[ -x "$BIN" ]] || { echo "no ffmpeg binary found; run tools/build-ffmpeg.sh" >&2; exit 1; }

MAX_MB=40
DECODERS="h264 hevc vp8 vp9 av1 mpeg2video mpeg4 mjpeg prores vc1 theora dvvideo wmv3
          aac mp3 opus vorbis flac ac3 eac3 wmav2 png"
ENCODERS="libx264 aac mjpeg png"
DEMUXERS="mov matroska avi mpegts flv asf ogg"

fail=0
note() { printf '  %-12s %s\n' "$1" "$2"; }

size_mb=$(( $(wc -c < "$BIN") / 1048576 ))
if (( size_mb < MAX_MB )); then note "size" "${size_mb} MB (budget ${MAX_MB})"
else note "SIZE" "${size_mb} MB exceeds ${MAX_MB} MB"; fail=1; fi

for c in $DECODERS; do
  "$BIN" -hide_banner -decoders 2>/dev/null | grep -qE "^ *[A-Z.]+ +$c\b" \
    || { note "MISSING" "decoder $c"; fail=1; }
done
for c in $ENCODERS; do
  "$BIN" -hide_banner -encoders 2>/dev/null | grep -qE "^ *[A-Z.]+ +$c\b" \
    || { note "MISSING" "encoder $c"; fail=1; }
done
for d in $DEMUXERS; do
  "$BIN" -hide_banner -demuxers 2>/dev/null | grep -qE "^ *[A-Z ]+ *$d\b" \
    || { note "MISSING" "demuxer $d"; fail=1; }
done

if (( fail )); then echo "bundled ffmpeg does not meet the spec" >&2; exit 1; fi
echo "  ok           $(echo "$DECODERS" | wc -w | tr -d ' ') decoders, $(echo "$ENCODERS" | wc -w | tr -d ' ') encoders, $(echo "$DEMUXERS" | wc -w | tr -d ' ') demuxers"
"$BIN" -version | head -1
