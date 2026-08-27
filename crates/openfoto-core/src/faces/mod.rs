//! Face detection, alignment and recognition.

pub mod detect;
pub mod embed;
pub mod models;

pub use detect::{Detector, Face};
pub use embed::{cosine, Embedder};
