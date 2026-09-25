//! Core library for Kollate: reading Kobo data, normalizing it, and storing
//! and merging it into the local library. Contains no GTK code.

pub mod error;
pub mod import;
pub mod kobo;
pub mod normalize;
pub mod store;

pub use error::{Error, Result};
pub use import::ImportStats;
pub use store::{Library, default_library_path};
