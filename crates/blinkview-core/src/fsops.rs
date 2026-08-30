//! Filesystem operations that respect the platforms this tool actually runs on.
//!
//! The reference library lives on **exFAT**, which is the common case for an external
//! photo drive shared between machines. Two consequences drive this module:
//!
//! * exFAT reserves `" * / : < > ? \ |`. macOS will happily create a file containing
//!   `:` on such a volume, but the name is invalid and may break on Windows or a TV.
//!   So we validate rather than trusting the OS to refuse.
//! * macOS stores extended attributes in AppleDouble `._name` sidecars. These must
//!   travel with their parent. `std::fs::rename` does not do this the way the `mv`
//!   command does, so we handle it explicitly.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Characters exFAT (and Windows) reserve.
pub const RESERVED: &[char] = &['"', '*', '/', ':', '<', '>', '?', '\\', '|'];

pub fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty filename");
    }
    if let Some(c) = name.chars().find(|c| RESERVED.contains(c)) {
        bail!("filename {name:?} contains reserved character {c:?}");
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        bail!("filename {name:?} contains a control character");
    }
    if name.ends_with(' ') || name.ends_with('.') {
        bail!("filename {name:?} ends with a space or dot");
    }
    Ok(())
}

/// The AppleDouble sidecar path for a file, if one exists on disk.
pub fn sidecar_of(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with("._") {
        return None; // a sidecar has no sidecar
    }
    let s = path.with_file_name(format!("._{name}"));
    s.exists().then_some(s)
}

pub fn is_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("._"))
}

/// Move a file and its sidecar. Refuses to clobber: an existing destination is an
/// error, never a silent overwrite.
///
/// This is the operation that failed mid-run during the manual session, when the
/// destination directory was renamed in Finder while the loop was running. It now
/// reports precisely that rather than falling back to a copy that also fails.
pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        bail!("source vanished: {}", from.display());
    }
    if to.exists() {
        bail!("destination already exists: {}", to.display());
    }
    let parent = to
        .parent()
        .with_context(|| format!("destination has no parent: {}", to.display()))?;
    if !parent.is_dir() {
        bail!(
            "destination directory does not exist: {} (was it renamed or unmounted?)",
            parent.display()
        );
    }
    let sidecar = sidecar_of(from);
    std::fs::rename(from, to)
        .with_context(|| format!("moving {} -> {}", from.display(), to.display()))?;

    // macOS may carry the sidecar automatically; only move it if it is still behind.
    if let Some(sc) = sidecar {
        let dst_name = to.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let dst = to.with_file_name(format!("._{dst_name}"));
        if sc.exists() && !dst.exists() {
            let _ = std::fs::rename(&sc, &dst);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_characters() {
        // The user's requested format was `12:01_pm_...`; on exFAT the colon is invalid,
        // which is why the shipped format uses hyphens.
        assert!(validate_filename("12:01_pm_20_aug_2026.jpg").is_err());
        assert!(validate_filename("a/b.jpg").is_err());
        assert!(validate_filename("q?.jpg").is_err());
        assert!(validate_filename("12-01_pm_20_aug_2026.jpg").is_ok());
    }

    #[test]
    fn rejects_trailing_space_or_dot() {
        assert!(validate_filename("photo .jpg").is_ok());
        assert!(validate_filename("photo.jpg.").is_err());
        assert!(validate_filename("photo ").is_err());
    }

    #[test]
    fn recognises_sidecars() {
        assert!(is_sidecar(Path::new("/x/._photo.jpg")));
        assert!(!is_sidecar(Path::new("/x/photo.jpg")));
    }

    #[test]
    fn refuses_to_clobber() {
        let d = std::env::temp_dir().join(format!("of-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a.jpg"), d.join("b.jpg"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        assert!(move_file(&a, &b).is_err());
        assert_eq!(std::fs::read(&b).unwrap(), b"b"); // untouched
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reports_missing_destination_directory() {
        let d = std::env::temp_dir().join(format!("of-test2-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.jpg");
        std::fs::write(&a, b"a").unwrap();
        let err = move_file(&a, &d.join("Renamed").join("a.jpg")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
        assert!(a.exists()); // source untouched on failure
        std::fs::remove_dir_all(&d).ok();
    }
}
