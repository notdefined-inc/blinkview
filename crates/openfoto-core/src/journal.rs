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

impl Journal {
    pub fn write(lib: &Library, label: &str, ops: Vec<Op>) -> Result<Self> {
        let now = chrono::Utc::now();
        let j = Journal {
            id: format!("{}-{}", now.format("%Y%m%dT%H%M%S"), label),
            label: label.to_string(),
            applied_at: now.timestamp(),
            ops,
        };
        let path = lib.journal_dir().join(format!("{}.json", j.id));
        std::fs::write(&path, serde_json::to_vec_pretty(&j)?)
            .with_context(|| format!("writing journal {}", path.display()))?;
        Ok(j)
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
        let mut n = 0;
        for op in self.ops.iter().rev() {
            fsops::move_file(&lib.abs(op.to()), &lib.abs(op.from()))?;
            lib.index.repath(op.to(), op.from())?;
            n += 1;
        }
        let path = lib.journal_dir().join(format!("{}.json", self.id));
        std::fs::remove_file(path).ok();
        Ok(n)
    }
}
