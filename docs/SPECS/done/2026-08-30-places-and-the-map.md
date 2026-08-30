# Places, and the map
Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem
A photo library knows where every photograph was taken and blinkview never says so.
EXIF GPS is read only as a display string in the info panel; there is no map, no way to
browse by place, and nothing resolves coordinates into a name a person recognises. And
the photographs that need it most — scans, screenshots, anything through a messaging
app — have no coordinates at all, with no way to supply them.

## Non-goals
- **No map tiles, ever.** Fetching tiles would tell a tile server where the user has
  been, on every pan, which is the one thing a local-first photo app must not do. The
  basemap is bundled vector outlines: land and borders, not streets.
- No routes, no heatmap, no clustering by time. Pins and place names.
- Writing GPS is JPEG-only. HEIC and video are refused by name, as with stripping.
- No online geocoding fallback for a place the bundled database does not know.

## Design
**Coordinates are cached in the index**, not read per map open. A new `gps(hash, lat,
lon)` table keyed by content hash, like `signatures` and `clip`, so it survives renames
and moves. A row with NULL coordinates means *checked, has none* — without that, every
map open would re-read every photograph that will never have GPS. Filled by a `locate`
pass that reports progress and skips what it has already seen; opening the map runs it.

**Place names come from a bundled table.** `crates/blinkview-core/data/places.bin` —
170,860 places from GeoNames cities1000, packed to 3.8 MB (interned region and country
names, coordinates as integer degrees × 10⁴). Reverse lookup is nearest-by-haversine
over a 1°-cell grid index, so a lookup touches a few dozen candidates rather than
170,000. Forward search matches names for "type a city". Both directions use one table,
so what the map calls a place is what the search box finds.

Rejected: `reverse_geocoder` and friends as dependencies — the lookup is a grid and a
distance function, and the datasets are the real substance. Rejected: shipping the raw
8.3 MB TSV.

**The map is drawn, not fetched.** Natural Earth outlines bundled at two levels —
`world110.json` (149 KB) for the world view, `world50.json` (1.4 MB) once zoomed past a
threshold — projected Web Mercator onto a canvas in the Aurora Glass palette. Pins are
clustered by screen distance so a city reads as one pin with a count, and clicking one
filters the grid to those photographs. Nothing loads over the network, so panning has
nothing to wait for.

**Writing GPS rebuilds the EXIF rather than appending to it.** A second APP1 segment
would be ambiguous, and replacing the existing one would throw away the camera and the
date. So the TIFF structure is parsed into its entries, the GPS IFD is inserted or
replaced, and the whole thing is re-serialised with recomputed offsets.

That is the risky part of this spec, so it is not trusted: the rewritten file is
**read back and re-parsed before it replaces the original**, and if the re-read does
not find the coordinates just written, the original is left exactly as it was. The
content hash changes, so ratings are carried across (ADR-0015).

## Acceptance criteria
1. A photograph with EXIF GPS resolves to "City, Region, Country" with no network.
2. A coordinate in open ocean, or far from any settlement, reports the country or
   nothing rather than naming an implausibly distant city.
3. The `locate` pass is incremental: a second run over the same library does no work.
4. The map draws every located photograph, clustered, with no network request.
5. Clicking a cluster filters the grid to exactly those photographs.
6. The map opens on a library with no located photographs without erroring.
7. Typing a city name offers matching places, largest first.
8. Choosing one writes GPS into the selected JPEGs, and re-reading the file finds the
   coordinates — verified with an independent decoder.
9. Writing GPS preserves the existing EXIF: camera, lens and `DateTimeOriginal` are
   still readable afterwards.
10. A write that cannot be read back leaves the original untouched.
11. Ratings and labels survive the write, though the content hash changes.
12. HEIC and video are refused by name.

## Tasks
- [x] 1. `tools/build-geodata.{sh,py}` and the bundled data (touches: tools/, data/)
- [x] 2. `geo` module: packed table, grid index, `nearest`, `search` + tests
- [x] 3. `gps` table, `read_gps`, the `locate` pass (touches: index.rs, geo.rs, scan.rs)
- [x] 4. EXIF GPS writer with read-back verification + tests (touches: geo.rs)
- [x] 5. Commands: `locate_photos`, `photo_places`, `place_search`, `set_photo_location`
- [x] 6. Map view: canvas, projection, pan/zoom, clustering, click-to-filter
- [x] 7. "Where was this?" for photographs with no coordinates

## Verification notes
Driven in the running app. The reference libraries turned out to carry **no GPS at
all** — 0 of 120 sampled photographs — which is itself the argument for the second half
of this spec, and meant the map was verified through the write path.

- Reverse lookup, against the bundled table with no network: the Acropolis resolves to
  Athens, Greece; Times Square to the United States; Point Nemo — the furthest point on
  earth from land — to nothing at all rather than an implausible city; Suva resolves
  across the antimeridian, which the grid has to wrap for.
- Writing into a **real 4.2 MB Galaxy S25 Ultra JPEG**: coordinates read back by an
  independent decoder, `Make`/`Model` and `DateTimeOriginal` still present afterwards,
  pixels unchanged at 4000×1848, and the file grew by 126 bytes.
- Three groups of photographs placed in Firá, Kyoto and Reykjavík reported
  *"40 placed in Firavitoba, Boyacá, Colombia"*, *"30 placed in Kyoto, Japan"*,
  *"12 placed in Reykjavík, Capital Region, Iceland"* — and the map then drew three
  clusters of 39, 30 and 12 in the right parts of the world.
- The `locate` pass is incremental: the second run reported `checked: 0` in **3 ms**.
- Hovering each cluster named it in the HUD; clicking Kyoto closed the map and left the
  grid holding exactly its 30 photographs.
- Both themes render (screenshotted): white land on pale blue in light, and a dark
  palette that had to be corrected — the first attempt drew `#181826` land on `#0a0a12`
  sea, which read as two dark greys rather than a map.

**A defect this found.** Searching "Fira" returned *Firavitoba, Colombia* rather than
Firá on Santorini, because GeoNames spells it with an accent and nobody types accents.
The ASCII spelling is now carried in the table (`OFGEO2`) and searched alongside the
display name, so "reykjavik" finds "Reykjavík". Cost: 0.5 MB.

Two limits worth stating. The basemap has no streets, so zooming far in shows coastline
and pins rather than a neighbourhood — the price of never fetching a tile. And the
place table names the nearest settlement within 120 km, so a photograph taken well
outside one reports nothing rather than guessing.
