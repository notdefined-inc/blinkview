# ADR-0018: a RAW is read from its preview, and never written

Date: 2026-08-31
Status: Accepted

## Context

`PHOTO_EXT` listed no camera-RAW extension, so a library of CR3/CR2/NEF/ARW/RAF/DNG
files opened empty. Closing that gap has two possible shapes.

**Develop the RAW.** `rawler` and `rawloader` (LGPL-2.1, compatible with our
GPL-3.0-or-later) and `quickraw` (MIT) demosaic properly. That is seconds per frame
against milliseconds, a dependency to track, and it still would not let us write a RAW
back — so it buys image quality we do not currently render anywhere, at a cost paid on
every thumbnail.

**Read the JPEG the camera already made.** Every one of these containers carries one:
the image on the back of the camera. Measured on CC0 samples from raw.pixls.us, it is
the full frame for three of the six formats.

Finding it cannot be done by scanning for JPEG start markers. `sample.cr2` stores its
sensor data as a 2238x2954 **lossless** JPEG — larger than the real 2496x1664 preview in
one dimension, and portrait where the photograph is landscape. A compressed-lossless DNG
carries about 350 such tiles. A "biggest JPEG wins" rule ships sensor noise as a
thumbnail and looks like a decoder bug.

## Decision

Read RAW from the preview its container **declares**, validate it, and never write back.

* Follow the tags that name a preview — `JPEGInterchangeFormat`, single-strip
  `StripOffsets`, SubIFDs — for the TIFF-based formats; the fixed header for RAF; the
  sample table and `PRVW` box for CR3's ISO-BMFF.
* Accept only SOF0/SOF1/SOF2. **SOF3 is lossless JPEG, which inside a RAW means sensor
  data**, and that single check is what separates a photograph from a noise field.
* `edit::apply` refuses a RAW outright. It ends in `rename(tmp, src)`, so an edit would
  put JPEG bytes in a file still called `.CR3` — the negative destroyed and the name now
  a lie. `geo::write_gps`, `write_datetime` and `metadata::strip_file` were already
  JPEG-only and needed no change.
* `sips` remains the macOS fallback for a file that declares no usable preview
  (ADR-0005). On Linux and Windows such a file is listed without a thumbnail, the same
  bargain video makes without ffmpeg.

CR3 and RAF hide their EXIF where no container reader looks — a `CMT1` box and the
preview's own APP1. `raw::exif_block` reaches both, because without it every Canon
R-series and Fujifilm frame is dated by its file timestamp, and the timeline is ordered
by exactly that.

## Consequences

Good: six formats, on every platform, with no new dependency. Extraction reads 0.9-3.0 MB
of a 12-50 MB file in 1.6-11.3 ms; a full thumbnail takes 27-182 ms, dominated by
decoding the preview rather than finding it.

Costly: **the preview is not always the full sensor image.** On the reference files it is
6000x4000 for CR3, 5760x3840 for DNG and 4928x3264 for NEF, but only 1616x1080 for ARW
and 1920x1280 for RAF — enough for the grid and honest in the lightbox, short of a
5K display. Developing the RAW is the only fix, and this ADR is where that would be
revisited.

Also: RAW is read-only inside Blinkview. Rating, labelling, renaming, moving and deleting
all work, because none of them touches the file's contents. Crop, colour and metadata
writes do not, and say so.
