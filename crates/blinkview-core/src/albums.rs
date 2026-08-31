//! Turning albums into folders.
//!
//! Transitional: albums were removed by ADR-0009, and this exists so libraries that
//! already have them are not simply stripped. Nothing writes albums any more.
//!
//! An album was a many-to-many label; a folder holds a file exactly once. That
//! mismatch is the whole difficulty, and it is handled by moving each photograph into
//! its first album and **reporting** the rest rather than guessing, silently dropping
//! them, or making copies.

use crate::plan::{Op, Plan};
use crate::userdata::UserDataSet;
use crate::Library;
use anyhow::Result;
use std::collections::BTreeMap;

/// A folder name that exFAT will accept, since album names were free text.
///
/// `Trip: Greece` becomes `Trip- Greece`. Returns `None` for a name with nothing
/// usable left, which must not become a folder called `""`.
pub fn folder_name(album: &str) -> Option<String> {
    let cleaned: String = album
        .chars()
        .map(|c| {
            if crate::fsops::RESERVED.contains(&c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    // A leading dot would hide the folder; a trailing dot or space is invalid on
    // several filesystems even where exFAT tolerates it.
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.')
        .trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// What migrating would do, without doing it.
#[derive(Debug, Default)]
pub struct Migration {
    pub plan: Plan,
    /// Album name as it will appear on disk, against the number of photographs moving.
    pub folders: BTreeMap<String, usize>,
    /// Album names changed to satisfy the filesystem: (original, on disk).
    pub renamed: Vec<(String, String)>,
}

/// Plan the move of every album member into a folder of that name.
///
/// A photograph belonging to several albums goes to the first alphabetically and the
/// others are recorded in `plan.skipped`, which the preview already surfaces. Guessing
/// which album "really" meant it is not something this can know.
pub fn plan(lib: &Library) -> Result<Migration> {
    let set = UserDataSet::load(lib.root())?;
    let mut folder_of_hash: BTreeMap<String, String> = BTreeMap::new();
    for r in lib.index.all()? {
        folder_of_hash.insert(r.hash.clone(), r.path);
    }

    let mut m = Migration {
        plan: Plan::new("albums to folders"),
        ..Default::default()
    };
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();

    for (album, _) in set.albums() {
        let Some(dest) = folder_name(&album) else {
            m.plan
                .skipped
                .push((album.clone(), "no usable folder name".into()));
            continue;
        };
        if dest != album {
            m.renamed.push((album.clone(), dest.clone()));
        }
        for (hash, path) in &folder_of_hash {
            let folder = crate::plan::folder_of(path);
            if !set.get(hash, folder).albums.iter().any(|a| a == &album) {
                continue;
            }
            if let Some(first) = claimed.get(hash) {
                m.plan.skipped.push((
                    path.clone(),
                    format!(
                        "also in {album:?}; a file lives in one folder, so it went to {first:?}"
                    ),
                ));
                continue;
            }
            let name = path.rsplit('/').next().unwrap_or(path);
            let to = format!("{dest}/{name}");
            if &to == path {
                continue; // already where it needs to be
            }
            claimed.insert(hash.clone(), dest.clone());
            *m.folders.entry(dest.clone()).or_default() += 1;
            m.plan.ops.push(Op::Move {
                hash: hash.clone(),
                from: path.clone(),
                to,
            });
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_characters_become_a_usable_folder_name() {
        assert_eq!(folder_name("Greece 2026").as_deref(), Some("Greece 2026"));
        assert_eq!(folder_name("Trip: Greece").as_deref(), Some("Trip- Greece"));
        assert_eq!(folder_name("a/b").as_deref(), Some("a-b"));
        // A leading dot would hide the folder in Finder.
        assert_eq!(folder_name(".hidden").as_deref(), Some("hidden"));
    }

    #[test]
    fn a_name_with_nothing_usable_is_refused() {
        // Better to report it than to create a folder called "-" or "".
        assert_eq!(folder_name("  "), None);
        assert_eq!(folder_name("..."), None);
    }
}
