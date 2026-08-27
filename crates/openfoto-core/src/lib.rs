//! openfoto-core — all logic for the openfoto photo organizer.
//!
//! Invariants this crate exists to uphold (see ../../docs/DECISIONS/):
//!
//! * The photo folders on disk are the only source of truth. `.openfoto/` is derived
//!   and must always be safe to delete (ADR-0001).
//! * A file is identified by its BLAKE3 content hash, never by its path. Users move
//!   and rename things in Finder while we run; that is normal (ADR-0001).
//! * Nothing touches the disk except through `Plan` -> `apply` -> `Journal`, so every
//!   mutation is previewable and reversible.

/// A previewed set of changes. Produced by pure planning functions; holds no open
/// handles and writes nothing. `--dry-run` prints one of these and stops.
#[derive(Debug, Default)]
pub struct Plan {
    pub ops: Vec<Op>,
}

/// A single reversible change to the library.
#[derive(Debug)]
pub enum Op {
    Move { hash: String, from: String, to: String },
    Rename { hash: String, from: String, to: String },
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }
}
