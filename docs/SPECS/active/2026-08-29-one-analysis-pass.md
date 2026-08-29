# One analysis pass

Status: active · 2026-08-29
Decision: ADR-0013 (one pass over the pixels)

## Intent

Make analysing a large library take about as long as decoding it once, because that is
the irreducible part. Today a photograph is decoded three times — thumbnail, faces,
embedding — and the decode is 85% of the cost of each.

The measure of success: **complete analysis of 200,000 photographs in about an hour on
eight cores**, against roughly thirteen today, with no new model and no new dependency.

## Scope

Core: a single `analyze` pass sharing one decode per photograph, parallel across
photographs, resumable, with per-stage results committed independently. A vision-only
encoder so the 161 MB text tower is not loaded during analysis.

App: one command driving it, with progress; the existing per-stage commands keep working
by delegating with the other stages switched off.

Out of scope: the `photos` payload at 200k — a source switch is ~3.8 s projected there,
and it is a separate problem with its own measurement.

## Design

```
for each photograph needing anything (parallel across photographs):
    if only the thumbnail is missing and the camera embedded a preview:
        thumbnail from the preview          ~3 ms, no full decode
    else:
        decode once, unrotated              ~60-85 ms
        thumbnail  = shrink 512  -> rotate -> encode
        faces      = shrink 1280 -> rotate -> detect -> embed each face
        embedding  = shrink 256  -> rotate -> CLIP vision
    commit each result separately
```

Rotation happens after each shrink, never on the full frame: rotating twelve megapixels
costs 14 ms and rotating the result costs 0.2 ms.

Worker count is `min(cores, 4)` by default. ONNX Runtime already threads one inference,
so more workers mainly multiply memory — each holds a detector, a face embedder and a
vision encoder.

## Acceptance criteria

1. A photograph with thumbnail, faces and embedding all cached is skipped without being
   opened — provable by a decode counter, not by timing.
2. A photograph missing only its thumbnail, where the camera embedded a usable preview,
   is not fully decoded.
3. A photograph missing two or more stages is decoded exactly once.
4. Faces found by the combined pass match those the old face pass found on the same
   photographs — the same count and the same boxes within a pixel.
5. Embeddings match the old pass to cosine >= 0.9999, so the ADR-0008 threshold still
   means what it did.
6. Thumbnails match the current output, which is already checked against a
   decode-everything reference by `examples/thumbcheck`.
7. A stage that fails does not prevent the others: a photograph whose detection errors
   still gets its thumbnail and its embedding, and the error is reported.
8. Interrupting the pass and running it again completes the library, redoing at most the
   photograph that was in flight.
9. Analysis never loads the CLIP text encoder.
10. Measured end to end on the reference backup, the combined pass beats the sum of the
    three separate passes by at least 1.6x.

## Tasks

- [ ] 1. `ImageEncoder`: vision-only, so analysis skips the text tower (core) — 9
- [ ] 2. `analyze::run`: one decode, three stages, per-stage commit (core) — 1, 2, 3, 7
- [ ] 3. Parallelise across photographs, models per worker (core) — 10
- [ ] 4. Equivalence tests against the existing passes (tests) — 4, 5, 6
- [ ] 5. Resumability test: interrupt and finish (tests) — 8
- [ ] 6. One app command with progress; old commands delegate (app)
- [ ] 7. Re-measure with `examples/bench` and `examples/throughput`, update STATUS.md — 10

## Risks

**Silent drift in face or embedding results.** The whole point of ADR-0003 and ADR-0008
is that thresholds were measured against particular outputs. Criteria 4 and 5 compare
against the existing passes directly rather than trusting that a refactor is faithful.

**Memory.** Each worker holds ~85 MB of models plus a decoded frame of up to 36 MB.
Four workers is ~500 MB; the worker count is capped rather than taken from the core
count.
