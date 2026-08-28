//! Ratings, labels and other things only the user can tell us.
//!
//! Everything else in `.openfoto/` is derived and can be rebuilt by rescanning. This
//! cannot: a star rating exists nowhere but in someone's head until they record it.
//! It therefore sits alongside `people.json` as a deliberate exception to ADR-0001 —
//! small, plain JSON, and the two files are the only things worth backing up.
//!
//! Keyed by content hash, so ratings survive renaming and moving a photo, including
//! when that happens in Finder.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Finder-style colour labels, which is what most people already have a mental model
/// for. Stored by name rather than index so the file stays readable.
pub const LABELS: [&str; 7] = ["red", "orange", "yellow", "green", "blue", "purple", "grey"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhotoMeta {
    /// 0 means unrated; 1-5 stars otherwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<String>,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

impl PhotoMeta {
    pub fn is_empty(&self) -> bool {
        self.rating == 0 && self.label.is_none() && self.albums.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserData {
    #[serde(default)]
    pub photos: BTreeMap<String, PhotoMeta>,
}

impl UserData {
    pub fn path(vault: &Path) -> std::path::PathBuf {
        vault.join("user.json")
    }

    pub fn load(vault: &Path) -> Result<Self> {
        let p = Self::path(vault);
        if !p.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
        serde_json::from_slice(&data).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self, vault: &Path) -> Result<()> {
        let p = Self::path(vault);
        std::fs::write(&p, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", p.display()))?;
        Ok(())
    }

    pub fn get(&self, hash: &str) -> PhotoMeta {
        self.photos.get(hash).cloned().unwrap_or_default()
    }

    pub fn set_rating(&mut self, hash: &str, rating: u8) {
        let e = self.photos.entry(hash.to_string()).or_default();
        e.rating = rating.min(5);
        self.prune(hash);
    }

    pub fn set_label(&mut self, hash: &str, label: Option<String>) {
        let e = self.photos.entry(hash.to_string()).or_default();
        e.label = label.filter(|l| LABELS.contains(&l.as_str()));
        self.prune(hash);
    }

    pub fn set_album(&mut self, hash: &str, album: &str, member: bool) {
        let e = self.photos.entry(hash.to_string()).or_default();
        e.albums.retain(|a| a != album);
        if member {
            e.albums.push(album.to_string());
        }
        self.prune(hash);
    }

    pub fn albums(&self) -> BTreeMap<String, usize> {
        let mut out: BTreeMap<String, usize> = BTreeMap::new();
        for m in self.photos.values() {
            for a in &m.albums {
                *out.entry(a.clone()).or_default() += 1;
            }
        }
        out
    }

    /// Drop entries that carry no information, so the file does not grow with every
    /// star that was set and then cleared.
    fn prune(&mut self, hash: &str) {
        if self.photos.get(hash).is_some_and(|m| m.is_empty()) {
            self.photos.remove(hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_a_rating_removes_the_entry() {
        let mut u = UserData::default();
        u.set_rating("h", 4);
        assert_eq!(u.get("h").rating, 4);
        u.set_rating("h", 0);
        assert!(u.photos.is_empty(), "an empty entry should not be kept");
    }

    #[test]
    fn ratings_are_capped() {
        let mut u = UserData::default();
        u.set_rating("h", 99);
        assert_eq!(u.get("h").rating, 5);
    }

    #[test]
    fn unknown_labels_are_rejected() {
        let mut u = UserData::default();
        u.set_label("h", Some("chartreuse".into()));
        assert_eq!(u.get("h").label, None);
        u.set_label("h", Some("blue".into()));
        assert_eq!(u.get("h").label.as_deref(), Some("blue"));
    }

    #[test]
    fn album_membership_does_not_duplicate() {
        let mut u = UserData::default();
        u.set_album("h", "Trip", true);
        u.set_album("h", "Trip", true);
        assert_eq!(u.get("h").albums, vec!["Trip"]);
        assert_eq!(u.albums().get("Trip"), Some(&1));
        u.set_album("h", "Trip", false);
        assert!(u.photos.is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("of-user-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut u = UserData::default();
        u.set_rating("a", 5);
        u.set_label("a", Some("red".into()));
        u.set_album("a", "Best", true);
        u.save(&dir).unwrap();
        let back = UserData::load(&dir).unwrap();
        assert_eq!(back.get("a").rating, 5);
        assert_eq!(back.get("a").label.as_deref(), Some("red"));
        assert_eq!(back.get("a").albums, vec!["Best"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
