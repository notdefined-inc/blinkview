//! Applied-operation history. Every mutation is reversible (ADR-0001).

use crate::{fsops, plan::Op, Library};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Journal {
    pub id: String,
    pub label: String,
    pub applied_at: i64,
    pub ops: Vec<Op>,
}

/// A label reduced to something safe as a filename on any of our targets.
fn slug(label: &str) -> String {
    let out: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "op".into()
    } else {
        trimmed
    }
}

impl Journal {
    pub fn write(lib: &Library, label: &str, ops: Vec<Op>) -> Result<Self> {
        let now = chrono::Utc::now();
        let j = Journal {
            // The id becomes a filename, so it cannot carry anything a path would read
            // as structure. A label like "move 12 to Trip/Alps" would otherwise try to
            // write into a journal/Trip/ directory that does not exist.
            id: format!("{}-{}", now.format("%Y%m%dT%H%M%S"), slug(label)),
            label: label.to_string(),
            applied_at: now.timestamp(),
            ops,
        };
        let path = lib.journal_dir().join(format!("{}.json", j.id));
        std::fs::write(&path, serde_json::to_vec_pretty(&j)?)
            .with_context(|| format!("writing journal {}", path.display()))?;
        Ok(j)
    }

    /// Remove this journal entry, for when the operation it records is being undone
    /// as part of a failed apply.
    pub fn discard(&self, lib: &Library) -> Result<()> {
        let path = lib.journal_dir().join(format!("{}.json", self.id));
        std::fs::remove_file(path).ok();
        Ok(())
    }

    pub fn list(lib: &Library) -> Result<Vec<String>> {
        let mut out = vec![];
        for e in std::fs::read_dir(lib.journal_dir())? {
            let p = e?.path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                if let Some(s) = p.file_stem().and_then(|x| x.to_str()) {
                    out.push(s.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn load(lib: &Library, id: &str) -> Result<Self> {
        let path = lib.journal_dir().join(format!("{id}.json"));
        let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_slice(&data)?)
    }

    /// Reverse this operation. Applied in reverse order so chains undo cleanly.
    pub fn undo(&self, lib: &mut Library) -> Result<usize> {
        for op in self.ops.iter().rev() {
            if !lib.abs(op.to()).exists() {
                bail!("cannot undo: {} is no longer where we left it", op.to());
            }
        }
        // Metadata follows the file back, or undoing a move would strand a rating in
        // the folder the photograph no longer lives in. Computed before anything moves.
        let mut meta = crate::plan::relocations(&self.ops, lib, true)?;
        let mut n = 0;
        for op in self.ops.iter().rev() {
            fsops::move_file(&lib.abs(op.to()), &lib.abs(op.from()))?;
            lib.index.repath(op.to(), op.from())?;
            n += 1;
        }
        if let Some(set) = meta.as_mut() {
            set.save(lib.root())?;
        }
        let path = lib.journal_dir().join(format!("{}.json", self.id));
        std::fs::remove_file(path).ok();
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_cannot_smuggle_a_path_into_the_filename() {
        // "move 12 to Trip/Alps" once tried to write journal/Trip/Alps.json and failed
        // only *after* the files had already moved.
        assert_eq!(slug("move 12 to Trip/Alps"), "move-12-to-Trip-Alps");
        assert!(!slug("a/b").contains('/'));
        assert!(!slug("../../etc/passwd").contains('/'));
        assert!(!slug("../../etc/passwd").starts_with('.'));
    }

    #[test]
    fn a_label_with_nothing_usable_still_yields_a_filename() {
        assert_eq!(slug("///"), "op");
        assert_eq!(slug(""), "op");
    }
}
