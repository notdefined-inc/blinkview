# RAW photographs

Status: Shipped   Owner: somesh   Date: 2026-08-31

## Problem

`PHOTO_EXT` (`crates/blinkview-core/src/scan.rs:22`) lists no camera-RAW extension, so a
library of CR3/CR2/NEF/ARW/RAF/DNG files is not degraded in Blinkview — it is **empty**.
Anyone who shoots a camera rather than a phone opens the app on nothing. Every other
subsystem (thumbnails, faces, scene search, near-duplicates, the map) is format-agnostic
once pixels exist, so the whole gap is at the front door.

## Non-goals

- **Demosaicing.** We show the JPEG the camera already wrote, not a developed RAW. No
  white balance, no highlight recovery, no curves.
- **Writing to RAW files.** Set Location and Set Date & Time stay JPEG-only; a RAW is
  read and never rewritten. Rename, move, delete and rating are file- and
  sidecar-level and keep working unchanged.
- **Editing RAW pixels.** Crop and colour presets stay unavailable on RAW rather than
  silently editing a preview and calling it the photograph.
- **RAW+JPEG pairing.** A `.CR3` and its `.JPG` sibling remain two photographs. (The
  Live Photo stem-pairing precedent exists, but pairing is its own decision.)
- **Formats beyond the six named.** ORF, RW2, PEF, SRW and the rest are a table entry
  away but are not verified here, so they are not claimed.

## Design

Two tiers, in this order, decided per file:

1. **Structured preview extraction, pure Rust, every platform.** Parse the container,
   follow the tag that *declares* a preview, and validate what it points at.
2. **`sips` fallback, macOS only.** When tier 1 finds nothing usable, reuse the existing
   HEIC machinery (ADR-0005): `needs_conversion` → `convert_to_jpeg` → cached derived
   JPEG. macOS ImageIO reads all six formats (`sips --formats`).

### Why structured parsing and not "find the biggest JPEG"

Measured on the CC0 samples, scanning for SOI markers picks the wrong bytes. `sample.cr2`
(Canon EOS 5D) contains three JPEG-shaped blobs: a 160x120 thumbnail, the 2496x1664
preview, and a **2238x2954** blob that is the lossless-JPEG-compressed sensor data, not a
viewable image. It is the second largest and portrait where the photograph is landscape.
A marker scan ships that as the thumbnail.

### Contract

```rust
// crates/blinkview-core/src/raw.rs
/// The camera-RAW formats indexed from their embedded preview.
pub fn is_raw(path: &Path) -> bool;

/// The largest preview a RAW container declares, read without loading the whole file.
/// `None` when the container has none, or none that survives validation.
pub fn preview(path: &Path) -> Option<Preview>;

pub struct Preview { pub jpeg: Vec<u8>, pub width: u32, pub height: u32 }
```

Validation, all of which must hold or the preview is rejected:
- starts with `FF D8 FF` and a SOF parses,
- long edge >= 512 px (`thumbs::THUMB_LONG`),
- aspect within 2% of the RAW's own `ImageWidth`/`ImageLength` when the container
  declares them — the same rule `imageio::embedded_preview` already applies to JPEG.

### Container routing

| Format | Container | Where the preview is declared |
|---|---|---|
| CR2, NEF, ARW, DNG | TIFF (II or MM) | IFD chain + SubIFDs (0x014A); `JPEGInterchangeFormat` 0x0201/0x0202, or `Compression`=7 with `StripOffsets` 0x0111/0x0117 |
| RAF | Fuji header | big-endian offset+length at bytes 84..92 |
| CR3 | ISO-BMFF | `uuid` box -> `PRVW` |

`geo.rs` also parses TIFF IFDs, and lifting it into a shared module was the original
plan. It was dropped on contact: that parser materialises every entry so the block can be
re-serialised for GPS writing, and works on an in-memory EXIF block. This one seeks
around a 50 MB file reading a few hundred bytes at a time and never writes. Sharing them
would have meant reshaping a verified writer to suit a reader, for about forty lines of
byte-order helpers. `raw.rs` reads directories on its own.

### Rejected alternatives

- **`sips` alone** (add six extensions to `needs_conversion`, ~1 line). RAW would work on
  macOS today and nowhere else, in a project that ships Windows and Linux installers.
- **`rawler` / `rawloader` / `quickraw`.** Real demosaicing, LGPL-2.1 (compatible with
  our GPL-3.0-or-later) or MIT. Rejected for this spec: seconds per frame against
  milliseconds for a preview, a new dependency to track, and it does not close the one
  thing we still cannot do — write RAW back. Revisit if developing RAW becomes a goal.

## Acceptance criteria

1. A folder of CR3, CR2, NEF, ARW, RAF and DNG files scans, and each file appears in the
   grid with a thumbnail that is the photograph — not a 160x120 stub, not the sensor blob.
2. `raw::preview` on `sample.cr2` returns the 2496x1664 preview, never the 2238x2954
   lossless-JPEG sensor data.
3. Extracting a preview from a 25 MB ARW reads less than 4 MB of the file.
4. A truncated or corrupt RAW yields `None` and a skipped file, never a panic.
5. Big-endian containers work: `sample.nef` (MM) yields its 4928x3264 preview.
6. On macOS, a RAW whose preview fails validation still displays, through `sips`.
7. On a non-macOS build with no usable preview, the file is indexed and listed with no
   thumbnail rather than failing the scan (the existing "no ffmpeg" behaviour for video).
8. Opening a RAW in the lightbox shows the preview at its native size; where that is
   smaller than the sensor image the size is stated rather than upscaled silently.
9. Crop and colour presets are unavailable on a RAW selection, with a reason given.
10. Set Location and Set Date & Time refuse a RAW with a message naming the format.
11. Near-duplicate grouping, face detection and scene search run on RAW previews.
12. Deleting, renaming, moving, rating and labelling a RAW behave as for a JPEG.

## Tasks

- [x] 1. ~~Lift the TIFF IFD reader out of `geo.rs`~~ — rejected on contact, see Design
- [x] 2. `raw.rs`: `is_raw`, TIFF-container preview extraction + validation
- [x] 3. RAF and CR3 containers
- [x] 4. Wire into `scan`, `thumbs`, `analyze`, `imageio::camera_preview`, sips fallback
- [x] 5. Refuse pixel edits on RAW at the core, and disable the controls in the window
- [x] 6. Verified against six CC0 samples, one per format
- [x] 7. `raw::exif_block` — unplanned, and the timeline was wrong without it
- [x] 8. Doc sync: ADR-0018, STATUS, ARCHITECTURE, README, landing page

## Measured

Release build, one CC0 sample per format from raw.pixls.us.

| File | Size | Preview found | Read | Extract | Thumbnail |
|---|---|---|---|---|---|
| CR2 (EOS 5D) | 12.8 MB | 2496x1664 | 1.7 MB | 4.8 ms | 52 ms |
| CR3 (EOS R10) | 34.7 MB | **6000x4000** | 3.0 MB | 11.3 ms | 182 ms |
| DNG (5D3) | 23.2 MB | **5760x3840** | 1.2 MB | 1.7 ms | 141 ms |
| NEF (D7000, big-endian) | 13.0 MB | **4928x3264** | 1.2 MB | 7.1 ms | 104 ms |
| ARW (ILCE-6000) | 25.6 MB | 1616x1080 | 1.1 MB | 9.2 ms | 51 ms |
| RAF (X-T2) | 50.6 MB | 1920x1280 | 0.9 MB | 1.6 ms | 27 ms |

Capture dates come from EXIF for all six. The RAF's reads 2017-01-14 17:35:32, which is
exactly what its filename says — the cross-check that the block being parsed is the right
one. Two criteria are unverified for want of hardware: 6 and 7 (the `sips` fallback, and
a non-macOS build) have no sample that fails preview extraction to exercise them.
