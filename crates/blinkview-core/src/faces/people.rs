//! Named identities and their reference faces.
//!
//! Lives at the **library root** — clustering is recomputable, but knowing a cluster
//! is called "Alex" is not, and a cache the documentation calls disposable is no place
//! for the one thing no machine can reproduce (ADR-0007).
//!
//! What the file *carries* changed, though. It used to hold the reference embedding
//! vectors themselves: 172 KB on the reference library, of which all but a rounding
//! error was vectors the index already had. Since ADR-0019's amendment it stores
//! pointers — `"<hash>:<idx>"`, the same idiom [`People::dismissed`] uses — and the
//! vectors are read back out of the index on load. The file is a few kilobytes, and
//! deleting the cache now costs re-embedding a handful of known faces rather than
//! forgetting a person.
//!
//! A vector nothing points at is kept inline rather than dropped, so a v1 file whose
//! cache has been deleted loses no identity by being upgraded.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    /// Reference embeddings. Deliberately a *set*, not a centroid: the same person
    /// front-on and in profile embeds differently enough that averaging them produces
    /// a vector resembling neither. On the reference library one face split across
    /// five clusters; keeping all of them raised within-person similarity to 0.842
    /// against 0.456 for other people (ADR-0003).
    pub references: Vec<Vec<f32>>,
    /// Photo hashes the user has explicitly said are *not* this person.
    ///
    /// Recognition can be right about a face and still wrong about what the user
    /// wants; an exclusion is a direct correction and always outranks a match. Kept
    /// per person rather than globally so removing someone from one photo does not
    /// affect anyone else in it.
    #[serde(default)]
    pub excluded: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct People {
    #[serde(default)]
    pub people: Vec<Person>,
    /// Faces the user has said are not worth naming — the waiter, the stranger in the
    /// background, the half of a face at the edge of a group shot.
    ///
    /// Recorded as `"<photo hash>:<face index>"` rather than by cluster, because a
    /// cluster has no durable identity: its id is a position in a list recomputed on
    /// every pass. That pair survives rescans, renames and moves, which is what makes
    /// a dismissal stick.
    ///
    /// Deliberately *not* an identity to match against. Treating dismissed faces as a
    /// hidden person would need a threshold, and the failure mode of that threshold is
    /// swallowing someone real — much worse than showing a stranger one more time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<String>,
}

/// How one face is addressed, in [`People::dismissed`] and in a record's `faces`.
fn face_key(hash: &str, idx: i64) -> String {
    format!("{hash}:{idx}")
}

/// The `"<hash>:<idx>"` pair a record stores, split back apart. Hashes are hex and
/// contain no colon, so the last one is the divider.
fn split_face_key(key: &str) -> Option<(String, i64)> {
    let (hash, idx) = key.rsplit_once(':')?;
    Some((hash.to_string(), idx.parse().ok()?))
}

/// One person as the file stores them: pointers to faces, with a vector only when no
/// face could be found to point at.
///
/// A serde shape of its own rather than `Person` with new fields, so that the
/// in-memory type can stay what matching wants — vectors, ready to compare — while
/// the disk stays what a person would want to find beside their photographs.
#[derive(Serialize, Deserialize)]
struct PersonRecord {
    name: String,
    /// `"<hash>:<idx>"`, resolved against the index on load.
    #[serde(default)]
    faces: Vec<String>,
    /// Vectors with nowhere to point. Normally empty; written for a v1 file whose
    /// cache had already been deleted, where dropping them would forget a person.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    references: Vec<Vec<f32>>,
    #[serde(default)]
    excluded: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct PeopleRecord {
    #[serde(default)]
    people: Vec<PersonRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dismissed: Vec<String>,
}

impl PeopleRecord {
    /// Vectors still stored as bytes rather than pointers.
    pub(crate) fn inline_vectors(&self) -> usize {
        self.people.iter().map(|p| p.references.len()).sum()
    }
}

impl People {
    /// The visible file at the library root.
    pub fn path(root: &Path) -> std::path::PathBuf {
        root.join("blinkview-people.json")
    }

    /// Where names used to live, inside the disposable cache.
    fn legacy_path(root: &Path) -> std::path::PathBuf {
        root.join(crate::library::VAULT_DIR).join("people.json")
    }

    /// Read the file at a library root, expanding nothing. The index is not touched,
    /// so a record's `faces` stay pointers — for tests, and for anyone inspecting the
    /// file's shape rather than matching against it.
    pub(crate) fn read_records(root: &Path) -> Result<PeopleRecord> {
        let p = Self::path(root);
        let from = if p.exists() {
            p
        } else {
            let legacy = Self::legacy_path(root);
            if !legacy.exists() {
                return Ok(PeopleRecord::default());
            }
            legacy
        };
        let data = std::fs::read(&from).with_context(|| format!("reading {}", from.display()))?;
        serde_json::from_slice(&data).with_context(|| format!("parsing {}", from.display()))
    }

    /// How many people the file at `root` names.
    ///
    /// For listings that will never match a face and so need no vectors expanded.
    pub fn named_in(root: &Path) -> usize {
        Self::read_records(root)
            .map(|r| r.people.len())
            .unwrap_or(0)
    }

    /// Records against a library: every pointer becomes the vector it names.
    ///
    /// A pointer whose face is gone contributes nothing — there is nothing to compare
    /// against — and the pointer itself stays in the file rather than being pruned,
    /// because the face may be back on the next scan and a name is not the index's
    /// to forget.
    pub(crate) fn from_records(lib: &crate::Library, records: PeopleRecord) -> Self {
        let people = records
            .people
            .into_iter()
            .map(|r| {
                let mut references = r.references;
                for key in &r.faces {
                    if let Some((hash, idx)) = split_face_key(key) {
                        if let Ok(Some(v)) = lib.face_embedding(&hash, idx) {
                            references.push(v);
                        }
                    }
                }
                Person {
                    name: r.name,
                    references,
                    excluded: r.excluded,
                }
            })
            .collect();
        Self {
            people,
            dismissed: records.dismissed,
        }
    }

    /// The file's shape for this library: vectors that name a face become pointers,
    /// and only the unplaceable stay as bytes.
    pub(crate) fn to_records(&self, lib: &crate::Library) -> Result<PeopleRecord> {
        let mut blobs: std::collections::HashMap<Vec<u8>, String> =
            lib.face_blobs()?.into_iter().collect();
        let people = self
            .people
            .iter()
            .map(|p| {
                let mut faces = Vec::new();
                let mut references = Vec::new();
                for v in &p.references {
                    match blobs.remove(&crate::faces::store::to_blob(v)) {
                        // `remove`, so two identical vectors cannot both point at one
                        // face: the second is kept inline rather than aliased.
                        Some(key) => faces.push(key),
                        None => references.push(v.clone()),
                    }
                }
                PersonRecord {
                    name: p.name.clone(),
                    faces,
                    references,
                    excluded: p.excluded.clone(),
                }
            })
            .collect();
        Ok(PeopleRecord {
            people,
            dismissed: self.dismissed.clone(),
        })
    }

    pub fn get(&self, name: &str) -> Option<&Person> {
        self.people.iter().find(|p| p.name == name)
    }

    /// True when the user has said this photo is not this person.
    pub fn is_excluded(&self, name: &str, photo_hash: &str) -> bool {
        self.get(name)
            .is_some_and(|p| p.excluded.iter().any(|h| h == photo_hash))
    }

    /// Record that these photos are not this person.
    pub fn exclude(&mut self, name: &str, hashes: &[String]) {
        if let Some(p) = self.people.iter_mut().find(|p| p.name == name) {
            for h in hashes {
                if !p.excluded.iter().any(|x| x == h) {
                    p.excluded.push(h.clone());
                }
            }
        }
    }

    /// Forget a person entirely.
    ///
    /// Used when the last photograph is untagged: a name that matches nothing is not
    /// information, and leaving it in the sidebar claiming zero photographs is worse
    /// than removing what the user has just finished disowning.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.people.len();
        self.people.retain(|p| p.name != name);
        self.people.len() != before
    }

    pub fn add_references(&mut self, name: &str, refs: Vec<Vec<f32>>) {
        match self.people.iter_mut().find(|p| p.name == name) {
            Some(p) => p.references.extend(refs),
            None => self.people.push(Person {
                name: name.to_string(),
                references: refs,
                excluded: Vec::new(),
            }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty()
    }

    /// Stop offering these faces for naming. The photographs are untouched.
    pub fn dismiss(&mut self, faces: &[(String, i64)]) -> usize {
        let before = self.dismissed.len();
        for (hash, idx) in faces {
            let key = face_key(hash, *idx);
            if !self.dismissed.contains(&key) {
                self.dismissed.push(key);
            }
        }
        self.dismissed.len() - before
    }

    pub fn is_dismissed(&self, hash: &str, idx: i64) -> bool {
        self.dismissed.contains(&face_key(hash, idx))
    }

    pub fn dismissed_count(&self) -> usize {
        self.dismissed.len()
    }

    /// Offer every dismissed face for naming again, and say how many came back.
    pub fn restore_dismissed(&mut self) -> usize {
        std::mem::take(&mut self.dismissed).len()
    }

    /// Fold `from` into `into`: one person, holding everything both knew.
    ///
    /// The difference from forgetting one of them is the whole point. References are a
    /// set rather than a centroid (ADR-0003), so concatenating them strictly improves
    /// recognition — the same face front-on and in profile are both kept. Exclusions
    /// are unioned, because each was a direct correction and a merge must not quietly
    /// undo one.
    pub fn merge(&mut self, from: &str, into: &str) -> Result<usize> {
        let (from, into) = (from.trim(), into.trim());
        if from.eq_ignore_ascii_case(into) {
            anyhow::bail!("{from} is already {into}");
        }
        let Some(i) = self.people.iter().position(|p| p.name == from) else {
            anyhow::bail!("{from} is not someone this library knows");
        };
        if !self.people.iter().any(|p| p.name == into) {
            anyhow::bail!("{into} is not someone this library knows");
        }
        let gone = self.people.remove(i);
        let moved = gone.references.len();
        let target = self
            .people
            .iter_mut()
            .find(|p| p.name == into)
            .expect("checked above");
        target.references.extend(gone.references);
        for h in gone.excluded {
            if !target.excluded.contains(&h) {
                target.excluded.push(h);
            }
        }
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> People {
        let mut p = People::default();
        p.add_references("Sam", vec![vec![1.0, 0.0]]);
        p.add_references("Alex", vec![vec![0.0, 1.0]]);
        p
    }

    #[test]
    fn exclusion_is_per_person() {
        let mut p = people();
        p.exclude("Sam", &["hash-a".into()]);
        assert!(p.is_excluded("Sam", "hash-a"));
        // Removing Sam from a photo must not remove anyone else in it.
        assert!(!p.is_excluded("Alex", "hash-a"));
        assert!(!p.is_excluded("Sam", "hash-b"));
    }

    #[test]
    fn excluding_twice_does_not_duplicate() {
        let mut p = people();
        p.exclude("Sam", &["h".into()]);
        p.exclude("Sam", &["h".into()]);
        assert_eq!(p.get("Sam").unwrap().excluded.len(), 1);
    }

    #[test]
    fn excluding_an_unknown_person_is_a_no_op() {
        let mut p = people();
        p.exclude("Nobody", &["h".into()]);
        assert!(!p.is_excluded("Nobody", "h"));
    }

    /// people.json written before exclusions existed must still load.
    #[test]
    fn loads_a_file_without_the_excluded_field() {
        let json = r#"{"people":[{"name":"Sam","references":[[1.0,0.0]]}]}"#;
        let p: People = serde_json::from_str(json).expect("legacy people.json must parse");
        assert_eq!(p.people[0].name, "Sam");
        assert!(p.people[0].excluded.is_empty());
    }

    #[test]
    fn a_dismissal_is_keyed_to_the_face_not_the_cluster() {
        let mut p = people();
        p.dismiss(&[("photo-a".into(), 0), ("photo-a".into(), 1)]);
        assert!(p.is_dismissed("photo-a", 0));
        assert!(p.is_dismissed("photo-a", 1));
        // A different face in the same photograph is a different face.
        assert!(!p.is_dismissed("photo-a", 2));
        assert!(!p.is_dismissed("photo-b", 0));
        // Dismissing twice does not double-count, so the sidebar cannot claim more
        // dismissals than there are faces.
        assert_eq!(p.dismiss(&[("photo-a".into(), 0)]), 0);
        assert_eq!(p.dismissed_count(), 2);
    }

    #[test]
    fn dismissals_come_back_together_and_leave_nothing_behind() {
        let mut p = people();
        p.dismiss(&[("a".into(), 0), ("b".into(), 0)]);
        assert_eq!(p.restore_dismissed(), 2);
        assert_eq!(p.dismissed_count(), 0);
        assert!(!p.is_dismissed("a", 0));
        // Named people are untouched by any of it.
        assert_eq!(p.people.len(), 2);
    }

    fn cache_for(dir: &std::path::Path) -> std::path::PathBuf {
        dir.parent().unwrap().join(format!(
            "{}-cache",
            dir.file_name().unwrap().to_string_lossy()
        ))
    }

    #[test]
    fn a_dismissal_survives_the_disk() {
        let dir = std::env::temp_dir().join(format!("of-dismiss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = crate::Library::open_in(&dir, cache_for(&dir)).unwrap();
        let mut p = people();
        p.dismiss(&[("photo-a".into(), 3)]);
        lib.save_people(&p).unwrap();
        // The file is the one at the library root, not in the disposable cache.
        assert!(lib.people().unwrap().is_dismissed("photo-a", 3));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(cache_for(&dir)).ok();
    }

    #[test]
    fn merging_keeps_everything_both_people_knew() {
        let mut p = people();
        p.add_references("Sam", vec![vec![0.9, 0.1]]);
        p.exclude("Sam", &["not-sam".into()]);
        p.exclude("Alex", &["not-alex".into()]);
        let moved = p.merge("Alex", "Sam").unwrap();

        assert_eq!(moved, 1, "Alex's one reference moved");
        assert_eq!(p.people.len(), 1);
        let sam = p.get("Sam").unwrap();
        assert_eq!(sam.name, "Sam");
        // References are concatenated, never averaged (ADR-0003): merging must make
        // recognition better, which is the whole difference from forgetting.
        assert_eq!(sam.references.len(), 3);
        // Both corrections survive — a merge must not quietly undo one.
        assert!(p.is_excluded("Sam", "not-sam"));
        assert!(p.is_excluded("Sam", "not-alex"));
        assert!(p.get("Alex").is_none());
    }

    #[test]
    fn a_merge_that_would_lose_references_is_refused() {
        let mut p = people();
        // Into themselves: a no-op that would otherwise remove and re-add.
        assert!(p.merge("Sam", "Sam").is_err());
        assert!(p.merge("sam", "SAM").is_err());
        // Into or from a stranger: refuse rather than drop the references on the floor.
        assert!(p.merge("Sam", "Nobody").is_err());
        assert!(p.merge("Nobody", "Sam").is_err());
        assert_eq!(p.people.len(), 2, "a refused merge changes nothing");
        assert_eq!(p.get("Sam").unwrap().references.len(), 1);
    }

    #[test]
    fn names_are_stored_outside_the_disposable_cache() {
        let p = People::path(std::path::Path::new("/lib"));
        assert!(
            !p.to_string_lossy().contains(".blinkview"),
            "names cannot be recomputed and must survive deleting the cache"
        );
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("of-people-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = crate::Library::open_in(&dir, cache_for(&dir)).unwrap();
        let mut p = people();
        p.exclude("Sam", &["x".into()]);
        lib.save_people(&p).unwrap();
        let back = lib.people().unwrap();
        assert!(back.is_excluded("Sam", "x"));
        assert_eq!(back.people.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(cache_for(&dir)).ok();
    }

    /// The file names faces, not vectors. A reference the index holds becomes a
    /// pointer; only one with no face to name stays as bytes.
    #[test]
    fn the_file_points_at_the_faces_it_names() {
        use crate::faces::store::StoredFace;

        let dir = std::env::temp_dir().join(format!("of-people2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = crate::Library::open_in(&dir, cache_for(&dir)).unwrap();

        let mut known = vec![0.01f32; crate::faces::embed::DIM];
        known[0] = 1.0;
        lib.put_face(&StoredFace {
            hash: "photo-a".into(),
            idx: 4,
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
            score: 0.9,
            ratio: 0.2,
            embedding: Some(known.clone()),
        })
        .unwrap();

        let mut p = People::default();
        // One vector the index has, and one it does not — an old file whose cache
        // was deleted, or a face merged in from elsewhere.
        let orphan = vec![0.5f32; crate::faces::embed::DIM];
        p.add_references("Sam", vec![known.clone(), orphan.clone()]);
        lib.save_people(&p).unwrap();

        let raw = std::fs::read(People::path(&dir)).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("\"faces\""),
            "the pointer field is what gets written"
        );
        assert!(
            text.contains("photo-a:4"),
            "the known face is named, not copied"
        );
        assert!(
            raw.len() < 6 * 1024,
            "one pointer and one orphan cost {} bytes; vectors would be kilobytes each",
            raw.len()
        );

        let back = lib.people().unwrap();
        let refs = &back.get("Sam").unwrap().references;
        assert_eq!(refs.len(), 2, "both vectors come back");
        assert!(refs.iter().any(|v| v == &known));
        assert!(refs.iter().any(|v| v == &orphan));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(cache_for(&dir)).ok();
    }

    /// A v1 file — vectors, no pointers — whose cache is already gone must not lose
    /// the person it names when this version reads it.
    #[test]
    fn a_file_without_faces_keeps_its_vectors() {
        let dir = std::env::temp_dir().join(format!("of-people1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let v1 = r#"{"people":[{"name":"Sam","references":[[0.5,0.5]],"excluded":[]}]}"#;
        std::fs::write(dir.join("blinkview-people.json"), v1).unwrap();

        let lib = crate::Library::open_in(&dir, cache_for(&dir)).unwrap();
        let people = lib.people().unwrap();
        assert_eq!(people.people[0].name, "Sam");
        assert_eq!(people.people[0].references, vec![vec![0.5, 0.5]]);

        // And writing it back keeps the vector, because there is still nothing to
        // point at.
        lib.save_people(&people).unwrap();
        let text = std::fs::read_to_string(People::path(&dir)).unwrap();
        assert!(
            text.contains("\"references\""),
            "an unplaceable vector is kept, not dropped"
        );
        assert_eq!(
            lib.people().unwrap().people[0].references,
            vec![vec![0.5, 0.5]]
        );
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(cache_for(&dir)).ok();
    }
}
