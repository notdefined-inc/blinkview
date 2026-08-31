//! Splitting out photos with no close-up person.
//!
//! "No close-up people" is defined by the largest face as a fraction of image width.
//! Measured on the reference library (ADR-0003): under 4% is an incidental figure on a
//! staircase or down an alley; 4-7% is a *posed full-body portrait* and must not be
//! swept away. The default keeps posed shots where they are.
//!
//! Two limits are inherent and worth stating: sculptures and portraits-within-photos
//! register as faces, and a person facing away is not detected at all, so a prominent
//! back-turned subject lands in scenery regardless.

use crate::{
    plan::{Op, Plan},
    Library,
};
use anyhow::Result;

pub const DEFAULT_MAX_FACE: f32 = 0.04;
pub const DEFAULT_DEST: &str = "Scenery";

pub struct Options {
    pub max_face: f32,
    pub dest: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_face: DEFAULT_MAX_FACE,
            dest: DEFAULT_DEST.into(),
        }
    }
}

pub struct Split {
    pub scenery: Vec<(String, f32)>,
    pub people: usize,
    /// Photos never analysed for faces. Excluded rather than assumed empty.
    pub unanalysed: usize,
}

pub fn split(lib: &Library, opt: &Options) -> Result<Split> {
    let mut scenery = Vec::new();
    let (mut people, mut unanalysed) = (0, 0);
    for r in lib.index.all()? {
        if r.kind != "photo" {
            continue;
        }
        if !lib.faces_done(&r.hash)? {
            unanalysed += 1;
            continue;
        }
        let ratio = lib.max_face_ratio(&r.hash)?;
        if ratio < opt.max_face {
            scenery.push((r.path, ratio));
        } else {
            people += 1;
        }
    }
    Ok(Split {
        scenery,
        people,
        unanalysed,
    })
}

pub fn plan(lib: &Library, opt: &Options) -> Result<Plan> {
    let mut p = Plan::new("scenery");
    let s = split(lib, opt)?;
    for (path, _) in s.scenery {
        if path.starts_with(&format!("{}/", opt.dest)) {
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or(&path);
        let hash = lib
            .index
            .by_path(&path)?
            .map(|r| r.hash)
            .unwrap_or_default();
        p.ops.push(Op::Move {
            hash,
            from: path.clone(),
            to: format!("{}/{}", opt.dest, name),
        });
    }
    Ok(p)
}
