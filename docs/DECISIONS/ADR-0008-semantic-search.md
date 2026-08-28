# ADR-0008: Semantic search with MobileCLIP

Date: 2026-08-28
Status: Accepted

## Context

"Find the photo with the dog" is how people actually look for photographs, and openfoto
could not answer it. The existing embeddings are no help: SFace produces 128 numbers
trained solely to tell one person's face from another. Nothing in that vector knows what
a dog is — asking it about "sky" is like asking a fingerprint reader what someone wore.

Answering the question needs a model that embeds images **and text** into one shared
space, so a typed phrase becomes a vector comparable against every photo.

Candidates, measured by downloading and running each against 280 real photos from the
reference library:

| Model | Vision | Text | Total |
|---|---|---|---|
| MobileCLIP-S0 | 11 MB int8 / 43 MB fp32 | 41 MB int8 | **52–84 MB** |
| CLIP ViT-B/32 | 85 MB int8 | 62 MB int8 | 147 MB |
| SigLIP base | 95 MB int8 | — | ~190 MB |

MobileCLIP-S0 is both the smallest and, per Apple's published figures, more accurate at
zero-shot classification than ViT-B/32 — the usual size/quality tradeoff does not apply
here, so there was no reason to pay for the larger models.

## Decision

MobileCLIP-S0: **fp32 vision (43 MB) with fp32 text (161 MB)**, 204 MB total.
*(Amended — this originally read "int8 text (41 MB), 84 MB total". See Correction below.)*

Quantised vision was measurably worse in the way that matters: mean pairwise similarity
across 40 photos was 0.703 quantised against 0.566 at fp32. A compressed embedding space
discriminates less, and 32 MB is a cheap price for the difference. The same turned out to
be true of the text encoder, for a reason the original reasoning missed — see Correction.

Both encoders produce 512-d vectors, L2-normalised on write so a query is a dot product.
Image embeddings are cached in the index keyed by content hash, like every other derived
artefact, and rebuilt by rescanning.

Preprocessing follows the model's own `preprocessor_config.json`: resize shortest edge to
256, centre crop 256, scale to 0..1, **no mean/std normalisation** (`do_normalize:false`,
which is unusual and easy to get wrong by assuming CLIP's defaults). Text is CLIP BPE via
the `tokenizers` crate reading the model's `tokenizer.json`, padded to the fixed
77-token context — the encoder rejects anything shorter.

## Measured quality

Top score against 280 real photos:

    a night sky        0.282   four night shots
    a train            0.231   train interior and platform
    a church           0.226   the wooden church interior
    a selfie of a man  0.220   four selfies
    a laptop computer  0.207   correct
    snowy mountains    0.202   correct
    flowers            0.175   weak
    the sea            0.160   wrong

Raw cosine between CLIP text and image embeddings is low by nature; what matters is that
**score tracks correctness**, which makes a threshold meaningful: results below it are not
shown rather than being presented as answers.

The numbers above were measured against the int8 text encoder and no longer describe the
shipping path. Re-measured on 237 photos with fp32 text, the separation is:

    correct queries, lowest true positive     0.183   "a laptop computer", "green trees"
    absent concepts, highest false positive   0.168   "a bridge over water"
                                              0.164   "the sea"
                                              0.159   "a dog on a beach"

**The threshold stays 0.18**, now justified by the 0.168-0.183 gap rather than the
0.175-0.20 one. It sits at the top of that gap, which errs toward returning nothing over
returning a wrong answer — the intended bias. The cost is visible: "people sitting
together" scores 0.179 on photos of people sitting together and is rejected.

## Correction — the text encoder must also be fp32

The original decision quantised the text tower on the reasoning that it "runs once per
query, where quantisation costs little". That reasoning conflated **cost** with **error**.
Running once per query means quantisation is cheap in *time*; it says nothing about
whether the vector is right, and every query result depends on that one vector.

Measured against the fp32 reference on 237 photos:

- int8 text vectors diverge from fp32 by cosine **0.89-0.94** — not runtime noise, the
  quantisation itself.
- int8 inflates scores, so at the same 0.18 threshold it returns **149 matches against
  fp32's 78**. For "a selfie of a man" it clears 103 of 237 photos, 43% of the library.
- Top-5 agreement with fp32 falls to 2/5 on some queries: it retrieves *different photos*,
  not merely different scores. For "a church" it ranks a desk scene above the actual
  church, which fp32 gets right.
- int8 is not reproducible across onnxruntime builds. `MatMulInteger` and
  `DynamicQuantizeLinear` derive scale from each input's own activation range, so
  ort 1.22 and Python 1.23 disagree by up to cosine 0.989 on the same input — enough to
  move 8 photos across the threshold. Graph-optimisation level shifts it further; thread
  count does not. With fp32 the two runtimes are **bit-identical** on all 12 queries.

That last point is what makes this load-bearing rather than a quality preference: under
int8 the ADR-0004 parity rule cannot be satisfied at all, so nothing catches the next
silent embedding bug. `tests/semantic_parity.rs` now enforces it.

fp16 (81 MB) would have been the compromise, but onnxruntime cannot load that export —
`SimplifiedLayerNormFusion` references a node that does not exist and initialisation
throws. Not something to ship.

The cost is real and was not chosen lightly: models go from 84 MB to 204 MB. The text
tower is a one-time download, not bundled in the binary, and it buys search results that
do not depend on which onnxruntime happens to be linked.

## Consequences

Good: photos become searchable by what is in them, with no tagging, using machinery
already present — `ort`, the SHA-verified model fetch, the hash-keyed cache.

Costly: 204 MB of extra models, and one image encode per photo on first analysis
(~125 ms each in Python; expected faster in Rust). Adds the `tokenizers` crate. Queries
are matched against everything with a dot product, which is fine to six figures of
photos and would need an ANN index beyond that.

Honest limits: this is a small model. It is reliable for scenes, times of day and broad
subjects, and unreliable for fine distinctions, text in images, and specific breeds or
landmarks. The threshold hides the worst of that; it does not fix it.
