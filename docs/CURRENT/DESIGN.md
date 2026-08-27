# Design direction

Applies to every openfoto surface: the review page today, the Tauri viewer later.

## Archetype
**Immich's photo-first density.** Near-black canvas, edge-to-edge thumbnail grid, chrome
that recedes so photographs carry the colour. Compact and serious, not playful. We follow
the conventions of that class of app — dark ground, dense grid, minimal furniture — with
our own palette and no borrowed branding or assets.

## Type
System stack (`ui-sans-serif, -apple-system, "Segoe UI", Roboto, sans-serif`) — the page is
served from localhost with no network, so no web fonts.
Scale: **12 / 14 / 16 / 20 / 28 / 40**. Body line-height 1.5, headings 1.2.
Numerals in data positions use `font-variant-numeric: tabular-nums`.

## Space
Base unit **4px**. All spacing is a multiple: 4, 8, 12, 16, 24, 32, 48.

## Colour
Neutral ramp (near-black, photo-first) plus a single accent hue.

| Token | Value | Use |
|---|---|---|
| `--bg` | `#0b0b0e` | page ground |
| `--surface` | `#141419` | cards, sidebar |
| `--surface-2` | `#1c1c23` | raised / hover |
| `--line` | `#2a2a33` | hairlines |
| `--text` | `#f2f2f5` | primary text |
| `--text-dim` | `#a1a1ad` | secondary |
| `--text-faint` | `#6e6e7a` | tertiary, metadata |
| `--accent` | `#7c9cff` | selection, primary action |
| `--accent-ink` | `#0b0b0e` | text on accent |
| `--ok` | `#4ade80` | assigned / confident |
| `--warn` | `#fbbf24` | ambiguous |
| `--danger` | `#f87171` | destructive, errors |

No ad-hoc hex. Confidence uses `--ok` / `--warn` / `--text-faint`, never a fourth colour.

## Shape
Radius **8px** on cards and thumbnails, **999px** on pills and avatars. One hairline border
(`1px solid var(--line)`); shadows only on overlays, never on cards.

## Density + tone
Compact. Thumbnails tile at 96px with 4px gutters. Chrome is quiet: no gradients, no
decorative icons. The signature element is the **face strip** — a horizontally scrolling
row of square face crops per cluster, which is what the user actually reads.

## States
Every interactive element ships default / hover / focus-visible / disabled. Focus is a
2px `--accent` ring at 2px offset, never removed. Empty and error states are written as
sentences, not icons.
