//! Named identities and their reference faces.
//!
//! Lives at the **library root**, not in `.openfoto/`. Clustering is recomputable;
//! knowing a cluster is called "Alex" is not. Keeping the names inside a cache the
//! documentation calls disposable would mean `rm -rf .openfoto` throws away work no
//! machine can reproduce. At the root it survives that, and travels with the folder.
//! See ADR-0007.

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

/// How one face is addressed in [`People::dismissed`].
fn face_key(hash: &str, idx: i64) -> String {
    format!("{hash}:{idx}")
}

impl People {
    /// The visible file at the library root.
    pub fn path(root: &Path) -> std::path::PathBuf {
        root.join("openfoto-people.json")
    }

    /// Where names used to live, inside the disposable cache.
    fn legacy_path(root: &Path) -> std::path::PathBuf {
        root.join(crate::library::VAULT_DIR).join("people.json")
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
        let _ = std::fs::remove_file(Self::legacy_path(root));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Person> {
        self.people.iter().find(|p| p.name == name)
    }

    /// True when the user has said this photo is not this person.
    pub fn is_excluded(&self, name: &str, photo_hash: &str) -> bool {
        self.get(name).is_some_and(|p| p.excluded.iter().any(|h| h == photo_hash))
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

    #[test]
    fn a_dismissal_survives_the_disk() {
        let dir = std::env::temp_dir().join(format!("of-dismiss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = people();
        p.dismiss(&[("photo-a".into(), 3)]);
        p.save(&dir).unwrap();
        // The file is the one at the library root, not in the disposable cache.
        assert!(People::load(&dir).unwrap().is_dismissed("photo-a", 3));
        std::fs::remove_dir_all(&dir).ok();
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
        assert!(!p.to_string_lossy().contains(".openfoto"),
            "names cannot be recomputed and must survive deleting the cache");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("of-people-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = people();
        p.exclude("Sam", &["x".into()]);
        p.save(&dir).unwrap();
        let back = People::load(&dir).unwrap();
        assert!(back.is_excluded("Sam", "x"));
        assert_eq!(back.people.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
