# Design direction — Aurora Glass

Applies to every blinkview surface. Binding for all UI work: tokens only, an ad-hoc
value is a bug.

## Archetype
**Aurora glass.** A photo-first dark canvas with soft ambient gradient light —
cyan → violet → amber, drawn from the app mark — bleeding through frosted-glass
panels. Reference class: macOS control surfaces and the product mockup in the
2026-08-28 UI spec — floating translucent layers over content, never opaque boxes.
Photographs supply the colour; chrome supplies light, not pixels. Compact and calm,
not playful.

## Type
System stack only (the page is served from localhost; no web fonts):
`ui-sans-serif, -apple-system, "SF Pro Text", "Segoe UI", Roboto, sans-serif`.
Scale: **12 / 13 / 14 / 16 / 20 / 28 / 40**. Body line-height 1.5, headings 1.2.
Numerals in data positions use `font-variant-numeric: tabular-nums`.

## Space
Base unit **4px**. All spacing is a multiple: 4, 8, 12, 16, 24, 32, 48.

## Colour
Neutral glass ramp plus the brand trio. No ad-hoc hex.

| Token | Value | Use |
|---|---|---|
| `--bg` | `#07070b` | page ground, under the aurora |
| `--ink` | `#f4f4f7` | primary text |
| `--ink-dim` | `#a8a8b6` | secondary text |
| `--ink-faint` | `#71717f` | tertiary, metadata |
| `--glass-1` | `rgba(255,255,255,.045)` | panel surfaces |
| `--glass-2` | `rgba(16,16,24,.55)` | floating surfaces (blur) |
| `--glass-3` | `rgba(20,20,28,.78)` | overlays, sheets (blur) |
| `--stroke` | `rgba(255,255,255,.09)` | hairlines |
| `--stroke-hi` | `rgba(255,255,255,.16)` | hover hairlines |
| `--brand-cyan` | `#38bdf8` | brand gradient, accents |
| `--brand-violet` | `#a78bfa` | brand gradient, AI surfaces |
| `--brand-amber` | `#fbbf24` | brand gradient, rating |
| `--accent` | `#6ea8fe` | selection, primary action, focus |
| `--accent-ink` | `#0b0b10` | text on accent |
| `--ok` | `#4ade80` | assigned / confident |
| `--warn` | `#fbbf24` | ambiguous |
| `--danger` | `#f87171` | destructive, errors |

The aurora itself: three fixed radial washes (cyan lower-left, violet lower-right,
amber upper-right) at ≤16% alpha over `--bg`, rendered once on `body::before`.
Confidence uses `--ok` / `--warn` / `--ink-faint`, never a fourth colour.

## Themes
Two themes, one design: **dark** (default, the values above) and **light**. The
active theme is `data-theme` on `<html>`, set by an inline script in `index.html`
before first paint (no flash), persisted to `localStorage` as `of-theme` — a UI
preference, never vault data. First run follows `prefers-color-scheme`.

Light is the same aurora daylit: pastel washes under white glass. It is expressed
as a token flip plus a short override list at the end of `app.css`:

| Token | Light value |
|---|---|
| `--bg` | `#eef0f7` |
| `--ink` / `--ink-dim` / `--ink-faint` | `#17171f` / `#4c4c5a` / `#7d7d8c` |
| `--glass-1` / `--glass-2` / `--glass-3` | white at `.5` / `.62` / `.9` |
| `--stroke` / `--stroke-hi` | `rgba(23,23,45,.11)` / `.22` |
| `--accent` / `--accent-ink` | `#3b82f6` / `#fff` |
| `--ok` / `--warn` / `--danger` | `#16a34a` / `#b45309` / `#dc2626` |

Rules that keep the flip safe:
- New chrome must be token-driven (`color-mix` adapts for free); a hard-coded
  `rgba(255,255,255,…)` wash needs a matching override in the light block.
- **The lightbox stays dark in both themes** — viewer chrome must never compete
  with the photograph. Dark tokens are re-pinned on `.lightbox` under
  `:root[data-theme="light"]`; anything new inside the viewer inherits them.
- Brand hues (`--brand-*`) are shared across themes; text set in a brand hue
  (query chips, Ask accents) gets an explicit darker ink in the light block.

## Shape
Radius **10px** on cards and thumbnails, **16px** on panels, **22px** on sheets,
**999px** on pills and avatars. Hairlines are `1px solid var(--stroke)`; shadows
only on floating layers, never on cards.

## Glass rules (WKWebView traps — load-bearing)
- `backdrop-filter` belongs on the titlebar, sidebar, panels, toasts, menus —
  **never on the full-screen `.lightbox` container**: on WKWebView it swallows
  child painting entirely (measures correctly, never appears).
- `[hidden]{display:none!important}` — class rules with `display` outrank the
  `hidden` attribute; overlays depend on the override.
- No CSS transition on `#lb-img`'s transform — WKWebView leaves it pending and
  computed style reads the start value.

## Density + tone
Compact. Justified grid at a 200px target height, 3px gutters, 10px corners.
Signature elements: the **aurora canvas**, the **Ask panel** (natural-language
questions answered with result cards), and the **lens mark** — an inline SVG of
four rounded frame corners around a gradient lens. AI surfaces glow faintly
violet; nothing bounces, nothing is cartoonish.

## States
Every interactive element ships default / hover / focus-visible / disabled; async
surfaces add loading; lists add empty and error. Focus is a 2px `--accent` ring at
2px offset, never removed. Empty and error states are written as sentences, not
icons.
