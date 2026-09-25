//! Read-only access to a Kobo e-reader's data.

pub mod chapters;
pub mod device;
pub mod model;
pub mod reader;

pub use device::{DeviceInfo, find_kobo_db};
pub use model::*;
pub use reader::KoboDb;
