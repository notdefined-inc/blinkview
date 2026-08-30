# ADR-0015: Stripping metadata keeps the original

Status: Accepted · 2026-08-30

## Context

Removing EXIF before sending a photograph to someone is an ordinary, reasonable thing
to want: it carries the camera, the lens, the exposure, and often the coordinates of
where it was taken.

But blinkview reads a photograph's date from EXIF first (ADR-0003), and that measurement
was decisive: a correct 300-photograph sample showed **100%** carry `DateTimeOriginal`,
disagreeing with the camera filename in 13% of cases. `taken_at` is what the grid sorts
by, what the date headings group by, and what every date query in the search language
resolves against.

So stripping is not like editing. An edit changes what a photograph looks like, and the
user is looking at it while they decide. Stripping changes something invisible, and the
consequence — this photograph moving to a different day, or losing its place in the
timeline entirely — shows up later, somewhere else, in a library the user is no longer
looking at.

The index keeps `taken_at` for the *old* content hash, and stripping changes the bytes,
so the stripped file is a new photograph to the index. It falls back to the filename,
then to mtime. A phone backup named `20260820_120132.jpg` survives that; a file named
`DSC_0001.jpg` does not.

## Decision

**Stripping keeps the original in `Originals/` by default**, exactly as editing does,
and the confirmation says why: the date blinkview sorts by comes from the metadata being
removed.

Two supporting choices:

- **Only what identifies the photographer goes.** EXIF and XMP (APP1), IPTC (APP13),
  maker notes in the other APPn slots, and free-text comments (COM). JFIF (APP0), ICC
  colour profiles (APP2) and Adobe's colour-transform marker (APP14) stay: they decide
  how the photograph *looks*, and "strip metadata" does not mean "change the colours".
  For PNG, `tEXt`/`zTXt`/`iTXt`/`eXIf`/`tIME` go and `iCCP`/`gAMA` stay.
- **Never a re-encode.** The entropy-coded scan data is copied byte for byte, so the
  decoded pixels are bit-identical. Re-encoding to drop data that is not in the pixels
  would lose quality to remove information that is not there.

Formats that cannot be rewritten this way — HEIC, video — are refused by name rather
than attempted.

## Consequences

- A stripped photograph may sort by filename or mtime instead of capture time. The
  original in `Originals/` still carries the real date, so the information is not gone
  from the library, only from that copy.
- `Originals/` grows. It is visible in Finder and the user can empty it, which is the
  same bargain editing already makes (ADR-0006).
- Ratings and labels survive, but not for free. They live in `blinkview.json` keyed by
  **content hash** (ADR-0007), and stripping changes the bytes and therefore the hash,
  so the rewrite carries them across explicitly (`UserDataSet::rekey`). Writing this
  ADR is what surfaced that: the first implementation lost a five-star rating on every
  strip, and so — it turned out — had *editing*, ever since editing existed. Anything
  that rewrites a photograph in place has to do this.
- Anyone who wants the original gone can uncheck it, and then it is gone. A user who
  has decided is entitled to decide; it is simply not the default.
