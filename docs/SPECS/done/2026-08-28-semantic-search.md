# Semantic search
Status: Agreed   Owner: notdefined   Date: 2026-08-28

## Problem

Photographs can be found by person, date, folder, rating and filename, but not by what
is in them. "The one with the dog" is how people actually search, and it is the single
most common thing this app cannot do. Face embeddings cannot answer it (ADR-0008).

## Non-goals

- No cloud, no API calls. The model runs locally or the feature does not exist.
- Not a replacement for people search — faces stay on the face pipeline, which is far
  more accurate for identity than CLIP would be.
- No OCR. Text inside images is out of scope; MobileCLIP is unreliable at it.
- No ANN index. A dot product over every photo is correct to six figures; beyond that is
  a later problem.
- No auto-tagging or generated captions. Search only.

## Design

`blinkview-core::semantic`, mirroring the shape of `faces`:

```
fetch          two more entries in faces::fetch::specs(), SHA-pinned
embed_image    resize 256 / centre crop / scale 0..1 / NCHW  -> 512-d, L2-normalised
embed_text     CLIP BPE via tokenizers, pad to 77            -> 512-d, L2-normalised
analyze        one pass over photos missing an embedding, cached by content hash
search(q, n)   text embed, dot product against all, rank, cut below the threshold
```

Storage: a `clip` table in `index.sqlite`, `hash TEXT PRIMARY KEY, embedding BLOB` —
derived, so it lives in the disposable vault (ADR-0001, ADR-0007).

UI: an unrecognised word in the search box becomes a semantic term rather than a
filename match, shown as a distinct chip. Semantic results combine with every existing
filter, so "dog august 2026 sam" narrows by meaning, date and person together.

Threshold: **0.18**, from the measurements in ADR-0008 — every correct query scored
above 0.20 and both failures below 0.18. Configurable via a flag.

## Acceptance criteria

1. `blinkview models fetch` installs both encoders, SHA-verified, and `models status`
   reports them.
2. Text and image encoders both produce 512-d vectors with unit length to 1e-4.
3. A text embedding computed in Rust matches the Python reference for the same string
   to cosine >= 0.999 — the ADR-0004 parity rule.
4. `semantic analyze` embeds every photo once; a second run does no work.
5. Deleting `.blinkview/` and rescanning reproduces identical embeddings.
6. On the reference library, "a night sky" returns night photographs in its top 4 and
   scores above 0.20.
7. Queries below the threshold return nothing rather than the least-bad photo.
8. A semantic term combines with person, date and rating filters in one query.
9. With models absent, search behaves exactly as it does today and never errors.
10. Embedding is interruptible and resumable: killing the process mid-analysis loses
    only the photo in flight.

## Tasks

- [x] 1. `semantic` module: model loading, image and text embedding (core)
- [x] 2. `clip` table, cache read/write keyed by hash (core)
- [x] 3. Model specs + SHA pins in `faces::fetch` (core) — criterion 1
- [x] 4. `analyze_semantic` with progress, and a CLI command (core, cli) — 4, 5, 10
- [x] 5. Parity test against a committed Python-generated reference (tests) — 2, 3
- [x] 6. `search` ranking with threshold (core) — 6, 7
- [x] 7. Tauri command + query integration + chip (app) — 8, 9

## Outcome

Shipped. All ten acceptance criteria met, verified against the 280-photo demo library
and a 12-photo HEIC library.

One decision changed during implementation. The spec assumed the int8 text encoder from
ADR-0008; the parity test in task 5 failed against it, and the investigation showed int8
was wrong rather than merely imprecise — vectors off by cosine 0.89 from fp32, nearly
double the matches clearing the threshold, and irreproducible across onnxruntime builds.
The text tower is now fp32 and the two runtimes agree bit-for-bit. ADR-0008 carries the
measurements and the correction; models grew from 84 MB to 204 MB.

The threshold stayed at 0.18 but was re-derived on the shipping path: the fp32 gap is
0.168 (highest false positive) to 0.183 (lowest true positive), not the 0.175-0.20 the
original evaluation reported from int8 scores.

Two things surfaced only in the running window, not in the code:

- Colour words were being claimed as label filters, so "a red sailing boat" searched for
  a boat of no particular colour and "green trees" searched for trees. A bare colour is
  now a label only when the query is nothing but colours.
- An in-flight query rendered "No photos match this filter" — a wrong answer shown
  before the right one arrived. There is now a distinct looking-in-progress state, and
  a missing model reports itself rather than being reported as "nothing recognised".
