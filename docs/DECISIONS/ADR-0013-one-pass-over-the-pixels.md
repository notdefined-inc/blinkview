# ADR-0013: One pass over the pixels

Date: 2026-08-29
Status: Accepted

## Context

Three passes walk the library and each decodes every photograph again: thumbnails,
face detection, semantic embedding. Measured on a 25GB phone backup, mean 7.8 MP:

| pass | per photo | of which inference |
|---|---|---|
| thumbnails | 33 ms | — |
| faces | 98 ms | **15 ms** |
| embedding | 145 ms | **33 ms** |

**The models are not the cost.** Detecting faces spends 85% of its time turning twelve
megapixels into pixels it immediately shrinks to 1280. Embedding does the same to reach
256. The decode is paid three times for one photograph.

Two obvious escapes were measured and rejected:

- **A faster decoder.** `image` already uses zune-jpeg; turbojpeg came in at 60.4 ms
  against 60.1 ms on 12 MP. A scaled decode saves only 22%, because a JPEG's entropy
  coding must be walked in full whatever size comes out.
- **Hardware acceleration.** CoreML was *slower* than CPU on these models — YuNet
  32.6 ms against 18.5 ms, MobileCLIP 42.4 ms against 32.6 ms. They are small enough
  that graph partitioning and host transfers cost more than they return.

Against Immich's published whole-library figures, openfoto sits in the same league:
80,000 assets embedded in 194 minutes today, against their 80 minutes for `ViT-B-32` and
270 minutes for `ViT-B-16-SigLIP-384`. We reach that with a *smaller* model, which is
the tell: the cost is the pipeline around the model, not the model.

## Decision

**Decode each photograph once, and take everything from that decode.**

One pass produces the thumbnail, the face detections and the semantic embedding
together, sharing a single decode and a single rotation. The pass is parallel across
photographs, with one set of models per worker.

Two rules keep it honest:

1. **Only decode when something is actually missing.** A photograph whose thumbnail,
   faces and embedding are all cached is skipped without being opened. A photograph
   needing *only* a thumbnail still uses the camera's embedded preview and never pays a
   full decode.
2. **Each result is committed on its own.** Faces and embeddings are recorded
   independently, so an interrupted pass loses only the photograph in flight and a
   second run resumes rather than restarting.

Work is parallelised across photographs at a modest width rather than one worker per
core. ONNX Runtime already threads a single inference, so the measured gain is about
1.8x on eight cores, not 8x, and each worker holds its own models.

## Consequences

Good: the three passes cost roughly one decode instead of three. Projected on the same
hardware, complete analysis of 200,000 photographs falls from around 13 hours to about
one. Every future consumer of pixels — a new model, a different thumbnail size — joins
the existing decode instead of adding another.

Costly: the passes become coupled where they were independent. Running face detection
without embedding was previously free; now it means running the combined pass with a
stage disabled, which the API must express. Memory also rises with worker count, since
each holds a detector, a face embedder and a vision encoder.

That last sentence was written before anything was measured, and it names the wrong
term. The three models are about 50 MB per worker; the decoded frames each worker holds
are up to 36 MB apiece, and what dominates is neither — it is the allocator keeping
freed large blocks per size class, so a library of mixed resolutions strands a region
per size. Between two and four workers peak RSS barely moves (1267 MB vs 1315 MB on 953
photographs); dropping to one roughly halves it, because it halves the images in flight
rather than the models. See STATUS.md, Memory.

Also costly: a failure in one stage must not abandon the others. A photograph whose face
detection throws still deserves its thumbnail, so stages are attempted independently and
report separately.

The text encoder is not loaded at all during analysis. It is 161 MB and answers queries,
not photographs; loading it per worker was buying nothing.
