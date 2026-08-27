# ADR-0003: Validated thresholds and the failures behind them

Date: 2026-08-27
Status: Accepted

## Context

Every number below was established empirically against a real 2519-file library
(`/Volumes/Notdefined/Swissgreece`, 9 days of travel photos, 3 recurring people). Most were
found by first getting them wrong and seeing the damage in a rendered contact sheet.

They are recorded here because they are the expensive part of this project. The code that
uses them is mechanical; these values and the reasoning are not.

## Decision

**Duplicate detection.** dHash (8x8, 64-bit) generates *candidates* at Hamming distance
<= 12. Each candidate pair is then confirmed by pixel comparison: 32x32 grayscale,
per-image mean/stddev normalized, accepted at RMSE <= 0.30. Clustering is
**complete-linkage** — every member must match every other member.

Why: dHash alone plus single-linkage chained **85 photos across 6 different days** into one
"duplicate" cluster in which only 9% of pairs were actually similar. The images were not flat
or dark (mean contrast sd 59) — it was pure transitive chaining: A~B, B~C, A≁C. Of clusters
with 3+ members, 69 of 115 exceeded the distance threshold in diameter. Complete-linkage
alone fixed it; the largest cluster fell to 10, all same-day and seconds apart.

RMSE thresholds measured: 0.20 -> 134 photos moved, 0.30 -> 284, 0.45 -> 549. 0.45 visibly
swept in alternate takes where the subject had clearly moved. **0.30 is the default.**

**Face detection.** YuNet at score >= 0.75. Detection at 1280px long edge for bulk scanning;
1920px when embeddings are needed for small faces. Minimum face width 50px for a usable
embedding — below that, SFace embeddings are unreliable.

**Face identity.** SFace 128-d embeddings, L2-normalized, cosine similarity. Assignment is
**discriminative**: compare max-similarity against the target person versus max-similarity
against all *other* known people, and require the target to win. A bare threshold is not
sufficient.

Why: a bare threshold at 0.40 pulled a different man and a blonde woman into the user's
own folder. The floor is **0.50**; below that, do not move.

Separation actually measured on confirmed data: within-person mean **0.842**, cross-person
mean **0.456**. Only 3 of 310 confirmed faces ranked a wrong person higher.

**A person is not one cluster.** The user's face split across **five** clusters because
SFace embeds front-on and profile views differently. Cluster centroids of the *same* person
in different poses sat at 0.77 similarity, while genuinely different people sat at 0.41-0.58.
Clustering therefore proposes groups; a human merges them. This is why the review step is
mandatory rather than a convenience.

**Scenery split.** "No close-up people" = largest face < **4% of image width** (at score
>= 0.75). Bands measured: <4% is incidental figures on stairs and in alleys; 4-7% is
*posed full-body portraits* and must not be swept.

**Capture time.** EXIF `DateTimeOriginal` first, camera filename second, mtime last.
Measured on a 300-photo random sample of the reference library: **100%** carry
`DateTimeOriginal`, and it disagrees with the camera filename in **13%** of cases, always
by exactly one second. EXIF is authoritative.

(An earlier probe reported only 36 of 119 photos having EXIF and nearly drove the opposite
ordering. It was reading the primary IFD, where `DateTimeOriginal` does not live — it sits
in the Exif sub-IFD at 0x8769. Corrected here so the mistake is not repeated.)

**Filenames.** `%I-%M-%S_%p_%d_%b_%Y` lowercased, `_2`/`_3` suffixes for collisions,
unique across the **whole library**.

Why seconds: at minute resolution 81% of the library collided. With seconds, 9%.
Why global: per-folder uniqueness left 65 names duplicated across folders (130 files) that
would have silently overwritten on any merge.

## Consequences

These become regression tests with the real library as fixture. Two are canonical:
the 85-photo cross-day blob must never reappear, and the blonde woman must never be
matched into a person folder at 0.40-0.44.

Tuning is defaulted, not hardcoded — every threshold is a CLI flag. The defaults are what a
user who never touches a flag receives, and they must stay conservative: unclear items stay
put.

Caveats found and unresolved: marble busts and statues are detected as faces; people facing
away are not detected at all and will land in the scenery split regardless of prominence.
