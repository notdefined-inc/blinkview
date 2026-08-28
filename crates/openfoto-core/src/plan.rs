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
    ///
    /// Metadata moves with the file. Since ADR-0010 a rating lives in the
    /// `openfoto.json` of the folder holding the photograph, so a move that did not
    /// migrate the entry would look exactly like the rating being lost.
    ///
    /// The order is deliberate: **files, then the journal, then metadata**, and any
    /// failure rolls the files back. An operation that cannot be recorded must not
    /// stand — the whole contract of this app is that every mutation is reversible, and
    /// a move with no journal entry is a move nobody can undo.
    pub fn apply(self, lib: &mut Library) -> Result<Journal> {
        self.validate(lib)?;
        let mut meta = relocations(&self.ops, lib, false)?;
        let mut done: Vec<Op> = Vec::with_capacity(self.ops.len());

        for op in &self.ops {
            let (from, to) = (op.from().to_string(), op.to().to_string());
            if let Err(e) = fsops::move_file(&lib.abs(&from), &lib.abs(&to)) {
                // Roll back what we already did, so a partial application never
                // survives. This is the failure the manual run hit mid-way.
                rollback(lib, &done);
                return Err(e.context(format!(
                    "applying {}; rolled back {} ops",
                    self.label,
                    done.len()
                )));
            }
            lib.index.repath(&from, &to)?;
            done.push(op.clone());
        }

        let journal = match Journal::write(lib, &self.label, done.clone()) {
            Ok(j) => j,
            Err(e) => {
                rollback(lib, &done);
                return Err(e.context(format!(
                    "applying {}: the change could not be recorded, so it was undone \
                     rather than left with no way back",
                    self.label
                )));
            }
        };

        if let Some(set) = meta.as_mut() {
            if let Err(e) = set.save(lib.root()) {
                let _ = journal.discard(lib);
                rollback(lib, &done);
                return Err(e.context(format!(
                    "applying {}: files moved but their ratings could not follow, so \
                     nothing was applied",
                    self.label
                )));
            }
        }
        Ok(journal)
    }
}

/// Put every completed move back, ignoring errors: this runs on a path that is already
/// failing, and reporting a second failure would bury the first.
fn rollback(lib: &mut Library, done: &[Op]) {
    for prev in done.iter().rev() {
        let _ = fsops::move_file(&lib.abs(prev.to()), &lib.abs(prev.from()));
        let _ = lib.index.repath(prev.to(), prev.from());
    }
}

/// Plan moving a chosen set of photographs into `dest`.
///
/// The one primitive the command layer needed that did not exist: every other plan
/// builder decides for itself which photographs it acts on, and a typed instruction
/// has already decided.
///
/// A photograph already in `dest` is not an op, and one whose name is taken there is
/// skipped with a reason rather than overwriting — `Plan::validate` would refuse the
/// whole plan otherwise, which would be a worse answer than moving the rest.
pub fn move_into(lib: &Library, hashes: &[String], dest: &str) -> Result<Plan> {
    let dest = dest.trim().trim_matches('/');
    if dest.is_empty() {
        anyhow::bail!("no destination folder given");
    }
    if let Some(c) = dest.split('/').find_map(|seg| {
        seg.chars().find(|c| crate::fsops::RESERVED.contains(c))
    }) {
        anyhow::bail!("folder name contains a reserved character {c:?}");
    }

    let rows = lib.index.all()?;
    let taken: std::collections::HashSet<String> = rows
        .iter()
        .filter(|r| folder_of(&r.path) == dest)
        .map(|r| r.path.rsplit('/').next().unwrap_or(&r.path).to_string())
        .collect();

    let mut plan = Plan::new(format!("move {} to {dest}", hashes.len()));
    let wanted: std::collections::HashSet<&String> = hashes.iter().collect();
    for r in rows.iter().filter(|r| wanted.contains(&r.hash)) {
        let name = r.path.rsplit('/').next().unwrap_or(&r.path);
        if folder_of(&r.path) == dest {
            continue;
        }
        if taken.contains(name) {
            plan.skipped.push((
                r.path.clone(),
                format!("{dest}/{name} already exists"),
            ));
            continue;
        }
        plan.ops.push(Op::Move {
            hash: r.hash.clone(),
            from: r.path.clone(),
            to: format!("{dest}/{name}"),
        });
    }
    Ok(plan)
}

/// The folder part of a library-relative path; `""` for the library root.
pub fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Move each photograph's metadata to follow its file, in memory.
///
/// `reverse` swaps the direction, for undo. Returns `None` when no op changes folder,
/// so the common case — a rename in place — does not walk the tree.
pub(crate) fn relocations(
    ops: &[Op],
    lib: &Library,
    reverse: bool,
) -> Result<Option<crate::userdata::UserDataSet>> {
    let crossing: Vec<(&str, &str, &str)> = ops
        .iter()
        .filter_map(|op| {
            let (hash, from, to) = match op {
                Op::Move { hash, from, to } | Op::Rename { hash, from, to } => (hash, from, to),
            };
            let (a, b) = if reverse { (to, from) } else { (from, to) };
            let (a, b) = (folder_of(a), folder_of(b));
            (a != b).then_some((hash.as_str(), a, b))
        })
        .collect();
    if crossing.is_empty() {
        return Ok(None);
    }
    let mut set = crate::userdata::UserDataSet::load(lib.root())?;
    for (hash, from, to) in crossing {
        set.relocate(hash, from, to);
    }
    Ok(Some(set))
}
