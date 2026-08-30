# Capture date and time correction
Status: Shipped 2026-08-30 · Owner: notdefined · 2026-08-30

## Problem

Scans, screenshots, and cameras with the wrong clock can have absent or incorrect
capture times. OpenFoto can already write corrected GPS coordinates directly into JPEG
EXIF and re-key all hash-addressed metadata, but there is no equivalent date command.
Changing only the index would violate the vault invariant: a rescan would forget it.

## Non-goals

- No date stored only in SQLite or `.openfoto/`.
- No guessing a timezone or offsetting a whole camera clock in this first version.
- No silent fallback to filesystem mtime.
- No rewrite of unsupported containers. They are reported and left unchanged.

## Design

`Set Date & Time…` appears in the photo context menu and selection actions. It accepts
one local wall-clock value and applies it to all selected files. For JPEG, the existing
TIFF/EXIF writer is generalized to set `DateTimeOriginal`, `DateTimeDigitized`, and the
IFD0 `DateTime` value in `YYYY:MM:DD HH:MM:SS` form. Existing EXIF, orientation, and GPS
entries are preserved.

Every file is written atomically, decoded again, and verified before OpenFoto accepts
the operation. Its new BLAKE3 hash replaces the old identity in user metadata and the
source is rescanned. Unsupported or malformed files remain byte-for-byte unchanged and
are included in the result summary.

The picker starts at the current capture time for a single photo and otherwise at the
first selected photo. A clear note says that multi-select stamps the same instant on
every item.

## Acceptance criteria

1. A JPEG with no EXIF gains all three date tags and rescans to the chosen time.
2. A JPEG with EXIF changes only the three date values and preserves GPS/orientation.
3. Multi-select applies one exact value to every supported item.
4. User rating, label, and other hash-keyed metadata survive the content-hash change.
5. A failed write leaves the source file and its user metadata unchanged.
6. Unsupported formats are named in a non-destructive result summary.

## Tasks

- [x] Generalize the TIFF rewriter and add date round-trip tests.
- [x] Add the multi-file desktop command with validation and re-keying.
- [x] Add the context/selection picker and result summary.
- [x] Verify the visible flow against a JPEG fixture and sync current docs.

