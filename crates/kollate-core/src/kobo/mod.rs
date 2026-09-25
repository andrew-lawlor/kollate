//! Read-only access to a Kobo e-reader's data.

pub mod assets;
pub mod chapters;
pub mod device;
pub mod epub;
pub mod model;
pub mod reader;

pub use device::{DeviceInfo, find_kobo_db, find_mounted_kobos, is_kobo_mount};
pub use model::*;
pub use reader::KoboDb;
