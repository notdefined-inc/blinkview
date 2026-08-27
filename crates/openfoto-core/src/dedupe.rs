//! Finding burst near-duplicates.
//!
//! Pipeline, and why each stage is there (ADR-0003):
//!   1. dHash Hamming <= 12 generates candidates. Cheap, and wrong on its own.
//!   2. Contrast-normalized RMSE <= threshold confirms them against actual pixels.
//!   3. Complete-linkage grouping prevents A~B~C chaining into a false cluster.
//!   4. The sharpest frame stays; the rest are planned into `Duplicates/`.
//!
//! Default RMSE is 0.30. Measured on the reference library: 0.20 -> 134 photos moved,
//! 0.30 -> 284, 0.45 -> 549. At 0.45 visibly different alternate takes were swept up.

use crate::{
    cluster, imagesig,
    index::FileRow,
    plan::{Op, Plan},
    Library,
};
use anyhow::Result;
use rayon::prelude::*;

pub const DEFAULT_RMSE: f32 = 0.30;
pub const DEFAULT_HAMMING: u32 = 12;
pub const DEFAULT_DEST: &str = "Duplicates";

pub struct Options {
    pub rmse: f32,
    pub hamming: u32,
    pub dest: String,
}

impl Default for Options {
    fn default() -> Self {
        Self { rmse: DEFAULT_RMSE, hamming: DEFAULT_HAMMING, dest: DEFAULT_DEST.into() }
    }
}

/// Compute and cache signatures for every photo missing one. Parallel; decoding is
/// the bottleneck, not the comparison.
pub fn ensure_signatures(lib: &Library) -> Result<usize> {
    let rows: Vec<FileRow> = lib.index.all()?.into_iter().filter(|r| r.kind == "photo").collect();

    // Resolve everything the workers need *before* going parallel: the SQLite
    // connection is not Sync, so no index access may cross the rayon boundary.
    let mut todo: Vec<(String, std::path::PathBuf)> = Vec::new();
    for r in &rows {
        if lib.index.get_signature(&r.hash)?.is_none() {
            todo.push((r.hash.clone(), lib.abs(&r.path)));
        }
    }

    let computed: Vec<(String, imagesig::Signature)> = todo
        .par_iter()
        .filter_map(|(hash, path)| imagesig::compute(path).ok().map(|s| (hash.clone(), s)))
        .collect();

    let n = computed.len();
    for (hash, sig) in computed {
        lib.index.put_signature(&hash, &sig)?;
    }
    Ok(n)
}

#[derive(Debug, Clone)]
pub struct Group {
    pub keep: FileRow,
    pub duplicates: Vec<FileRow>,
}

/// Group near-duplicates. Returns groups of two or more, largest first.
pub fn find_groups(lib: &Library, opt: &Options) -> Result<Vec<Group>> {
    let rows: Vec<FileRow> = lib.index.all()?.into_iter().filter(|r| r.kind == "photo").collect();
    let mut sigs = Vec::with_capacity(rows.len());
    let mut keep_rows = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(s) = lib.index.get_signature(&r.hash)? {
            sigs.push(s);
            keep_rows.push(r);
        }
    }
    let n = sigs.len();

    // Stage 1+2: candidates by dHash, confirmed by pixels.
    let pairs: Vec<(f32, usize, usize)> = (0..n)
        .into_par_iter()
        .flat_map(|i| {
            let sigs = &sigs;
            ((i + 1)..n)
                .filter_map(move |j| {
                    if imagesig::hamming(sigs[i].dhash, sigs[j].dhash) > opt.hamming {
                        return None;
                    }
                    let d = imagesig::rmse(&sigs[i].thumb, &sigs[j].thumb);
                    (d <= opt.rmse).then_some((d, i, j))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Stage 3: complete-linkage, so no group is held together by transitivity.
    let close = |a: usize, b: usize| imagesig::rmse(&sigs[a].thumb, &sigs[b].thumb) <= opt.rmse;
    let groups = cluster::complete_linkage(n, pairs, close);

    // Stage 4: the sharpest frame stays.
    Ok(groups
        .into_iter()
        .map(|g| {
            let best = *g
                .iter()
                .max_by(|&&a, &&b| {
                    sigs[a].sharpness.partial_cmp(&sigs[b].sharpness).unwrap_or(std::cmp::Ordering::Equal)
                })
                .expect("groups are non-empty");
            Group {
                keep: keep_rows[best].clone(),
                duplicates: g.iter().filter(|&&i| i != best).map(|&i| keep_rows[i].clone()).collect(),
            }
        })
        .collect())
}

pub fn plan(lib: &Library, opt: &Options) -> Result<Plan> {
    let mut p = Plan::new("dedupe");
    for g in find_groups(lib, opt)? {
        for d in g.duplicates {
            let name = d.path.rsplit('/').next().unwrap_or(&d.path);
            p.ops.push(Op::Move {
                hash: d.hash.clone(),
                from: d.path.clone(),
                to: format!("{}/{}", opt.dest, name),
            });
        }
    }
    Ok(p)
}
