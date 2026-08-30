# Dismissing a face, and merging two people
Status: Done (shipped 2026-08-30)   Owner: notdefined   Date: 2026-08-30

## Problem
Two corrections the review flow cannot make.

`people_overview` deliberately lists singletons — a person photographed once is still a
person — so on a phone backup the sidebar fills with "Who is this?" rows for waiters,
passers-by and strangers in the background. There is no way to say *not a person I care
about*: clusters are recomputed from unassigned faces on every pass, so an ignored group
comes back every time, and there is nowhere to record that it was ignored.

And merging only works in one direction. Naming a cluster with an existing person's name
extends that person (`add_references`), and the name prompt offers known people first for
exactly that reason. But two people who are *already named* — "Sam" and "Samantha", the
same person named twice — can only be fixed by forgetting one, which throws its reference
embeddings away. A correction should not make recognition worse.

## Non-goals
- Dismissing does **not** learn a "not a person" identity. No threshold, no matching
  against dismissed faces — the failure mode of that is hiding a real person, which is
  much worse than showing a stranger.
- No per-face dismissal from inside a cluster. The unit is the group, which is the unit
  the sidebar offers.
- No undo of an individual dismissal in this spec: dismissals are restored together.

## Design
**A dismissal is recorded against the faces themselves**, because a cluster has no
durable identity — its id is a position in a recomputed list. Every face is already
addressed by `(photo content hash, face index)`, which survives rescans, renames and
moves, so that pair is what is stored, as `"<hash>:<idx>"` in a new `dismissed` list on
`People`. It lives in `openfoto-people.json` beside the names, so it survives deleting
the cache (ADR-0007) and travels with the folder.

`cluster_unassigned` filters dismissed faces out before grouping. A dismissed face is
therefore invisible to review but still in the index: nothing is deleted, and restoring
is putting the list back.

The honest limit, stated in the UI's wording rather than hidden: dismissing these faces
does not dismiss *that person*. A new photograph of the same stranger forms a new group.

**Merging folds one person into another**: references are concatenated (a set, never a
centroid — ADR-0003), exclusions are unioned, and the emptied name is removed. Merging
strictly improves recognition, which is the difference from forgetting.

Rejected: dismissing by cluster id (no durable identity), and treating dismissed faces
as a hidden person (a threshold that can swallow someone real).

## Acceptance criteria
1. Dismissing an unnamed group removes it from the sidebar without touching any
   photograph, and the photographs stay in the library.
2. It stays dismissed across a re-cluster, an app restart, and a deleted `.openfoto/`.
3. A dismissed face is not offered as a suggestion for any other group.
4. The sidebar shows how many are dismissed, and one click brings them all back.
5. Restoring re-creates the same groups as before they were dismissed.
6. Dismissing a group does not affect named people or their photograph counts.
7. Merging "Samantha" into "Sam" leaves one person named "Sam" holding both sets of
   references, and "Samantha" is gone.
8. Photographs matched by either name are all matched by the survivor after a merge.
9. Exclusions from both people survive the merge, so a correction is never undone.
10. Merging a person into themselves, or into a name that does not exist, is refused
    rather than silently dropping references.

## Tasks
- [x] 1. `People::dismiss/is_dismissed/restore_dismissed/merge` + tests
      (touches: crates/openfoto-core/src/faces/people.rs)
- [x] 2. `cluster_unassigned` skips dismissed faces (touches: faces/pipeline.rs)
- [x] 3. `dismiss_cluster`, `restore_dismissed`, `merge_people` commands; dismissed
      count on the overview (touches: apps/desktop/src-tauri/src/lib.rs)
- [x] 4. Sidebar: ✕ on an unnamed row, ⇄ merge on a named row, a faint "N dismissed"
      row that restores (touches: apps/desktop/dist/app.js, app.css)

## Verification notes
Driven in the running app against a scratch library of 40 real photographs (copies, so
the source library was never written to — its index checksum was identical afterwards).
Face detection produced **8 unnamed groups, four of them singletons** — the passer-by
problem this spec exists for.

- Dismissing the largest group reported *Set aside 9 faces from 9 photographs*. The
  group vanished, the other seven were untouched (`[9,3,3,2,1,1,1,1]` → `[3,3,2,1,1,1,1]`),
  and the library still held 40 photographs before and after.
- Re-clustering did not bring it back.
- It persisted as `"<hash>:<idx>"` in `openfoto-people.json` **at the library root**,
  with nothing written into `.openfoto/`, so it survives deleting the cache by
  construction (ADR-0007) — and a unit test asserts the round trip.
- Restoring reported *9 faces back for naming* and re-created **exactly** the groups
  that existed before: `[9,3,3,2,1,1,1,1]`.
- Merging: naming two groups separately gave `Sam=13` and `Samantha=3`; merging
  Samantha into Sam reported *Samantha is now Sam · 3 more reference faces for Sam* and
  left one person, `Sam=15`. Fifteen rather than sixteen because one photograph held
  both faces and photographs are counted as a set — the merge did not double-count it.
- Refusals: *Sam is already Sam* (into themselves, case-insensitively) and *Nobody is
  not someone this library knows*. A refused merge changes nothing, which a unit test
  also asserts.
- The merge control only appears when there is somebody to merge into: with one named
  person the row shows the ✕ alone.

Screenshotted: the sidebar with the faint *"2 faces set aside ↩"* row, and the merge
dialog — *"Sam is the same person as…"* with the other people as face pills, one click
each. The ✕ and ⇄ use the existing `.pact` reveal, confirmed rendering under
`:focus-within` (which is the same rule as `:hover`; WKWebView cannot be made to latch
a synthetic hover, a limit recorded in the thumbnail-performance spec).
