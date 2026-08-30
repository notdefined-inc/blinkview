# Duplicate review and reclaim space
Status: Shipped 2026-08-30 · Owner: notdefined · 2026-08-30

## Problem

Blinkview already finds near-duplicates correctly: dHash proposes candidates, normalized
pixel RMSE confirms them, complete-linkage prevents chaining, and Laplacian variance
selects the sharpest member. The desktop currently reduces that evidence to a generic
file-move plan. A person cannot compare a burst, override the suggested keeper, or see
how much space a decision will recover before applying it.

## Non-goals

- No cloud model or upload. Detection and scoring remain on-device.
- No automatic deletion. A recommendation is never a decision.
- No new album database. “Add to Album” is “Move to Folder” under ADR-0009.
- No permanent erase in this flow. Rejected files first enter Blinkview Trash and remain
  undoable.

## Design

`duplicate_review` exposes the existing groups as review data instead of a move plan.
Each item carries its content hash, path, byte size, capture time, dimensions, and the
existing sharpness score. The backend marks the highest-scoring item as the suggested
keeper; the browser keeps all user choices in memory until Apply.

The review is a full-window workspace: a narrow day/trip queue at left, one large
side-by-side comparison, and a filmstrip underneath. The still-visible originals make
the compare useful at first paint; selecting a frame upgrades both sides to full
resolution. A compact quality explanation says why the recommendation was made without
pretending the score measures artistic value.

Groups are batched by capture day. When a group has GPS, the nearest known place labels
the day as a trip; grouping never infers or writes a new location. A batch shows its
recoverable bytes and count. `Move N to Trash` previews the exact selection, then uses
the existing journalled delete plan. `Reclaim Space` is the same safe operation across
all reviewed batches, never a separate destructive path.

The toolbar includes Like (five-star rating) and Move to Folder. These update the same
on-disk sidecar and filesystem structures as the rest of the app.

## Acceptance criteria

1. Every returned group has passed pixel confirmation and complete-linkage grouping.
2. The suggested keeper is deterministic and has the highest sharpness score.
3. No file changes until the user confirms the complete keep/trash plan.
4. Applying a batch moves only rejected hashes to Blinkview Trash and `undo` restores
   the exact prior tree.
5. Reclaim totals equal the byte sizes of the files selected for Trash.
6. A user can override every suggested keeper, rate a photo, and move it to a folder.
7. Full-screen compare works from the keyboard and exposes useful accessible labels.
8. The review stays usable when a group contains a mixture of portrait and landscape
   images or missing capture/GPS metadata.

## Tasks

- [x] Add duplicate-review DTOs and command over the existing dedupe engine.
- [x] Build the day/trip queue, compare canvas, filmstrip, and staged decision model.
- [x] Route apply/reclaim through the existing reversible Trash plan.
- [x] Add focused Rust and command-grammar tests.
- [x] Render, screenshot, and visually inspect the populated review state.

