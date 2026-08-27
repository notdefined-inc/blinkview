//! The library root and its derived `.openfoto/` directory.

use crate::index::Index;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Name of the derived directory. Everything inside it is rebuildable by `scan`;
/// deleting it must never lose user-visible state (ADR-0001).
pub const VAULT_DIR: &str = ".openfoto";

pub struct Library {
    root: PathBuf,
    pub index: Index,
}

impl Library {
    /// Open a library, creating `.openfoto/` if absent.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            bail!("not a directory: {}", root.display());
        }
        let root = root
            .canonicalize()
            .with_context(|| format!("resolving {}", root.display()))?;

        let vault = root.join(VAULT_DIR);
        for sub in ["", "thumbs", "journal"] {
            std::fs::create_dir_all(vault.join(sub))
                .with_context(|| format!("creating {}", vault.join(sub).display()))?;
        }
        let index = Index::open(&vault.join("index.sqlite"))?;
        Ok(Self { root, index })
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
