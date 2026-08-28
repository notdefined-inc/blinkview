//! Ratings, labels and other things only the user can tell us.
//!
//! This lives at the **library root**, not inside `.openfoto/`, and that placement is
//! the whole point. A star rating cannot be recomputed — it exists nowhere but in
//! someone's head until they record it — so putting it in a cache the documentation
//! calls disposable would mean `rm -rf .openfoto` silently destroys work.
//!
//! At the root it survives deleting the cache, travels with the folder when it is
//! copied, and is visible in Finder next to `Trash/` and `Originals/`. No photograph is
//! modified to store it. See ADR-0007.
//!
//! Keyed by content hash, so ratings survive renaming and moving a photo, including
//! when that happens in Finder.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Finder-style colour labels, which is what most people already have a mental model
/// for. Stored by name rather than index so the file stays readable.
/// The name of the metadata file, at the library root and in any folder below it.
pub const FILE: &str = "openfoto.json";

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

/// A named query, replacing what albums were used for across folders (ADR-0009).
///
/// Only the query is stored, never a list of members: that is the point. A saved
/// search stays current as photographs are added, where an album would need
/// remembering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedSearch {
    pub name: String,
    pub query: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserData {
    #[serde(default)]
    pub photos: BTreeMap<String, PhotoMeta>,
    /// Library-wide, so only meaningful in the root file. A folder describes its
    /// photographs; it does not describe how the whole library is searched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub searches: Vec<SavedSearch>,
}

impl UserData {
    /// The visible file at the library root.
    pub fn path(root: &Path) -> std::path::PathBuf {
        root.join(FILE)
    }

    /// Where this data used to live, inside the disposable cache. Read once so an
    /// existing library does not lose its ratings on upgrade.
    pub(crate) fn legacy_path(root: &Path) -> std::path::PathBuf {
        root.join(crate::library::VAULT_DIR).join("user.json")
    }

    pub fn load(root: &Path) -> Result<Self> {
        let p = Self::path(root);
        let from = if p.exists() {
            p
        } else {
            let legacy = Self::legacy_path(root);
            if !legacy.exists() {
                return Ok(Self::default());
            }
            legacy
        };
        let data = std::fs::read(&from).with_context(|| format!("reading {}", from.display()))?;
        serde_json::from_slice(&data).with_context(|| format!("parsing {}", from.display()))
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let p = Self::path(root);
        std::fs::write(&p, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", p.display()))?;
        // Once written at the root, the copy in the cache is stale and misleading.
        let _ = std::fs::remove_file(Self::legacy_path(root));
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

/// Every ancestor folder of a photograph's folder, nearest first, ending at the
/// library root (`""`).
fn ancestors(folder: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = folder;
    while let Some((parent, _)) = cur.rsplit_once('/') {
        out.push(parent.to_string());
        cur = parent;
    }
    if !folder.is_empty() {
        out.push(String::new());
    }
    out
}

/// The cascade: one `openfoto.json` per folder, nearest wins (ADR-0010).
///
/// Reads consult a photograph's own folder first, then each ancestor. Writes go to the
/// folder that directly contains the photograph, which is the rule that makes copying a
/// folder in Finder carry its ratings and names along with it.
///
/// Loaded once and held, never walked per photograph.
#[derive(Debug, Default)]
pub struct UserDataSet {
    /// Keyed by folder path relative to the library root; the root itself is `""`.
    by_folder: BTreeMap<String, UserData>,
    dirty: std::collections::BTreeSet<String>,
}

impl UserDataSet {
    /// Read every `openfoto.json` at or below `root`.
    ///
    /// A library written before ADR-0010 has only the root file. That still works — the
    /// root is simply the outermost level of the cascade.
    pub fn load(root: &Path) -> Result<Self> {
        let mut by_folder = BTreeMap::new();
        by_folder.insert(String::new(), UserData::load(root)?);
        collect(root, root, &mut by_folder)?;
        Ok(Self { by_folder, dirty: Default::default() })
    }

    /// Resolved metadata for a photograph, given the folder it lives in.
    pub fn get(&self, hash: &str, folder: &str) -> PhotoMeta {
        let mut here = std::iter::once(folder.to_string()).chain(ancestors(folder));
        here.find_map(|f| self.by_folder.get(&f).and_then(|u| u.photos.get(hash)).cloned())
            .unwrap_or_default()
    }

    /// Change a photograph's metadata, writing into the folder that contains it.
    ///
    /// Any entry inherited from an ancestor is copied down first, so editing one field
    /// does not silently drop the others.
    pub fn edit(&mut self, hash: &str, folder: &str, f: impl FnOnce(&mut UserData)) {
        let inherited = self.get(hash, folder);
        let u = self.by_folder.entry(folder.to_string()).or_default();
        if !inherited.is_empty() && !u.photos.contains_key(hash) {
            u.photos.insert(hash.to_string(), inherited);
        }
        f(u);
        self.dirty.insert(folder.to_string());
    }

    /// Move a photograph's metadata between folders, for when the file moves.
    ///
    /// Without this a move would silently drop a rating, because the entry lives beside
    /// the photograph rather than at the root. Returns whether anything moved.
    pub fn relocate(&mut self, hash: &str, from: &str, to: &str) -> bool {
        if from == to {
            return false;
        }
        let meta = self.get(hash, from);
        if meta.is_empty() {
            return false;
        }
        if let Some(u) = self.by_folder.get_mut(from) {
            if u.photos.remove(hash).is_some() {
                self.dirty.insert(from.to_string());
            }
        }
        self.by_folder
            .entry(to.to_string())
            .or_default()
            .photos
            .insert(hash.to_string(), meta);
        self.dirty.insert(to.to_string());
        true
    }

    /// Write back only the folders that changed.
    pub fn save(&mut self, root: &Path) -> Result<()> {
        for folder in std::mem::take(&mut self.dirty) {
            let Some(u) = self.by_folder.get(&folder) else { continue };
            let dir = if folder.is_empty() { root.to_path_buf() } else { root.join(&folder) };
            let path = dir.join(FILE);
            if u.photos.is_empty() && u.searches.is_empty() {
                // An empty file is litter in a folder people browse in Finder.
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !dir.exists() {
                continue;
            }
            std::fs::write(&path, serde_json::to_vec_pretty(u)?)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        let _ = std::fs::remove_file(UserData::legacy_path(root));
        Ok(())
    }

    /// The library's saved searches, which live only in the root file.
    pub fn searches(&self) -> &[SavedSearch] {
        self.by_folder.get("").map(|u| u.searches.as_slice()).unwrap_or(&[])
    }

    /// Add or replace a saved search by name.
    pub fn set_search(&mut self, name: &str, query: &str) {
        let u = self.by_folder.entry(String::new()).or_default();
        u.searches.retain(|s| s.name != name);
        if !query.trim().is_empty() {
            u.searches.push(SavedSearch { name: name.to_string(), query: query.trim().to_string() });
            u.searches.sort_by(|a, b| a.name.cmp(&b.name));
        }
        self.dirty.insert(String::new());
    }

    /// Drop every album label, once they have become folders.
    pub fn clear_albums(&mut self) {
        for (folder, u) in self.by_folder.iter_mut() {
            let mut touched = false;
            for m in u.photos.values_mut() {
                if !m.albums.is_empty() {
                    m.albums.clear();
                    touched = true;
                }
            }
            u.photos.retain(|_, m| !m.is_empty());
            if touched {
                self.dirty.insert(folder.clone());
            }
        }
    }

    /// Every album name still recorded anywhere in the cascade, with its photo count.
    /// Kept for migrating albums to folders (ADR-0009); nothing else writes albums.
    pub fn albums(&self) -> BTreeMap<String, usize> {
        let mut out: BTreeMap<String, usize> = BTreeMap::new();
        for u in self.by_folder.values() {
            for (name, n) in u.albums() {
                *out.entry(name).or_default() += n;
            }
        }
        out
    }
}

/// Walk for `openfoto.json`, skipping the cache and anything hidden.
fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, UserData>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let f = path.join(FILE);
            if f.exists() {
                let data = std::fs::read(&f).with_context(|| format!("reading {}", f.display()))?;
                let u: UserData = serde_json::from_slice(&data)
                    .with_context(|| format!("parsing {}", f.display()))?;
                out.insert(rel, u);
            }
            collect(root, &path, out)?;
        }
    }
    Ok(())
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
    fn is_stored_outside_the_disposable_cache() {
        let p = UserData::path(Path::new("/lib"));
        assert!(!p.to_string_lossy().contains(".openfoto"),
            "user-authored data must survive deleting the cache: {}", p.display());
        assert_eq!(p, Path::new("/lib/openfoto.json"));
    }

    #[test]
    fn reads_data_left_in_the_old_location() {
        let dir = std::env::temp_dir().join(format!("of-legacy-{}", std::process::id()));
        let vault = dir.join(".openfoto");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("user.json"),
            br#"{"photos":{"h":{"rating":3}}}"#).unwrap();

        // An upgrade must not lose ratings written before the move.
        let u = UserData::load(&dir).unwrap();
        assert_eq!(u.get("h").rating, 3);

        // Saving relocates it and clears the stale copy.
        u.save(&dir).unwrap();
        assert!(dir.join("openfoto.json").exists());
        assert!(!vault.join("user.json").exists(), "the stale copy should be removed");
        std::fs::remove_dir_all(&dir).ok();
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

    /// Matches the convention in tests/lifecycle.rs — no extra dependency for this.
    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir()
                .join(format!("openfoto-ud-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_nearest_file_wins() {
        let d = Tmp::new("nearest");
        let root = d.path();
        std::fs::create_dir_all(root.join("Trip/Greece Day3")).unwrap();
        let mut set = UserDataSet::default();
        set.edit("h", "", |u| u.set_rating("h", 1));
        set.edit("h", "Trip", |u| u.set_rating("h", 3));
        set.edit("h", "Trip/Greece Day3", |u| u.set_rating("h", 5));
        assert_eq!(set.get("h", "Trip/Greece Day3").rating, 5);
        assert_eq!(set.get("h", "Trip").rating, 3);
        assert_eq!(set.get("h", "").rating, 1);
        // A folder with no file of its own inherits from the nearest ancestor.
        assert_eq!(set.get("h", "Trip/Greece Day1").rating, 3);
        let _ = root;
    }

    #[test]
    fn a_write_lands_beside_the_photograph() {
        let d = Tmp::new("write");
        let root = d.path();
        std::fs::create_dir_all(root.join("Trip/Greece Day3")).unwrap();
        let mut set = UserDataSet::default();
        set.edit("h", "Trip/Greece Day3", |u| u.set_rating("h", 4));
        set.save(root).unwrap();
        assert!(root.join("Trip/Greece Day3/openfoto.json").exists());
        assert!(!root.join("openfoto.json").exists(), "must not write to the root");
    }

    /// The property the whole decision exists for: copy a folder out on its own and it
    /// is still self-describing.
    #[test]
    fn a_copied_folder_carries_its_metadata() {
        let src = Tmp::new("copied-src");
        std::fs::create_dir_all(src.path().join("Trip/Greece Day3")).unwrap();
        let mut set = UserDataSet::default();
        set.edit("h", "Trip/Greece Day3", |u| u.set_rating("h", 5));
        set.save(src.path()).unwrap();

        let dst = Tmp::new("copied-dst");
        std::fs::copy(
            src.path().join("Trip/Greece Day3/openfoto.json"),
            dst.path().join("openfoto.json"),
        )
        .unwrap();
        // Opened as a library in its own right, the folder still knows the rating.
        let reopened = UserDataSet::load(dst.path()).unwrap();
        assert_eq!(reopened.get("h", "").rating, 5);
    }

    #[test]
    fn editing_one_field_does_not_drop_an_inherited_one() {
        let mut set = UserDataSet::default();
        set.edit("h", "Trip", |u| {
            u.set_rating("h", 4);
            u.set_label("h", Some("red".into()));
        });
        // Set a label deeper down; the inherited rating must come with it.
        set.edit("h", "Trip/Day1", |u| u.set_label("h", Some("blue".into())));
        let m = set.get("h", "Trip/Day1");
        assert_eq!(m.label.as_deref(), Some("blue"));
        assert_eq!(m.rating, 4, "the inherited rating was dropped");
    }

    #[test]
    fn relocating_moves_the_entry_and_leaves_nothing_behind() {
        let mut set = UserDataSet::default();
        set.edit("h", "Day1", |u| u.set_rating("h", 5));
        assert!(set.relocate("h", "Day1", "Day3"));
        assert_eq!(set.get("h", "Day3").rating, 5);
        // Not merely copied: the old folder must no longer claim it, or the rating
        // would reappear if the photograph moved back.
        assert_eq!(set.by_folder.get("Day1").map(|u| u.photos.len()), Some(0));
    }

    #[test]
    fn a_root_only_library_still_reads() {
        let d = Tmp::new("rootonly");
        let mut u = UserData::default();
        u.set_rating("h", 3);
        std::fs::write(d.path().join("openfoto.json"), serde_json::to_vec(&u).unwrap()).unwrap();
        let set = UserDataSet::load(d.path()).unwrap();
        // Photograph is two levels down; the root is the outermost cascade level.
        assert_eq!(set.get("h", "Trip/Greece Day3").rating, 3);
    }

    #[test]
    fn an_emptied_file_is_removed_rather_than_left_as_litter() {
        let d = Tmp::new("litter");
        std::fs::create_dir_all(d.path().join("Trip")).unwrap();
        let mut set = UserDataSet::default();
        set.edit("h", "Trip", |u| u.set_rating("h", 4));
        set.save(d.path()).unwrap();
        assert!(d.path().join("Trip/openfoto.json").exists());
        set.edit("h", "Trip", |u| u.set_rating("h", 0));
        set.save(d.path()).unwrap();
        assert!(!d.path().join("Trip/openfoto.json").exists());
    }
}
