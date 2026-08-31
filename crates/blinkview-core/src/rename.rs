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

/// The counter token, written `%%n` in a pattern.
///
/// chrono turns `%%` into a literal `%`, so the pattern reaches the substitution as
/// `%n`. The doubling is not decoration: a bare `%n` is chrono's *newline*, which
/// would put a line break in a filename.
const COUNTER: &str = "%n";

/// Whether chrono can render this pattern.
///
/// Worth checking rather than discovering: a bad specifier panics when the format is
/// rendered, and this pattern is typed by a user, so the alternative is the window
/// falling over on a typo.
pub fn valid_format(format: &str) -> bool {
    !format.trim().is_empty()
        && !chrono::format::StrftimeItems::new(format)
            .any(|i| matches!(i, chrono::format::Item::Error))
}

/// Plan a rename of every indexed file to its capture timestamp.
pub fn plan(lib: &Library, format: &str) -> Result<Plan> {
    plan_scoped(lib, format, None)
}

/// Plan a rename of the files named by content hash, or of everything when `None`.
///
/// Uniqueness is still checked against **every** name in the library, never only the
/// scope: renaming twelve selected files into names another folder already uses is
/// exactly the silent-overwrite-on-merge this module's header exists to prevent.
pub fn plan_scoped(lib: &Library, format: &str, only: Option<&[String]>) -> Result<Plan> {
    if !valid_format(format) {
        anyhow::bail!("{format:?} is not a pattern I can read");
    }
    let mut p = Plan::new("rename");
    let rows = lib.index.all()?;

    // Every name in the library, so uniqueness is global (criterion 7).
    let mut taken: HashSet<String> = rows
        .iter()
        .filter_map(|r| r.path.rsplit('/').next().map(|s| s.to_string()))
        .collect();

    let scope: Option<HashSet<&str>> =
        only.map(|h| h.iter().map(|s| s.as_str()).collect());
    let mut work: Vec<&FileRow> = rows
        .iter()
        .filter(|r| scope.as_ref().is_none_or(|s| s.contains(r.hash.as_str())))
        .collect();

    // A counter only means anything in capture order — "1, 2, 3" following whatever
    // order the index happened to return would be a worse name than the original.
    let numbered = format.contains("%%n");
    if numbered {
        work.sort_by(|a, b| {
            let key = |r: &FileRow| (r.taken_at.unwrap_or(r.mtime), r.path.clone());
            key(a).cmp(&key(b))
        });
    }
    let width = work.len().to_string().len();

    for (i, row) in work.iter().enumerate() {
        let row = *row;
        let ext = ext_of(&row.path);
        let mut base = stem_for(row, format);
        if numbered {
            base = base.replace(COUNTER, &format!("{:0width$}", i + 1, width = width));
        }
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
        // A pattern can produce anything, including a slash, which would silently
        // become a folder. Report it rather than moving the file somewhere unasked.
        if let Err(e) = crate::fsops::validate_filename(&name) {
            p.skipped.push((row.path.clone(), e.to_string()));
            continue;
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

    /// A cache beside the fixture, so a unit test never writes to the machine's.
    fn cache_for(dir: &std::path::Path) -> std::path::PathBuf {
        dir.parent()
            .unwrap()
            .join(format!("{}-cache", dir.file_name().unwrap().to_string_lossy()))
    }

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

    /// A library of dummy files. Indexing is by extension, so the bytes need only
    /// differ; the timestamps come from the filenames, which is what makes the
    /// counter's order assertable.
    fn library(name: &str, files: &[&str]) -> (std::path::PathBuf, Library) {
        let d = std::env::temp_dir().join(format!("blinkview-rn-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (i, f) in files.iter().enumerate() {
            std::fs::write(d.join(f), format!("file {i}")).unwrap();
        }
        let mut lib = Library::open_in(&d, cache_for(&d)).unwrap();
        crate::scan::scan(&mut lib, false).unwrap();
        (d, lib)
    }

    #[test]
    fn a_scope_renames_only_what_it_names() {
        let (d, lib) = library("scope", &[
            "20260820_120101.jpg", "20260820_120102.jpg", "20260820_120103.jpg",
        ]);
        let rows = lib.index.all().unwrap();
        let one: Vec<String> = rows
            .iter()
            .filter(|r| r.path.ends_with("102.jpg"))
            .map(|r| r.hash.clone())
            .collect();
        assert_eq!(one.len(), 1);

        let scoped = plan_scoped(&lib, DEFAULT_FORMAT, Some(&one)).unwrap();
        assert_eq!(scoped.len(), 1, "only the named file may be renamed");
        assert!(scoped.ops[0].from().ends_with("102.jpg"));

        // The same call with no scope is the whole library, which is what `plan` is.
        assert_eq!(plan(&lib, DEFAULT_FORMAT).unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_counter_numbers_in_capture_order_and_pads_to_the_widest() {
        // Twelve, so the width is two and a naive counter would give 1..12 unpadded —
        // which sorts 10 before 2 in every file browser there is.
        let names: Vec<String> = (1..=12)
            .map(|i| format!("20260820_1201{i:02}.jpg"))
            .collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (d, lib) = library("counter", &refs);

        let p = plan_scoped(&lib, "shot_%%n", None).unwrap();
        assert_eq!(p.len(), 12);
        let mut got: Vec<(String, String)> = p
            .ops
            .iter()
            .map(|o| (o.from().to_string(), o.to().to_string()))
            .collect();
        got.sort();
        // Capture order is filename order here, so the first second gets the first
        // number and the padding makes them sort the way they were numbered.
        assert_eq!(got[0].1, "shot_01.jpg", "{got:?}");
        assert_eq!(got[11].1, "shot_12.jpg", "{got:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pattern_that_would_build_a_path_is_skipped_rather_than_obeyed() {
        let (d, lib) = library("slash", &["20260820_120101.jpg"]);
        // A slash in a pattern would quietly move the file into a new folder.
        let p = plan_scoped(&lib, "%Y/%m", None).unwrap();
        assert!(p.is_empty(), "nothing may be renamed");
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].1.contains("reserved"), "{:?}", p.skipped);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pattern_chrono_cannot_read_is_refused_before_it_panics() {
        assert!(valid_format(DEFAULT_FORMAT));
        assert!(valid_format("%Y-%m-%d_%%n"));
        assert!(!valid_format("%Q"));
        assert!(!valid_format(""));
        let (d, lib) = library("badfmt", &["20260820_120101.jpg"]);
        assert!(plan_scoped(&lib, "%Q", None).is_err());
        let _ = std::fs::remove_dir_all(&d);
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
