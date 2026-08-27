//! Face detection, alignment and recognition.

pub mod assign;
pub mod detect;
pub mod embed;
pub mod models;
pub mod people;
pub mod pipeline;
pub mod review;
pub mod store;

pub use detect::{Detector, Face};
pub use embed::{cosine, Embedder};
