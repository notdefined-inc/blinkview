//! Locating the ONNX model files.
//!
//! Models are large (37MB for SFace) so they are not committed. They are looked up in
//! the user cache first so several libraries share one copy, and `.openfoto/` stays
//! disposable — deleting a library's vault must never force a re-download.

use anyhow::{bail, Result};
use std::path::PathBuf;

pub const YUNET: &str = "yunet.onnx";
pub const SFACE: &str = "sface.onnx";

/// Search order: `OPENFOTO_MODELS`, then `./models`, then the user cache.
pub fn search_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("OPENFOTO_MODELS") {
        v.push(PathBuf::from(p));
    }
    v.push(PathBuf::from("models"));
    if let Some(home) = std::env::var_os("HOME") {
        v.push(PathBuf::from(home).join(".cache/openfoto/models"));
    }
    v
}

pub fn find(name: &str) -> Result<PathBuf> {
    for dir in search_paths() {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    bail!(
        "model {name} not found. Looked in: {}\nRun `openfoto models fetch` to download it.",
        search_paths().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )
}
