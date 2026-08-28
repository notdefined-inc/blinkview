# ADR-0006: Editing keeps the original in a visible folder

Date: 2026-08-28
Status: Accepted

## Context

Rotate and crop are the first features that change a photograph rather than move it.

Apple Photos, Google Photos and Samsung Gallery all converge on the same guarantee: the
original is never destroyed and "revert to original" is always available. Apple stores
the original plus an adjustments blob and renders on demand; Google retains the original
behind an explicit revert; Samsung keeps it alongside. None of them silently overwrites
a user's file.

They can do that because each owns a database. openfoto deliberately does not, and
`.openfoto/` is disposable by design (ADR-0001). Keeping the only copy of an original
there would mean **deleting a cache destroys the user's photograph** — which is worse
than any of the three products we are learning from.

## Decision

Editing writes the edited image in the photo's place and moves the untouched original to
a visible `Originals/` folder in the library root — the same shape as `Trash/`. The move
is journalled, so undo reverses it.

`keep_original` defaults to **true**, in the type (`serde` default) and in the UI, where
"Keep the original" is pre-selected. A destructive save is offered, labelled as
unrecoverable, because a user who has decided is entitled to decide.

Rejected: **edits in the vault**, which would be lost with a cache the docs promise is
disposable. Rejected: **sidecar edit files**, which are invisible to every other
application and so break the premise that a folder organised by openfoto is just a
folder of photographs.

## Order of operations

Quarter-turn, flip, straighten, adjust, then crop — in that order. Rotate and flip run
**before** crop. The crop rectangle is drawn by the user on the
transformed preview, so its fractions are in that space; cropping first would apply
their rectangle to the untransformed image and cut the wrong region. A test asserts
rotate-then-crop on a 40x10 image yields 10x20.

Straightening by a few degrees leaves blank wedges at the corners, so the result is
trimmed to the largest inscribed rectangle containing no blank area. A test asserts all
four corners of a straightened image still carry pixels.

The preview is a CSS filter and the save is per-pixel Rust. Those two must agree or the
preview lies about the result, so `filterFor()` in the frontend deliberately mirrors
`edit::adjust` — the squared contrast curve and Rec. 601 luma included.

## Consequences

Good: the guarantee those three products offer, without inventing a database. Finder
shows the originals. Other applications see an ordinary edited JPEG rather than an edit
list they cannot read. Deleting `.openfoto/` still costs nothing.

Costly: an edited photo is a re-encode, so repeated edit cycles lose quality — unlike
Apple's model, which always re-renders from the original. `Originals/` grows and is the
user's to manage, as `Trash/` is. And the edit changes the file's content hash, so the
photo is re-identified on the next scan and its cached thumbnail and face data are
discarded and rebuilt.
