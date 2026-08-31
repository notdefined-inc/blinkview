# Remote control — browse and drive the app from a paired device

Status: Agreed   Owner: somesh   Date: 2026-08-31

## Problem

The library lives on the desktop, so showing someone photographs means gathering
around the Mac. The roadmap already planned phone apps as *clients* that connect to
the desktop over the network (ROADMAP.md, Platforms) — nothing ships yet. A QR code
on screen, a scan, and the whole app runs in the phone's browser.

## Non-goals

- **Internet reach.** LAN pairing only in this spec. Internet relay needs the E2E
  encryption spec first, then a relay spec (see Design → sequence). The wire protocol
  below must not preclude them, and that is all it does for them here.
- **A second UI.** No responsive redesign or mobile app. The desktop frontend runs
  as-is in the phone browser; only viewport basics and hiding native-only affordances
  are in scope.
- **Upload from the device.** The phone consumes and drives the library; it never adds
  photographs.
- **Multiple simultaneous devices.** One paired device at a time in this spec.

## Design

The premise of ADR-0001 is amended by ADR-0021 (same directory): nothing readable
leaves the machine except to a device the user explicitly paired, and no third party
ever gains the ability to read anything.

**One engine, three fronts.** The bridge is the CLI and the Tauri window's third
peer (ARCHITECTURE.md): it dispatches the *same 68 commands* the frontend invokes,
through the same core functions, with the same journal and Plan discipline. Nothing
about a library may be reimplemented at this layer.

    Phone browser                    Desktop app (Tauri)
    ┌──────────────┐   HTTP   GET    ┌─────────────────────┐
    │ dist/app.js  │ ◄────────────── │  static dist/ + shim │
    │ + remote.js  │   HTTP  Range   │  /photo → scheme     │
    │              │ ◄──────────────►│  handler (same       │
    │ invoke/listen│   WS frames     │  boundary, + Range)  │
    └──────────────┘                 └─────────────────────┘

**Serving.** `tokio` (already in the tree via Tauri) + `axum` for HTTP and WebSocket.
Enabled only by an explicit toggle; binds an ephemeral port on all interfaces.

**Auth.** Toggling generates a fresh 128-bit token. The QR encodes
`http://<lan-ip>:<port>/p/<token>`; that route sets an HttpOnly cookie and redirects
to the clean app URL. Every route — static, `/photo`, `/ws` — requires the cookie.
10 failed pair attempts disable the server until re-toggled.

**Wire protocol.** JSON text frames. Request `{"id":7,"cmd":"set_rating","args":{…}}`;
reply `{"id":7,"ok":true,"result":…}` or `{"id":7,"ok":false,"err":"…"}`. Host pushes
`{"ev":"progress","payload":…}` for every event the window `listen`s for
(`progress`, `source-ready`, `library-changed`, `open-path`). Command names and
argument names are byte-identical to the Tauri commands, so the frontend cannot tell
the transports apart.

**The parity rule.** Dispatch is generic over a command registry shared with the
Tauri window — never a bridge-side whitelist of match arms. A command added for the
window is callable from the browser with no bridge change, and a new event channel
pushes with no bridge change; that is what keeps "the browser can do what the desktop
can do" true as features land. The only sanctioned divergence is native services
(share sheet, Finder drag-out, Open With, folder pickers), which have no browser
equivalent: a feature that leans on one must gate it on `__BLINKVIEW_REMOTE__` when
the feature is built.

**The shim.** `remote.js`, served before `app.js`, defines `window.__TAURI__` when
the page is not under Tauri: `invoke` becomes a WS round-trip, `listen` a WS
subscription, `dialog` rejects with the structured error, and
`window.__BLINKVIEW_REMOTE__` is set. `app.js` changes only where native services
surface (share, drag-out, Open With) — hidden when the flag is set.

**Pixels.** `/photo?…` reuses the scheme handler's authorization (`owning_source`,
peek-parent rule) and its LRU, adding HTTP Range so videos seek on iOS Safari.

**QR + control surface.** A glass dialog: QR (Rust `qrcodegen`, data URL), the URL
in text, connected-device name, Disconnect. The server dies with the toggle.

### Sequence (later specs, not this one)

1. **This spec** — LAN, plaintext, pairing token. Accepted risk: a hostile LAN
   spoofing/reading the channel; documented, revisited in the next spec.
2. **E2E encryption** — the QR carries key material in the URL fragment; the WS
   session upgrades into an encrypted channel. Frame format unchanged.
3. **Relay** — both peers dial out over WSS to a relay that sees ciphertext; video
   bandwidth is the open risk there.

## Acceptance criteria

1. When the toggle is off, nothing listens: a connect to the port is refused, and the
   port does not even exist until toggled (ephemeral bind).
2. When scanning the QR from a phone on the same Wi-Fi, the grid renders within 5 s
   on a 3,000-photo library, thumbnails loading lazily as the view scrolls.
3. When a photograph is opened, the lightbox shows the 2000 px preview; zooming past
   1x loads the original; an MP4 plays and seeks (Range works).
4. When the same query is run on desktop and phone, results match exactly, including
   semantic search on an embedded library.
5. When a rating is set on the phone, the desktop window reflects it without a
   rescan (event push), and `blinkview.json` on disk holds it.
6. When a move is applied from the phone, the preview card is shown first, the journal
   gains an entry, and desktop ⌘Z restores the prior tree.
7. Every route returns 403 without the cookie; a cookie from a previous toggle
   (stale token) is refused; the 10th failed pair attempt disables the server.
8. `/photo` refuses a path outside added sources exactly as `photo://` does (shared
   test over both entry points).
9. Share, drag-out and Open With are absent on the phone without console errors, and
   the desktop window is unaffected.
10. When a command is registered for the window it is dispatchable over the bridge:
    a test enumerates both registries and fails on divergence (bar the explicit
    native-service list), so a future feature cannot ship desktop-only by accident.
11. `cargo test --workspace`, clippy `--all-targets -D warnings`, and
    `node apps/desktop/tests/grammar.test.mjs` pass.

## Tasks

- [ ] 1. ADR-0021 (amend ADR-0001's premise for paired devices) (touches: docs)
- [ ] 2. Factor command bodies out of `#[tauri::command]` wrappers into plain async
      fns callable from both transports; no behavior change (touches: src-tauri)
- [ ] 3. Bridge server: axum app, token gate on every route, static dist/ + shim,
      cookie pairing route, lockout (touches: src-tauri/remote)
- [ ] 4. `/photo` route reusing the scheme boundary + LRU, with Range (touches: src-tauri/remote)
- [ ] 5. WS dispatch + event push over the shared command fns (touches: src-tauri/remote)
- [ ] 6. `remote.js` shim; `app.js` native-affordance gating; mobile viewport basics;
      grammar test still green (touches: dist, tests)
- [ ] 7. QR glass dialog, connections list, Disconnect (touches: dist, src-tauri)
- [ ] 8. Security and parity tests: 403s, lockout, boundary parity, journal-on-remote-move (touches: src-tauri, core tests)
- [ ] 9. STATUS/ARCHITECTURE/ROADMAP updates; spec → Agreed before task 2 starts (touches: docs)
