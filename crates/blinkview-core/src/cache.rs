//! Where the derived cache lives: outside the photograph folders (ADR-0019).
//!
//! A library used to keep `.blinkview/` beside its photographs. On the reference
//! machine that put **1.9 GB of regenerable thumbnails inside a 26 GB photo folder**,
//! plus `._.blinkview` AppleDouble sidecars on exFAT; a library in Dropbox or iCloud
//! synced the lot and corrupted the SQLite (ADR-0011); a library on read-only media
//! could not be opened at all, because opening created the vault.
//!
//! The cache now lives under one root per machine, and a library finds its own through
//! a `.blinkview-id` marker at its root — about forty bytes. Keyed by marker rather
//! than by path because a cache is expensive to lose: renaming a folder in Finder is a
//! first-class event (`survives_a_folder_renamed_externally`), and a path key would
//! orphan the index, the thumbnails, the face embeddings and the undo journal with it.
//!
//! A marker is not a claim of ownership. When two folders carry the same id — someone
//! duplicated a library in Finder — the second one to be opened mints a fresh id and
//! rewrites its marker, so the copy re-indexes rather than fighting the original for
//! one cache. The breadcrumb left in each cache is what makes that detectable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// The file at a library root naming its cache. Not a photograph, so `scan` never
/// indexes it; `watch` ignores it, or naming a library would itself trigger a rescan.
pub const MARKER: &str = ".blinkview-id";

/// The breadcrumb inside a cache directory, recording the path of the library it
/// belongs to. Written on every open, so it doubles as the copy detector.
const BREADCRUMB: &str = "path";

/// Library root → resolved vault. One map, so [`forget`] really does forget what
/// [`vault_for`] remembered.
static MEMO: LazyLock<Mutex<HashMap<PathBuf, PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Caches live in `Caches`, not Application Support: the contents are disposable, and
/// both Time Machine and iCloud skip that directory. The OS may purge it under disk
/// pressure, which costs a rescan — the event ADR-0011 already treats as normal.
pub fn root() -> PathBuf {
    if let Ok(p) = std::env::var("BLINKVIEW_CACHE") {
        return PathBuf::from(p);
    }
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    return home().join("Library/Caches/dev.notdefined.blinkview");
    #[cfg(all(unix, not(target_os = "macos")))]
    return std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
        .join("blinkview");
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Local"))
        .join("Blinkview")
        .join("cache");
}

/// The vault directory a library root resolves to, creating and migrating as needed.
///
/// Memoized: the desktop's `photo://` handler asks for every thumbnail it serves, and
/// resolving means a canonicalize and a marker read. Only the first ask does work.
pub fn vault_for(lib_root: &Path) -> PathBuf {
    let key = lib_root.to_path_buf();
    if let Ok(map) = MEMO.lock() {
        if let Some(v) = map.get(&key) {
            return v.clone();
        }
    }
    let vault = resolve(lib_root, &root());
    if let Ok(mut map) = MEMO.lock() {
        map.insert(key, vault.clone());
    }
    vault
}

/// A library's vault, when one already exists — without creating, minting or
/// migrating anything.
///
/// For readers that must not cause side effects: `Library::open_readable` waits on a
/// scan that another thread is running, and minting an id here would race it.
pub fn existing_vault_for(lib_root: &Path) -> Option<PathBuf> {
    let canonical = lib_root.canonicalize().unwrap_or_else(|_| lib_root.to_path_buf());
    let id = read_marker(&canonical)?;
    let vault = libraries(&root()).join(id);
    vault.is_dir().then_some(vault)
}

/// Create the temporary cache for a peek without reading or writing a library marker.
///
/// Kept outside `libraries/` so cache listing and pruning never mistake a transient
/// view for a source the user committed to keeping.
pub(crate) fn peek_vault(lib_root: &Path, cache_root: &Path) -> PathBuf {
    let vault = cache_root.join("peek").join(path_id(lib_root));
    for sub in ["", "thumbs", "derived"] {
        let _ = std::fs::create_dir_all(vault.join(sub));
    }
    vault
}

/// Drop the memoized location of a library's cache. Call after deleting one.
pub fn forget(lib_root: &Path) {
    if let Ok(mut map) = MEMO.lock() {
        map.remove(lib_root);
    }
}

/// Every cache directory, with the library path each was last seen serving.
///
/// The listing behind `blinkview cache list`, and the input to `prune`.
pub fn known() -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut out = Vec::new();
    let libs = libraries(&root());
    let Ok(entries) = std::fs::read_dir(&libs) else { return out };
    for e in entries.flatten() {
        let vault = e.path();
        if !vault.is_dir() {
            continue;
        }
        let path = std::fs::read_to_string(vault.join(BREADCRUMB))
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.is_absolute());
        out.push((vault, path));
    }
    out.sort();
    out
}

/// Resolve a library root against an explicit cache root, doing the work.
///
/// Split from [`vault_for`] so a test can aim a library at a cache of its own without
/// touching the machine's — `Library::open` otherwise writes to the real root, and a
/// test suite that littered `~/Library/Caches` would be a bug of its own.
pub(crate) fn resolve(lib_root: &Path, cache_root: &Path) -> PathBuf {
    let root = lib_root.canonicalize().unwrap_or_else(|_| lib_root.to_path_buf());
    let libraries = libraries(cache_root);

    // An existing marker is the fast path: this library has been opened before.
    let mut id = read_marker(&root);
    if let Some(seen) = &id {
        let vault = libraries.join(seen);
        // Two folders, one id: someone copied a library. The copy re-indexes; the
        // original keeps what it had. Only when the original demonstrably still
        // exists — a breadcrumb naming a vanished folder is a library that was
        // *moved*, and a move keeps its cache (ADR-0019).
        let copied = std::fs::read_to_string(vault.join(BREADCRUMB))
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .is_some_and(|p| p != root && p.exists());
        if copied {
            id = None;
        }
    }

    // No marker, or a marker that turned out to be a copy's: mint one.
    let fresh = id.is_none();
    let mut id = id.unwrap_or_else(|| mint(&root, &libraries));
    if fresh && write_marker(&root, &id).is_err() {
        // Read-only media: no marker, so the cache is keyed by where the folder is —
        // which must be *stable*, or every open starts again. Strictly better than
        // refusing to open, which is what used to happen; the cost is that moving
        // the folder loses the cache.
        eprintln!(
            "[blinkview] cannot write {MARKER} in {}; keying this cache by path",
            root.display()
        );
        id = path_id(&root);
    }
    let vault = libraries.join(&id);

    if fresh {
        // A library from before ADR-0019 keeps its cache, journal and all: the vault
        // is renamed in rather than rebuilt. A rename is a metadata hop, so it is
        // instant — and it cannot cross a filesystem, which for a library on an
        // external drive is exactly the case. There the cache starts again and the
        // old directory stays where it is: reported, never deleted from inside
        // someone's photographs.
        let old = root.join(crate::library::VAULT_DIR);
        if old.is_dir() && !vault.exists() {
            match std::fs::rename(&old, &vault) {
                Ok(()) => eprintln!(
                    "[blinkview] moved {} into the cache at {}",
                    old.display(),
                    vault.display()
                ),
                Err(e) => eprintln!(
                    "[blinkview] left {} where it is ({e}); starting a fresh cache at {}",
                    old.display(),
                    vault.display()
                ),
            }
        }
    }

    for sub in ["", "thumbs", "journal"] {
        let _ = std::fs::create_dir_all(vault.join(sub));
    }
    // Best effort: the breadcrumb is how a copy is detected and how `cache list`
    // names a library, not a record anything is keyed on.
    let _ = std::fs::write(vault.join(BREADCRUMB), root.as_os_str().as_encoded_bytes());
    vault
}

fn libraries(cache_root: &Path) -> PathBuf {
    cache_root.join("libraries")
}

/// The marker contents, when they are a well-formed id.
fn read_marker(root: &Path) -> Option<String> {
    let s = std::fs::read_to_string(root.join(MARKER)).ok()?;
    let id = s.trim();
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_string())
}

fn write_marker(root: &Path, id: &str) -> std::io::Result<()> {
    std::fs::write(root.join(MARKER), format!("{id}\n"))
}

/// A fresh id: enough entropy to be unique, checked against the directory it would
/// name, because a collision is not a checksum error — it is two libraries sharing
/// one index.
fn mint(root: &Path, libraries: &Path) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..8 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut seed = format!(
            "{}{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            n,
        );
        seed.push_str(&root.to_string_lossy());
        let id = hex32(seed.as_bytes());
        if !libraries.join(&id).exists() {
            return id;
        }
    }
    // Astronomically unreachable; the counter alone separates attempts.
    hex32(format!("fallback{}", std::process::id()).as_bytes())
}

/// Two FNV-1a passes over the same bytes, printed as 32 hex characters.
///
/// Not cryptography — an id. FNV is written by hand rather than pulled in as a
/// dependency because the alternative (`DefaultHasher`) is seeded randomly per
/// process, and the path-derived fallback must hash the *same* path to the *same* id
/// across runs or it is not a key at all.
fn hex32(bytes: &[u8]) -> String {
    let fnv = |seed: u64| {
        let mut h = seed;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    };
    format!("{:016x}{:016x}", fnv(0xcbf2_9ce4_8422_2325), fnv(0x9e37_79b9_7f4a_7c15))
}

/// The stable id for a library whose marker cannot be written: a hash of where it is.
///
/// Namespaced away from minted ids so the two schemes cannot collide.
pub(crate) fn path_id(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut bytes = b"path:".to_vec();
    bytes.extend_from_slice(canonical.as_os_str().as_encoded_bytes());
    hex32(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinkview-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_marker_round_trips_and_rubbish_does_not() {
        let d = dir("marker");
        assert_eq!(read_marker(&d), None, "no marker yet");
        write_marker(&d, &"0".repeat(32)).unwrap();
        assert_eq!(read_marker(&d).unwrap(), "0".repeat(32));
        std::fs::write(d.join(MARKER), "not-an-id\n").unwrap();
        assert_eq!(read_marker(&d), None, "a malformed marker is not an id");
        std::fs::remove_dir_all(&d).ok();
    }

    /// The fallback key has to be stable across calls, or a read-only library gets a
    /// new cache every time it opens — which is no cache at all.
    #[test]
    fn a_path_id_is_stable_and_never_a_minted_one() {
        let d = dir("pathid");
        let a = path_id(&d);
        assert_eq!(a, path_id(&d));
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, path_id(d.parent().unwrap()));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn minted_ids_are_unique_and_well_formed() {
        let d = dir("mint");
        let libs = libraries(&d);
        std::fs::create_dir_all(&libs).unwrap();
        let a = mint(&d, &libs);
        let b = mint(&d, &libs);
        assert_ne!(a, b);
        for id in [a, b] {
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|c| c.is_ascii_hexdigit()));
        }
        std::fs::remove_dir_all(&d).ok();
    }
}
