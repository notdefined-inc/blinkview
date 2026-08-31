//! Walking the library and keeping the index in step with the disk.
//!
//! `scan` never mutates photos. It is safe to run at any time and is the only way
//! the index is populated — which is what makes `.blinkview/` disposable (ADR-0001).

use crate::{
    fsops,
    index::FileRow,
    library::{LEGACY_VAULT_DIR, VAULT_DIR},
    timesource, Library,
};
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use walkdir::WalkDir;

/// Directory names that are application or operating-system internals rather than
/// photograph collections. The chosen root is always exempt: explicitly adding
/// `~/Library/Photos` must work even though a `Library` encountered while walking a
/// home directory is skipped.
pub const SKIP_DIRS: &[&str] = &[
    "Library",
    "Applications",
    "System",
    "Volumes",
    "private",
    "node_modules",
    ".git",
    ".Trash",
    "$RECYCLE.BIN",
    "Windows",
    "Program Files",
];

/// Stop a survey before an accidentally selected disk can keep the dialog waiting
/// indefinitely. The result says "more than" this limit rather than inventing an
/// exact count from an incomplete walk.
pub const SURVEY_LIMIT: usize = 200_000;

/// Extensions blinkview will index as photographs.
///
/// Kept in step with what `image` is built to decode, plus HEIC which goes through a
/// converter. Indexing a format we cannot read only produces a file that fails once and
/// is then recorded as unreadable.
pub const PHOTO_EXT: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "gif", "tif", "tiff", "bmp", "ico", "tga", "qoi",
    "pbm", "pgm", "ppm", "pnm", "hdr", "dds",
];

/// Every extension a photograph can have, camera RAW included. RAW is indexed from the
/// preview the camera embedded rather than developed (`crate::raw`).
fn is_photo_ext(ext: &str) -> bool {
    PHOTO_EXT.contains(&ext) || crate::raw::RAW_EXT.contains(&ext)
}
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

/// The inexpensive, pre-commit description of a folder. A survey reads directory
/// entries only: files are never opened, hashed, decoded or inspected for metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Survey {
    pub here: usize,
    pub below: Option<usize>,
    pub subfolders: usize,
    pub excluded: Vec<String>,
}

pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if is_photo_ext(ext.as_str()) {
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
    scan_with_progress(lib, rehash, &crate::progress::silent)
}

/// Scan only files directly inside the library root.
pub fn scan_shallow(lib: &mut Library, rehash: bool) -> Result<ScanStats> {
    scan_shallow_with_progress(lib, rehash, &crate::progress::silent)
}

/// As [`scan`], reporting `(done, total)`.
///
/// Two passes: the first only reads directory entries to learn how many files there
/// are, the second does the work. Walking names is cheap next to hashing contents, and
/// without a total there is no progress to report — only a spinner, which on a 25GB
/// library tells the user nothing about whether to wait.
pub fn scan_with_progress(
    lib: &mut Library,
    rehash: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ScanStats> {
    let shallow = lib.is_shallow();
    let skip_default_dirs = lib.skips_default_dirs();
    scan_with_options(lib, rehash, shallow, skip_default_dirs, progress)
}

/// As [`scan_shallow`], reporting `(done, total)`.
pub fn scan_shallow_with_progress(
    lib: &mut Library,
    rehash: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ScanStats> {
    let skip_default_dirs = lib.skips_default_dirs();
    scan_with_options(lib, rehash, true, skip_default_dirs, progress)
}

fn scan_with_options(
    lib: &mut Library,
    rehash: bool,
    shallow: bool,
    skip_default_dirs: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ScanStats> {
    let mut st = ScanStats::default();
    let mut on_disk: HashSet<String> = HashSet::new();
    let root = lib.root().to_path_buf();

    let walk = if shallow {
        WalkDir::new(&root).max_depth(1)
    } else {
        WalkDir::new(&root)
    };
    let files: Vec<std::path::PathBuf> = walk
        .into_iter()
        // The pre-rename cache is skipped too. A library can still hold one — an older
        // install left it, or a copy of the folder carried it along — and a directory
        // of thumbnails indexed as photographs is a folder of duplicates appearing out
        // of nowhere.
        .filter_entry(|e| should_descend(e, skip_default_dirs))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && !fsops::is_sidecar(e.path()))
        .filter(|e| kind_of(e.path()).is_some())
        .map(|e| e.into_path())
        .collect();

    let counter = crate::progress::Counter::new(files.len(), progress);
    for path in &files {
        counter.tick();
        let path = path.as_path();
        let Some(kind) = kind_of(path) else { continue };
        let Some(rel) = lib.rel(path) else { continue };
        st.seen += 1;
        on_disk.insert(rel.clone());

        let meta = match std::fs::metadata(path) {
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

fn should_descend(entry: &walkdir::DirEntry, skip_default_dirs: bool) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    if name == VAULT_DIR || name == LEGACY_VAULT_DIR {
        return false;
    }
    !skip_default_dirs || !SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip))
}

/// Count media and subfolders before a folder becomes a source.
pub fn survey_folder(root: impl AsRef<Path>) -> Result<Survey> {
    survey_folder_cancellable(root, &|| false)
}

/// [`survey_folder`] with a cancellation predicate for UI callers.
pub fn survey_folder_cancellable(
    root: impl AsRef<Path>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Survey> {
    let root = root.as_ref();
    if !root.is_dir() {
        anyhow::bail!("not a directory: {}", root.display());
    }

    let mut survey = Survey {
        below: Some(0),
        ..Default::default()
    };
    let mut excluded = BTreeSet::new();
    let mut entries_seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        if cancelled() {
            anyhow::bail!("folder survey cancelled");
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // A directory that cannot be read contributes nothing. The eventual scan
            // reports individual failures; the survey remains a cheap size warning.
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            entries_seen += 1;
            if entries_seen > SURVEY_LIMIT {
                survey.below = None;
                survey.excluded = excluded.into_iter().collect();
                return Ok(survey);
            }
            if cancelled() {
                anyhow::bail!("folder survey cancelled");
            }
            let ty = match entry.file_type() {
                Ok(ty) => ty,
                Err(_) => continue,
            };
            if ty.is_dir() {
                survey.subfolders += 1;
                let name = entry.file_name().to_string_lossy().to_string();
                if SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
                    excluded.insert(name);
                } else {
                    stack.push((entry.path(), depth + 1));
                }
            } else if ty.is_file()
                && !fsops::is_sidecar(&entry.path())
                && kind_of(&entry.path()).is_some()
            {
                if depth == 0 {
                    survey.here += 1;
                } else if let Some(below) = survey.below.as_mut() {
                    *below += 1;
                }
            }
        }
    }

    survey.excluded = excluded.into_iter().collect();
    Ok(survey)
}
