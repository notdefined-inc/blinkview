//! Named identities and their reference faces.
//!
//! Lives at the **library root**, not in `.openfoto/`. Clustering is recomputable;
//! knowing a cluster is called "Nikhil" is not. Keeping the names inside a cache the
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
