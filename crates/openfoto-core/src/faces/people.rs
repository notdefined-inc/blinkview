//! Named identities and their reference faces.
//!
//! Lives in `.openfoto/people.json`. It is the one part of the vault that is not
//! recomputable — a machine cannot know a cluster is called "Nikhil". It is small,
//! plain JSON, and re-derivable in a couple of minutes through `faces review`, which
//! is the tradeoff ADR-0001 accepts.

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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct People {
    #[serde(default)]
    pub people: Vec<Person>,
}

impl People {
    pub fn path(vault: &Path) -> std::path::PathBuf {
        vault.join("people.json")
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

    pub fn get(&self, name: &str) -> Option<&Person> {
        self.people.iter().find(|p| p.name == name)
    }

    pub fn add_references(&mut self, name: &str, refs: Vec<Vec<f32>>) {
        match self.people.iter_mut().find(|p| p.name == name) {
            Some(p) => p.references.extend(refs),
            None => self.people.push(Person { name: name.to_string(), references: refs }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty()
    }
}
