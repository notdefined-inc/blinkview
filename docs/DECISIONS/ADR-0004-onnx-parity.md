# ADR-0004: Matching OpenCV's inference exactly

Date: 2026-08-27
Status: Accepted

## Context

Every threshold in ADR-0003 was tuned against OpenCV's Python implementation. Porting to
`ort` only preserves those thresholds if the Rust pipeline produces the *same numbers*.
Three discrepancies surfaced, none of which announce themselves at runtime.

**1. SFace is fed RGB; YuNet is fed BGR.** OpenCV's `FaceRecognizerSF::feature` calls
`blobFromImage(..., swapRB = true)`, so the recognizer receives RGB even though the source
Mat is BGR. `FaceDetectorYN` uses the default `swapRB = false` and receives BGR. Feeding
SFace BGR produced embeddings that still clustered plausibly — cosine **0.91** against the
reference instead of 1.0. Nothing crashes; the thresholds simply stop meaning what they
were measured to mean.

**2. The `2023mar` YuNet export has a fixed 640x640 input.** OpenCV's DNN module silently
reshapes the graph when `setInputSize` is called; `ort` refuses. Forcing photos into
640x640 would halve working resolution and drop small faces below the size where
embeddings are reliable.

**3. The dynamic export needs dimensions that are multiples of 32.** Otherwise the
stride-32 branch disagrees with its skip connection ("broadcast an axis by a dimension
other than 1, 36 by 37").

## Decision

Use the **`2026may`** YuNet export, whose height and width axes are symbolic, and
zero-pad inputs on the right and bottom to a multiple of 32. Padding the far edges leaves
the origin fixed, so detected coordinates need no correction.

Feed **BGR to YuNet** and **RGB to SFace**, matching OpenCV's per-model `swapRB`.

Alignment reproduces `getSimilarityTransformMatrix`: a least-squares similarity transform
onto ArcFace's canonical 112x112 five-point template, sampled bilinearly.

## Consequences

Measured parity against OpenCV on real photos from the reference library:

| Stage | Agreement |
|---|---|
| SFace on an identical crop | cosine **1.000000** |
| Full pipeline (our align + our embed) | cosine **0.9956** mean, 0.9865 worst |
| Detector: face counts | identical on all 10 images, including a 3-face shot |
| Detector: box IoU | **0.990** mean |

The residual in the full pipeline is bilinear-interpolation rounding in the warp, three
orders of magnitude inside the 0.50 similarity threshold. The thresholds in ADR-0003
therefore carry over unchanged.

`tests/onnx_parity.rs` pins this with a committed synthetic fixture and its OpenCV
reference embedding, and its failure message names the channel-order bug explicitly. The
fixture is synthetic rather than a real face crop, so no one's photograph enters the repo.

Because detection now uses a different export than the one the thresholds were measured
against, detection-side values (score 0.75, 50px minimum, 4% scenery ratio) should be
re-confirmed on the full library before `faces` is trusted unattended.
