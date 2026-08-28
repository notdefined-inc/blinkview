# Aurora Glass UI overhaul

Status: done · 2026-08-28

## Intent

Replace the desktop app's visual layer with the Aurora Glass direction
(`docs/CURRENT/DESIGN.md`): frosted panels over an ambient gradient canvas, the
new lens mark, and an **Ask panel** — a natural-language surface that answers
questions about the library with result cards. This is the AI-era face of the
app: the same CLIP/face engines, presented as a conversation instead of a filter.

## Scope

- `apps/desktop/dist/index.html`, `app.css`, `app.js` — rewritten/restyled.
- `docs/CURRENT/DESIGN.md` — rewritten (done in the same change).
- **No Rust changes.** No new commands, no new events, no schema changes.

## Preserved contract (regression fence)

- All 32 invoked commands keep their names/payloads, incl. `semantic_status`,
  `semantic_index`, `semantic_search`, `people_overview`, `clusters`,
  `plan_op`/`apply_op`, `edit_photo`, `set_album`, `undo`.
- Events: `progress` (ops: faces, thumbs, clusters, plan, apply, models,
  semantic) and `tauri://drag-enter/leave/drop`.
- `photo://localhost<path>` with `?t=<hash>` and `?full=<hash>`.
- Edit preview math: `filterFor()` ↔ `edit::adjust`, `straightenZoom()` ↔
  `edit::inscribed_same_aspect` — byte-identical behaviour.
- WKWebView traps in DESIGN.md "Glass rules" stay honoured.
- ADR-0008 states: below-threshold semantic ⇒ nothing, not least-bad; missing
  model says so; in-flight query shows a looking state.

## Ask panel

Right-side floating glass panel, toggled from the titlebar or `⌘K`. A question is
parsed by the existing `parseQuery` (dates, people, albums, `field:value`), then
leftover words go to `semantic_search`; the answer card shows intent chips,
people face chips, a thumbnail strip (click → lightbox), and actions (Show in
library, Select results, Add to album…). Thread is session-only — nothing is
persisted (vault invariant).

## Verification

- Screenshot loop via the tauri-mcp bridge (debug builds): grid, Ask open with a
  query answered, lightbox, sheet, review, empty states; critique against
  DESIGN.md, fix, re-shoot (≥2 passes).
- `cargo clippy --workspace --all-targets -q -- -D warnings` and
  `cargo test --workspace`, branched on exit code.
- Manual sweep: search chips, semantic states, people naming, rating/label,
  albums, rename, delete/restore, organize preview/apply, undo, crop/straighten/
  adjust save, drag-drop import.
