//! Read-only access to a Kobo e-reader's data.

pub mod assets;
pub mod chapters;
pub mod device;
pub mod epub;
pub mod model;
pub mod reader;

pub use device::{
    DeviceInfo, ensure_not_on_kobo, find_kobo_db, find_mounted_kobos, is_kobo_mount,
    kobo_root_containing,
};
pub use model::*;
pub use reader::KoboDb;
