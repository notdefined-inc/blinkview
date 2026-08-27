//! The plan/apply contract.
//!
//! Every mutating command produces a `Plan` from pure inspection, then optionally
//! applies it. A `Plan` writes nothing, so `--dry-run` is not a special code path that
//! can drift from the real one — it is simply declining to call `apply`.
//!
//! `Plan::validate` runs before any disk write and is what turns the failure modes
//! found during the manual session into refusals: name collisions, reserved characters,
//! missing destination directories.

use crate::{fsops, journal::Journal, Library};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Op {
    /// Move a file to a different folder, keeping its name.
    Move { hash: String, from: String, to: String },
    /// Rename a file in place.
    Rename { hash: String, from: String, to: String },
}

impl Op {
    pub fn from(&self) -> &str {
        match self {
            Op::Move { from, .. } | Op::Rename { from, .. } => from,
        }
    }
    pub fn to(&self) -> &str {
        match self {
            Op::Move { to, .. } | Op::Rename { to, .. } => to,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub label: String,
    pub ops: Vec<Op>,
    /// Things deliberately left alone, with the reason. Surfacing these is a feature:
    /// low-confidence items must never be moved silently, and the user needs to know
    /// what was skipped and why.
    pub skipped: Vec<(String, String)>,
}

impl Plan {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Reject a plan that cannot be applied safely. Called by `apply`, so an
    /// unvalidated plan cannot reach the disk.
    pub fn validate(&self, lib: &Library) -> Result<()> {
        let mut targets: HashSet<&str> = HashSet::new();
        let mut sources: HashSet<&str> = HashSet::new();

        for op in &self.ops {
            let (from, to) = (op.from(), op.to());
            if from == to {
                bail!("no-op in plan: {from}");
            }
            if !targets.insert(to) {
                bail!("two files would land on the same path: {to}");
            }
            if !sources.insert(from) {
                bail!("same source moved twice: {from}");
            }

            let name = to.rsplit('/').next().unwrap_or(to);
            fsops::validate_filename(name)?;

            let abs_from = lib.abs(from);
            if !abs_from.exists() {
                bail!("source missing: {from}");
            }
            let abs_to = lib.abs(to);
            // A destination that exists is only acceptable if that same file is itself
            // being moved away by this plan.
            if abs_to.exists() && !sources.contains(to) && !self.ops.iter().any(|o| o.from() == to) {
                bail!("destination already exists: {to}");
            }
            if let Some(parent) = abs_to.parent() {
                if !parent.is_dir() {
                    bail!(
                        "destination directory missing: {} (renamed or unmounted?)",
                        parent.display()
                    );
                }
            }
        }
        Ok(())
    }

    /// Apply the plan, returning a journal that can undo it.
    pub fn apply(self, lib: &mut Library) -> Result<Journal> {
        self.validate(lib)?;
        let mut done: Vec<Op> = Vec::with_capacity(self.ops.len());

        for op in &self.ops {
            let (from, to) = (op.from().to_string(), op.to().to_string());
            if let Err(e) = fsops::move_file(&lib.abs(&from), &lib.abs(&to)) {
                // Roll back what we already did, so a partial application never
                // survives. This is the failure the manual run hit mid-way.
                for prev in done.iter().rev() {
                    let _ = fsops::move_file(&lib.abs(prev.to()), &lib.abs(prev.from()));
                    let _ = lib.index.repath(prev.to(), prev.from());
                }
                return Err(e.context(format!("applying {}; rolled back {} ops", self.label, done.len())));
            }
            lib.index.repath(&from, &to)?;
            done.push(op.clone());
        }
        Journal::write(lib, &self.label, done)
    }
}
