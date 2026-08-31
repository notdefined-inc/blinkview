//! Watching a library for changes made outside the app.
//!
//! Scanning on open (ADR-0011) covers everything except photographs arriving while the
//! window is already open — dropping a folder of holiday pictures into `Trip/` in
//! Finder and switching back. This closes that gap.
//!
//! Two properties matter more than promptness:
//!
//! - **Debounced.** Copying 400 files produces hundreds of events. Rescanning on each
//!   would be far more expensive than the copy itself, so events are collected and a
//!   single rescan runs once the burst has been quiet for a moment.
//! - **Ignores our own writes.** The cache lives outside the library now (ADR-0019),
//!   so the only thing blinkview writes into a watched folder is the `.blinkview-id`
//!   marker — and reacting to *that* would rescan on the very first open, forever.

use notify::{Event, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the filesystem must be quiet before a rescan runs.
///
/// Long enough that a large paste is one rescan rather than dozens; short enough that
/// dropping in a single photograph still feels immediate.
const QUIET: Duration = Duration::from_millis(700);

pub struct Watchers {
    inner: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}

impl Default for Watchers {
    fn default() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }
}

impl Watchers {
    /// Watch `root`, calling `on_change` after the filesystem settles.
    ///
    /// Watching the same root twice is a no-op, so this is safe to call whenever a
    /// source is opened.
    pub fn watch<F>(&self, root: &str, on_change: F) -> anyhow::Result<()>
    where
        F: Fn() + Send + 'static,
    {
        let mut map = self.inner.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        if map.contains_key(root) {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(Path::new(root), RecursiveMode::Recursive)?;

        let marker = PathBuf::from(root).join(blinkview_core::cache::MARKER);
        std::thread::spawn(move || debounce(rx, marker, on_change));
        map.insert(root.to_string(), watcher);
        Ok(())
    }

    pub fn unwatch(&self, root: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(root);
        }
    }
}

/// Collect events until the filesystem has been quiet for `QUIET`, then fire once.
fn debounce(rx: mpsc::Receiver<notify::Result<Event>>, marker: PathBuf, on_change: impl Fn()) {
    let mut pending: Option<Instant> = None;
    loop {
        // Wait indefinitely when idle; only poll while a burst is in flight.
        let msg = match pending {
            None => rx.recv().ok().map(Some),
            Some(_) => match rx.recv_timeout(QUIET) {
                Ok(m) => Some(Some(m)),
                Err(mpsc::RecvTimeoutError::Timeout) => Some(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            },
        };
        let Some(msg) = msg else { return };  // the watcher was dropped

        match msg {
            Some(Ok(ev)) if interesting(&ev, &marker) => pending = Some(Instant::now()),
            Some(_) => {}
            None => {
                pending = None;
                on_change();
            }
        }
    }
}

/// Whether an event is worth a rescan.
///
/// Writing the marker is blinkview naming a library, not a photograph arriving, and
/// reacting to it would rescan on first open and then never stop. The cache itself is
/// outside the watched tree since ADR-0019, so there is nothing else of ours in here —
/// but a leftover `.blinkview/` from before the move still is, and rescanning on its
/// thumbnails would be the same loop.
fn interesting(ev: &Event, marker: &Path) -> bool {
    use notify::EventKind;
    if matches!(ev.kind, EventKind::Access(_)) {
        return false;
    }
    ev.paths.iter().any(|p| {
        if p == marker {
            return false;
        }
        !p.iter().any(|c| {
            c == std::ffi::OsStr::new(blinkview_core::library::VAULT_DIR)
                || c == std::ffi::OsStr::new(blinkview_core::library::LEGACY_VAULT_DIR)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, EventKind};

    fn ev(kind: EventKind, path: &str) -> Event {
        Event { kind, paths: vec![PathBuf::from(path)], attrs: Default::default() }
    }

    #[test]
    fn our_own_writes_are_ignored() {
        let marker = PathBuf::from("/lib/.blinkview-id");
        // Naming a library is not a photograph arriving, or the rescan it triggers
        // rewrites the marker and it never stops.
        assert!(!interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/.blinkview-id"),
            &marker
        ));
        // A cache left beside the photographs by a version before ADR-0019, still
        // being written by nothing at all — but ignored all the same if touched.
        assert!(!interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/.blinkview/thumbs/ab.jpg"),
            &marker
        ));
    }

    #[test]
    fn photographs_arriving_are_interesting() {
        let marker = PathBuf::from("/lib/.blinkview-id");
        assert!(interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/Trip/new.jpg"),
            &marker
        ));
    }

    #[test]
    fn merely_reading_a_file_is_not_a_change() {
        let marker = PathBuf::from("/lib/.blinkview-id");
        // Serving a photograph to the grid opens it; that must not look like an edit.
        assert!(!interesting(
            &ev(EventKind::Access(notify::event::AccessKind::Read), "/lib/Trip/a.jpg"),
            &marker
        ));
    }
}
