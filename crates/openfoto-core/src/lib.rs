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

pub mod cluster;
pub mod dedupe;
pub mod edit;
pub mod faces;
pub mod fsops;
pub mod imageio;
pub mod imagesig;
pub mod index;
pub mod journal;
pub mod library;
pub mod plan;
pub mod progress;
pub mod rename;
pub mod scan;
pub mod scenery;
pub mod thumbs;
pub mod userdata;
pub mod timesource;

pub use library::Library;
pub use plan::{Op, Plan};
