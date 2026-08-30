#!/usr/bin/env bash
# Build the ffmpeg the desktop app bundles as a sidecar (ADR-0014).
#
# Builds for the host only. Cross-compiling ffmpeg is a fight not worth having when the
# release workflow already runs one job per platform, so each job builds its own.
#
# Output: apps/desktop/src-tauri/binaries/ffmpeg-<target-triple>[.exe], which is the
# name Tauri's externalBin expects.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/tools/ffmpeg.lock"
OUT_DIR="$ROOT/apps/desktop/src-tauri/binaries"
WORK="${FFMPEG_BUILD_DIR:-$ROOT/target/ffmpeg-build}"

# shellcheck disable=SC1090
set -a; . "$LOCK"; set +a

# `TARGET_TRIPLE` because MSYS2 on the Windows runner starts with a minimal PATH and
# cannot see rustc. Falling back to rustc keeps the script pleasant to run by hand.
TRIPLE="${TARGET_TRIPLE:-$(rustc -vV | awk '/^host:/ {print $2}')}"
EXE=""
if [[ "$TRIPLE" == *windows* ]]; then EXE=".exe"; fi
TARGET="$OUT_DIR/ffmpeg-$TRIPLE$EXE"

if [[ -x "$TARGET" && "${FORCE:-0}" != "1" ]]; then
  echo "ffmpeg already built: $TARGET"
  "$TARGET" -version | head -1
  exit 0
fi

# Say what is missing in one line. Without this, a missing pkg-config surfaces as
# ffmpeg's configure reporting "x264 not found using pkg-config" after it has silently
# substituted `false` for the tool it could not find, which reads like an x264 problem
# and is not one.
preflight() {
  local missing=()
  for tool in curl tar make cc; do
    command -v "$tool" >/dev/null || missing+=("$tool")
  done
  # ffmpeg locates libx264 only through pkg-config; there is no --with-x264 escape.
  command -v pkg-config >/dev/null || command -v pkgconf >/dev/null || missing+=("pkg-config")
  # x264's x86 assembly needs an assembler. aarch64 uses the system one and does not.
  case "$(uname -m)" in
    x86_64|amd64) command -v nasm >/dev/null || missing+=("nasm") ;;
  esac
  if (( ${#missing[@]} )); then
    echo "missing build prerequisites: ${missing[*]}" >&2
    echo >&2
    echo "  macOS   brew install ${missing[*]}" >&2
    echo "  Debian  sudo apt-get install -y ${missing[*]}" >&2
    echo "  Windows pacman -S \${MINGW_PACKAGE_PREFIX}-{${missing[*]}} under MSYS2" >&2
    exit 1
  fi
}
preflight

sha256() { if command -v shasum >/dev/null; then shasum -a 256 "$1" | cut -d' ' -f1
           else sha256sum "$1" | cut -d' ' -f1; fi; }

# A source whose checksum does not match is never unpacked, let alone built and signed
# into the bundle. This is the whole reason the sources are pinned.
fetch() {
  local url="$1" want="$2" dest="$3"
  [[ -f "$dest" ]] || curl -fsSL --retry 3 -o "$dest" "$url"
  local got; got="$(sha256 "$dest")"
  if [[ "$got" != "$want" ]]; then
    echo "checksum mismatch for $url" >&2
    echo "  expected $want" >&2
    echo "  actual   $got" >&2
    rm -f "$dest"
    exit 1
  fi
  echo "verified $(basename "$dest")"
}

mkdir -p "$WORK" "$OUT_DIR"
cd "$WORK"

fetch "$FFMPEG_URL" "$FFMPEG_SHA256" "ffmpeg-$FFMPEG_VERSION.tar.xz"
fetch "$X264_URL"   "$X264_SHA256"   "x264-$X264_COMMIT.tar.bz2"

# Upstream also signs the release. Checked when gpg and the key are available; absence
# of gpg is not fatal because the pinned checksum is the primary guarantee.
if command -v gpg >/dev/null && [[ -n "${FFMPEG_VERIFY_SIG:-}" ]]; then
  curl -fsSL -o ffmpeg.asc "$FFMPEG_SIG"
  gpg --verify ffmpeg.asc "ffmpeg-$FFMPEG_VERSION.tar.xz" || { echo "signature check failed" >&2; exit 1; }
fi

PREFIX="$WORK/prefix"
mkdir -p "$PREFIX"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
JOBS="$( (command -v nproc >/dev/null && nproc) || sysctl -n hw.ncpu || echo 4)"

if [[ ! -f "$PREFIX/lib/libx264.a" ]]; then
  rm -rf x264-src && mkdir x264-src
  tar xf "x264-$X264_COMMIT.tar.bz2" -C x264-src --strip-components=1
  ( cd x264-src && ./configure --prefix="$PREFIX" --enable-static --enable-pic \
      --disable-cli --disable-opencl && make -j"$JOBS" && make install )
fi

rm -rf ffmpeg-src && mkdir ffmpeg-src
tar xf "ffmpeg-$FFMPEG_VERSION.tar.xz" -C ffmpeg-src --strip-components=1
cd ffmpeg-src

# Criterion 7 of the spec, expressed as configure flags. Started from --disable-everything
# so the binary carries what a phone, a camera or a download actually produces and not
# the whole of ffmpeg: no network protocols, no devices, no encoders beyond the two the
# previews need. Every decoder listed is native; libx264 is the only external library.
./configure \
  --prefix="$PREFIX" \
  --pkg-config-flags=--static \
  --extra-cflags="-I$PREFIX/include" \
  --extra-ldflags="-L$PREFIX/lib" \
  --disable-shared --enable-static \
  --enable-gpl --enable-version3 \
  --disable-everything \
  --disable-doc --disable-htmlpages --disable-manpages --disable-podpages --disable-txtpages \
  --disable-network --disable-devices --disable-sdl2 --disable-debug --disable-autodetect \
  --disable-ffplay --enable-ffmpeg --enable-ffprobe \
  --enable-libx264 \
  --enable-zlib \
  --enable-demuxer=mov,matroska,avi,mpegts,mpegps,flv,asf,ogg,image2,mjpeg,h264,hevc,wav,mp3,flac \
  --enable-muxer=mp4,mov,image2,mjpeg,webp \
  --enable-parser=h264,hevc,vp8,vp9,av1,mpeg4video,mpegvideo,mjpeg,aac,ac3,opus,vorbis,flac \
  --enable-decoder=h264,hevc,vp8,vp9,av1,mpeg2video,mpeg4,mjpeg,prores,vc1,theora,dvvideo,wmv1,wmv2,wmv3,msmpeg4v3,png,webp \
  --enable-decoder=aac,aac_latm,mp3,opus,vorbis,flac,ac3,eac3,wmav2,pcm_s16le,pcm_s16be,pcm_u8,pcm_f32le,pcm_alaw,pcm_mulaw \
  --enable-encoder=libx264,aac,mjpeg,png \
  --enable-filter=scale,format,fps,thumbnail,crop,transpose,null,anull,aformat,aresample \
  --enable-protocol=file,pipe \
  --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,extract_extradata

make -j"$JOBS"
cp "ffmpeg$EXE" "$TARGET"
command -v strip >/dev/null && strip "$TARGET" 2>/dev/null || true
chmod +x "$TARGET"

echo
echo "built: $TARGET"
ls -la "$TARGET" | awk '{printf "size: %.1f MB\n", $5/1048576}'
"$TARGET" -version | head -1
