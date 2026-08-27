//! Date-time filenames.
//!
//! Two failures from the manual session are load-bearing here:
//!
//! * **Seconds are not optional.** At minute resolution 81% of the reference library
//!   collided; with seconds, 9%. A counter would otherwise be doing the timestamp's job.
//! * **Uniqueness is library-wide, not per-folder.** Per-folder uniqueness left 65 names
//!   duplicated across folders (130 files) that would silently overwrite on any merge.
//!
//! The default format uses hyphens because the reference drive is exFAT, where `:`
//! is reserved (see `fsops::RESERVED`).

use crate::{index::FileRow, plan::{Op, Plan}, Library};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use std::collections::HashSet;

pub const DEFAULT_FORMAT: &str = "%I-%M-%S_%p_%d_%b_%Y";

/// Split `stem` into (base, counter) where the counter is a *trailing* `_N`.
///
/// Guarding this is not academic: a naive non-greedy regex read `..._19_aug_2026` as
/// base `..._19_aug` plus counter `2026`, which would have silently destroyed the year
/// on 65 files. Only a suffix that is not part of the timestamp counts.
fn split_counter(stem: &str) -> (&str, Option<u32>) {
    let Some(idx) = stem.rfind('_') else { return (stem, None) };
    let (base, tail) = stem.split_at(idx);
    let tail = &tail[1..];
    match tail.parse::<u32>() {
        // A 4-digit run is a year, not a counter.
        Ok(n) if tail.len() < 4 => (base, Some(n)),
        _ => (stem, None),
    }
}

fn stem_for(row: &FileRow, format: &str) -> String {
    let ts = row.taken_at.unwrap_or(row.mtime);
    Utc.timestamp_opt(ts, 0)
        .single()
        .unwrap_or_default()
        .format(format)
        .to_string()
        .to_lowercase()
}

fn ext_of(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("jpg").to_ascii_lowercase()
}

fn dir_of(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
}

/// Plan a rename of every indexed file to its capture timestamp.
pub fn plan(lib: &Library, format: &str) -> Result<Plan> {
    let mut p = Plan::new("rename");
    let rows = lib.index.all()?;

    // Every name in the library, so uniqueness is global (criterion 7).
    let mut taken: HashSet<String> = rows
        .iter()
        .filter_map(|r| r.path.rsplit('/').next().map(|s| s.to_string()))
        .collect();

    for row in &rows {
        let ext = ext_of(&row.path);
        let base = stem_for(row, format);
        let current = row.path.rsplit('/').next().unwrap_or(&row.path).to_string();

        // Already correctly named? Leave it alone so re-runs are idempotent.
        let (cur_base, _) = split_counter(current.trim_end_matches(&format!(".{ext}")));
        if cur_base == base {
            continue;
        }

        let mut name = format!("{base}.{ext}");
        let mut n = 2u32;
        while taken.contains(&name) {
            name = format!("{base}_{n}.{ext}");
            n += 1;
        }
        taken.remove(&current);
        taken.insert(name.clone());

        let to = match dir_of(&row.path) {
            Some(d) => format!("{d}/{name}"),
            None => name,
        };
        p.ops.push(Op::Rename { hash: row.hash.clone(), from: row.path.clone(), to });
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_mistake_a_year_for_a_counter() {
        // The exact bug the dry run caught.
        assert_eq!(split_counter("01-13-51_pm_19_aug_2026"), ("01-13-51_pm_19_aug_2026", None));
    }

    #[test]
    fn finds_a_real_counter() {
        assert_eq!(split_counter("01-13-51_pm_19_aug_2026_2"), ("01-13-51_pm_19_aug_2026", Some(2)));
        assert_eq!(split_counter("05-16-27_pm_18_aug_2026_12"), ("05-16-27_pm_18_aug_2026", Some(12)));
    }

    #[test]
    fn default_format_is_exfat_safe() {
        let row = FileRow {
            hash: "h".into(), path: "a/20260820_120132.jpg".into(),
            size: 1, mtime: 0, kind: "photo".into(),
            taken_at: Some(1786968092), taken_src: Some("filename".into()),
        };
        let name = stem_for(&row, DEFAULT_FORMAT);
        assert!(crate::fsops::validate_filename(&format!("{name}.jpg")).is_ok());
        assert!(!name.contains(':'));
    }
}
