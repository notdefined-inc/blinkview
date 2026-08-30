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
//! - **Ignores our own writes.** Every thumbnail, index write and journal entry lands
//!   inside `.blinkview/`, and reacting to those would rescan forever.

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

        let vault = PathBuf::from(root).join(blinkview_core::library::VAULT_DIR);
        std::thread::spawn(move || debounce(rx, vault, on_change));
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
fn debounce(rx: mpsc::Receiver<notify::Result<Event>>, vault: PathBuf, on_change: impl Fn()) {
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
            Some(Ok(ev)) if interesting(&ev, &vault) => pending = Some(Instant::now()),
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
/// Anything inside `.blinkview/` is our own doing — thumbnails, the index, journal
/// entries — and reacting to it would rescan in a loop.
fn interesting(ev: &Event, vault: &Path) -> bool {
    use notify::EventKind;
    if matches!(ev.kind, EventKind::Access(_)) {
        return false;
    }
    ev.paths.iter().any(|p| !p.starts_with(vault))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, EventKind};

    fn ev(kind: EventKind, path: &str) -> Event {
        Event { kind, paths: vec![PathBuf::from(path)], attrs: Default::default() }
    }

    #[test]
    fn our_own_cache_writes_are_ignored() {
        let vault = PathBuf::from("/lib/.blinkview");
        // Thumbnails and index writes must never trigger a rescan, or the rescan they
        // trigger writes more of them and it never stops.
        assert!(!interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/.blinkview/thumbs/ab.jpg"),
            &vault
        ));
        assert!(!interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/.blinkview/index.sqlite-wal"),
            &vault
        ));
    }

    #[test]
    fn photographs_arriving_are_interesting() {
        let vault = PathBuf::from("/lib/.blinkview");
        assert!(interesting(
            &ev(EventKind::Create(CreateKind::File), "/lib/Trip/new.jpg"),
            &vault
        ));
    }

    #[test]
    fn merely_reading_a_file_is_not_a_change() {
        let vault = PathBuf::from("/lib/.blinkview");
        // Serving a photograph to the grid opens it; that must not look like an edit.
        assert!(!interesting(
            &ev(EventKind::Access(notify::event::AccessKind::Read), "/lib/Trip/a.jpg"),
            &vault
        ));
    }
}
