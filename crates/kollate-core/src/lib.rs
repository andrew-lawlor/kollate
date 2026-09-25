//! Core library for Kollate: reading Kobo data, normalizing it, and (later)
//! storing, merging and exporting it. Contains no GTK code.

pub mod error;
pub mod kobo;
pub mod normalize;

pub use error::{Error, Result};
