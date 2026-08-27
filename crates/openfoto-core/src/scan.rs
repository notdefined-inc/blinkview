//! Walking the library and keeping the index in step with the disk.
//!
//! `scan` never mutates photos. It is safe to run at any time and is the only way
//! the index is populated — which is what makes `.openfoto/` disposable (ADR-0001).

use crate::{fsops, index::FileRow, library::VAULT_DIR, timesource, Library};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use walkdir::WalkDir;

pub const PHOTO_EXT: &[&str] = &["jpg", "jpeg", "png"];
pub const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v"];

#[derive(Debug, Default, PartialEq)]
pub struct ScanStats {
    pub seen: usize,
    /// Content-hashed because they were new or had changed.
    pub hashed: usize,
    /// Skipped hashing because size and mtime matched the index.
    pub unchanged: usize,
    /// Re-identified at a new path by content hash — moved outside the tool.
    pub moved: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if PHOTO_EXT.contains(&ext.as_str()) {
        Some("photo")
    } else if VIDEO_EXT.contains(&ext.as_str()) {
        Some("video")
    } else {
        None
    }
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    std::io::copy(&mut f, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn scan(lib: &mut Library, rehash: bool) -> Result<ScanStats> {
    let mut st = ScanStats::default();
    let mut on_disk: HashSet<String> = HashSet::new();
    let root = lib.root().to_path_buf();

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| e.file_name() != VAULT_DIR)
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                st.errors.push(e.to_string());
                continue;
            }
        };
        let path = entry.path();
        if !entry.file_type().is_file() || fsops::is_sidecar(path) {
            continue;
        }
        let Some(kind) = kind_of(path) else { continue };
        let Some(rel) = lib.rel(path) else { continue };
        st.seen += 1;
        on_disk.insert(rel.clone());

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                st.errors.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Fast path: same path, same size, same mtime -> assume unchanged.
        if !rehash {
            if let Some(prev) = lib.index.by_path(&rel)? {
                if prev.size == size && prev.mtime == mtime {
                    st.unchanged += 1;
                    continue;
                }
            }
        }

        let hash = match hash_file(path) {
            Ok(h) => h,
            Err(e) => {
                st.errors.push(format!("{rel}: {e}"));
                continue;
            }
        };
        st.hashed += 1;

        // Was this file known at a path that no longer exists? Then the user moved it
        // in Finder. Re-identify by content rather than treating it as new (ADR-0001).
        let mut moved_from = None;
        for cand in lib.index.by_hash(&hash)? {
            if cand.path != rel && !lib.abs(&cand.path).exists() {
                moved_from = Some(cand.path);
                break;
            }
        }

        let (taken, src) = timesource::resolve(path, mtime);
        let row = FileRow {
            hash,
            path: rel.clone(),
            size,
            mtime,
            kind: kind.to_string(),
            taken_at: Some(taken.timestamp()),
            taken_src: Some(src.as_str().to_string()),
        };
        if let Some(from) = moved_from {
            lib.index.remove_path(&from)?;
            st.moved += 1;
        }
        lib.index.upsert(&row)?;
    }

    // Drop rows whose files are gone and were not accounted for as moves.
    for row in lib.index.all()? {
        if !on_disk.contains(&row.path) {
            lib.index.remove_path(&row.path)?;
            st.removed += 1;
        }
    }
    Ok(st)
}
