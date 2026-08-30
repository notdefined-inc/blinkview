//! The library root and its derived `.blinkview/` directory.

use crate::index::Index;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Name of the derived directory. Everything inside it is rebuildable by `scan`;
/// deleting it must never lose user-visible state (ADR-0001).
pub const VAULT_DIR: &str = ".blinkview";

/// What the same directory was called before the rename (ADR-0017). Adopted rather
/// than abandoned: it holds the index, and discarding it would charge every existing
/// library a full rescan — twelve minutes on the reference phone backup — to pay for
/// a change of name.
pub const LEGACY_VAULT_DIR: &str = ".openfoto";

pub struct Library {
    root: PathBuf,
    pub index: Index,
    /// The metadata cascade, held rather than re-walked.
    ///
    /// Reading it means walking every folder for an `blinkview.json` — 100 ms on a phone
    /// backup, and every query wanted it. Held here and dropped whenever something
    /// writes to it or the folder tree changes, so the walk happens when the answer
    /// could actually have changed instead of once per question.
    user_data: Option<crate::userdata::UserDataSet>,
}

impl Library {
    /// Open a library, creating `.blinkview/` if absent.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            bail!("not a directory: {}", root.display());
        }
        let root = root
            .canonicalize()
            .with_context(|| format!("resolving {}", root.display()))?;

        let vault = root.join(VAULT_DIR);
        Self::adopt_legacy_vault(&root, &vault);
        for sub in ["", "thumbs", "journal"] {
            std::fs::create_dir_all(vault.join(sub))
                .with_context(|| format!("creating {}", vault.join(sub).display()))?;
        }
        let index = Self::open_index(&vault.join("index.sqlite"))?;
        let lib = Self { root, index, user_data: None };
        lib.rescue_user_data();
        Ok(lib)
    }

    /// The metadata cascade, loaded once and reused.
    pub fn user_data(&mut self) -> Result<&crate::userdata::UserDataSet> {
        if self.user_data.is_none() {
            self.user_data = Some(crate::userdata::UserDataSet::load(&self.root)?);
        }
        Ok(self.user_data.as_ref().expect("just loaded"))
    }

    /// Forget the cached cascade. Call after anything writes an `blinkview.json`, or
    /// after the folder tree changes underneath us.
    pub fn invalidate_user_data(&mut self) {
        self.user_data = None;
    }

    /// Open a library that already has an index, without scanning or creating anything.
    ///
    /// For readers that must not wait on a writer: the index is in WAL mode, so this
    /// sees every row the scan has committed so far. Fails when there is no index yet,
    /// which is the caller's cue that there is nothing to read.
    pub fn open_readable(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let db = root.join(VAULT_DIR).join("index.sqlite");
        if !db.is_file() {
            bail!("no index yet at {}", db.display());
        }
        Ok(Self { root, index: Index::open(&db)?, user_data: None })
    }

    /// Open the index, rebuilding it if it is unusable.
    ///
    /// A library kept in iCloud or Dropbox will corrupt its SQLite eventually — this is
    /// well documented and not preventable from in here. Because everything in the
    /// cache is reproducible and nothing user-authored is in it (ADR-0001, ADR-0007),
    /// corruption costs a rescan and nothing else, so it is handled as a normal event
    /// rather than reported as an error. See ADR-0011.
    fn open_index(path: &Path) -> Result<Index> {
        match Index::open(path).and_then(|i| {
            i.integrity_check()?;
            Ok(i)
        }) {
            Ok(i) => Ok(i),
            Err(e) => {
                eprintln!("[blinkview] index unusable ({e}); rebuilding");
                // The sidecars go too: a WAL belonging to a discarded database is
                // worse than no WAL.
                for suffix in ["", "-wal", "-shm"] {
                    let mut p = path.as_os_str().to_os_string();
                    p.push(suffix);
                    let _ = std::fs::remove_file(std::path::PathBuf::from(p));
                }
                Index::open(path)
            }
        }
    }

    /// Take over the cache a pre-rename version left behind.
    ///
    /// Only when there is no current one: if both exist the newer directory is the
    /// truth and the old one is left alone rather than silently merged.
    fn adopt_legacy_vault(root: &Path, vault: &Path) {
        let legacy = root.join(LEGACY_VAULT_DIR);
        if vault.exists() || !legacy.is_dir() {
            return;
        }
        match std::fs::rename(&legacy, vault) {
            Ok(()) => eprintln!("[blinkview] adopted {LEGACY_VAULT_DIR} as {VAULT_DIR}"),
            // A rescan reproduces everything in here, so a failure is not fatal.
            Err(e) => eprintln!("[blinkview] could not adopt {LEGACY_VAULT_DIR}: {e}"),
        }
    }

    /// Move user-authored files out of the cache if an older version left them there.
    ///
    /// Done on open rather than on next save: a library upgraded today and then cleaned
    /// with `rm -rf .blinkview` tomorrow would otherwise lose names and ratings that no
    /// machine can reproduce, without the user ever having done anything wrong.
    fn rescue_user_data(&self) {
        for (legacy, current) in [
            ("people.json", "blinkview-people.json"),
            ("user.json", "blinkview.json"),
        ] {
            let from = self.root.join(VAULT_DIR).join(legacy);
            let to = self.root.join(current);
            if from.exists() && !to.exists() && std::fs::rename(&from, &to).is_ok() {
                eprintln!("[blinkview] moved {legacy} out of the cache to {current}");
            }
        }
        // Named files at the root, from before the rename. Ratings and names are the
        // one thing here no machine can reproduce, so they are carried over by name.
        for (legacy, current) in [
            ("openfoto-people.json", "blinkview-people.json"),
            (crate::userdata::LEGACY_FILE, crate::userdata::FILE),
        ] {
            let from = self.root.join(legacy);
            let to = self.root.join(current);
            if from.exists() && !to.exists() && std::fs::rename(&from, &to).is_ok() {
                eprintln!("[blinkview] renamed {legacy} to {current}");
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn vault(&self) -> PathBuf {
        self.root.join(VAULT_DIR)
    }

    pub fn journal_dir(&self) -> PathBuf {
        self.vault().join("journal")
    }

    /// Absolute path for a library-relative path.
    pub fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Library-relative path for an absolute one, or `None` if outside the library.
    pub fn rel(&self, abs: &Path) -> Option<String> {
        abs.strip_prefix(&self.root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    }
}
