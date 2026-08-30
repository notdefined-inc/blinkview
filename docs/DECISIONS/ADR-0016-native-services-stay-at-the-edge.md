# ADR-0016: Native services stay at the edge

Status: Accepted · 2026-08-30

## Context

AirDrop, Messages, Mail, Save to Files, and Finder file drags are macOS services. A web
facsimile would be incomplete, while exposing filesystem paths or generic shell/process
commands to the webview would enlarge the most privileged boundary in the app.

Update notification has a related boundary problem. The app must contact GitHub to know
that a release exists, but neither the webview nor GitHub needs to know anything about a
library. The requested behavior is notification and an explicit Download button, not an
auto-installer.

## Decision

Native services are narrow Rust edge adapters:

- The frontend passes content hashes. Rust resolves those hashes inside the active
  library and supplies file URLs to AppKit's share or drag APIs. JavaScript never gains
  a generic file-path or process primitive.
- The macOS adapter uses maintained `objc2` bindings and is compiled only on macOS.
  Other platforms return a named unsupported result until an equivalent native adapter
  is designed.
- Update checks use the existing HTTP client to request only GitHub's latest release
  metadata with OpenFoto's application version in the user agent. The response is
  validated and the Download action may open only the returned `github.com` release URL.
- No updater plugin or installer is included. OpenFoto never downloads or executes an
  update in the background.

## Consequences

Sharing looks and behaves like the Mac rather than like a custom export dialog, and the
webview receives less authority. The AppKit code is platform-specific and needs a real
macOS interaction test; compile-time bindings alone are not enough.

Checking on launch necessarily contacts GitHub. The UI says so accurately: no photo or
library data leaves the Mac, but a release request does. Failures are silent on launch
and explicit after a manual check.
