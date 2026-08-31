//! Filing photos into per-person folders.
//!
//! Two rules, both learned the hard way:
//!
//! * A photo containing two different known people belongs to neither folder. It stays
//!   where it is and is reported.
//! * A face that is not confidently anyone is never a reason to move a photo.

use crate::{
    faces::{assign, people::People},
    plan::{Op, Plan},
    Library,
};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

pub struct Outcome {
    pub plan: Plan,
    /// Photos containing more than one identified person, deliberately left alone.
    pub shared: Vec<String>,
    /// Photos whose faces matched nobody confidently.
    pub unclaimed: usize,
}

pub fn plan(lib: &Library, people: &People, opt: &assign::Options) -> Result<Outcome> {
    let hash_to_path: BTreeMap<String, String> = lib
        .index
        .all()?
        .into_iter()
        .map(|r| (r.hash, r.path))
        .collect();

    let mut by_photo: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in lib.all_faces()? {
        let Some(e) = f.embedding.as_ref() else {
            continue;
        };
        if let Some(name) = assign::assign(e, people, opt).person() {
            // An explicit "not this person" always wins over a match.
            if people.is_excluded(name, &f.hash) {
                continue;
            }
            by_photo
                .entry(f.hash.clone())
                .or_default()
                .insert(name.to_string());
        }
    }

    let mut plan = Plan::new("faces-file");
    let mut shared = Vec::new();
    let mut unclaimed = 0;
    for (hash, path) in &hash_to_path {
        match by_photo.get(hash) {
            None => unclaimed += 1,
            Some(names) if names.len() > 1 => {
                shared.push(path.clone());
                plan.skipped.push((
                    path.clone(),
                    format!(
                        "contains {}",
                        names.iter().cloned().collect::<Vec<_>>().join(" and ")
                    ),
                ));
            }
            Some(names) => {
                let name = names.iter().next().expect("non-empty");
                if path.starts_with(&format!("{name}/")) {
                    continue;
                }
                let file = path.rsplit('/').next().unwrap_or(path);
                plan.ops.push(Op::Move {
                    hash: hash.clone(),
                    from: path.clone(),
                    to: format!("{name}/{file}"),
                });
            }
        }
    }
    Ok(Outcome {
        plan,
        shared,
        unclaimed,
    })
}
