#!/usr/bin/env bash
# Rebuild the bundled offline place database and world outlines.
#
# blinkview resolves coordinates to a place name without a network, so both datasets
# ship inside the binary. This script is the only way they are produced — run it to
# refresh them, and commit what it writes.
#
#   GeoNames cities1000  CC BY 4.0   https://www.geonames.org/  (attribution required,
#                                    shown in the app's map view)
#   Natural Earth        public domain (CC0)  https://www.naturalearthdata.com/
#
# Output:
#   crates/blinkview-core/data/places.bin   ~3.8 MB, 170k places, packed (see geo.rs)
#   apps/desktop/dist/world110.json        ~150 KB, coarse outlines for the world view
#   apps/desktop/dist/world50.json         ~1.4 MB, finer outlines once zoomed in
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "==> downloading"
curl -fsSL -o "$work/cities1000.zip" https://download.geonames.org/export/dump/cities1000.zip
curl -fsSL -o "$work/admin1.txt"     https://download.geonames.org/export/dump/admin1CodesASCII.txt
curl -fsSL -o "$work/countryInfo.txt" https://download.geonames.org/export/dump/countryInfo.txt
curl -fsSL -o "$work/ne110.geojson" \
  https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_110m_admin_0_countries.geojson
curl -fsSL -o "$work/ne50.geojson" \
  https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_0_countries.geojson
unzip -oq "$work/cities1000.zip" -d "$work"

echo "==> packing"
WORK="$work" ROOT="$root" python3 "$root/tools/build-geodata.py"

echo "==> done"
ls -l "$root/crates/blinkview-core/data/places.bin" \
      "$root/apps/desktop/dist/world110.json" \
      "$root/apps/desktop/dist/world50.json"
