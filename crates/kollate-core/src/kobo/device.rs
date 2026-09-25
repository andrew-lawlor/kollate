//! Locating and identifying a mounted Kobo.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub const DB_RELATIVE_PATH: &str = ".kobo/KoboReader.sqlite";

/// Accepts a mount point (containing `.kobo/KoboReader.sqlite`) or a direct
/// path to a `KoboReader.sqlite` file and returns the database path.
pub fn find_kobo_db(path: &Path) -> Result<PathBuf> {
    let candidate = if path.is_dir() {
        path.join(DB_RELATIVE_PATH)
    } else {
        path.to_path_buf()
    };
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(Error::NotAKobo(path.to_path_buf()))
    }
}

/// Contents of `.kobo/version`: `serial,?,firmware,?,?,model-id`.
/// Field layout to be verified against a real device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub serial: String,
    pub firmware: Option<String>,
    pub model_id: Option<String>,
}

impl DeviceInfo {
    pub fn parse(version_file: &str) -> Option<Self> {
        let fields: Vec<&str> = version_file.trim().split(',').map(str::trim).collect();
        let serial = fields.first().filter(|s| !s.is_empty())?.to_string();
        Some(Self {
            serial,
            firmware: fields.get(2).map(|s| s.to_string()),
            model_id: fields
                .last()
                .filter(|_| fields.len() > 1)
                .map(|s| s.to_string()),
        })
    }

    pub fn read(mount: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(mount.join(".kobo/version")).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_file() {
        let info = DeviceInfo::parse(
            "N418000000000,4.1.15,4.41.23145,4.1.15,4.1.15,00000000-0000-0000-0000-000000000393\n",
        )
        .unwrap();
        assert_eq!(info.serial, "N418000000000");
        assert_eq!(info.firmware.as_deref(), Some("4.41.23145"));
        assert_eq!(
            info.model_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000393")
        );
        assert!(DeviceInfo::parse("").is_none());
    }
}
