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

/// Whether `path` is the root of a mounted Kobo.
pub fn is_kobo_mount(path: &Path) -> bool {
    path.join(DB_RELATIVE_PATH).is_file()
}

/// Contents of `.kobo/version`: `serial,?,firmware,?,?,model-id`.
/// Verified on a Libra Colour (model ID suffix `0390`), firmware 4.45.
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

    /// Marketing name for known model IDs.
    pub fn model_name(&self) -> &'static str {
        match self
            .model_id
            .as_deref()
            .and_then(|id| id.rsplit('-').next())
        {
            Some("000000000390") => "Kobo Libra Colour",
            _ => "Kobo",
        }
    }

    pub fn read(mount: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(mount.join(".kobo/version")).ok()?)
    }

    /// Identifies the device from `.kobo/version` when `path` is a mount
    /// point or the database inside one. A loose database file (e.g. a
    /// backup) is treated as its own device, keyed by its path.
    pub fn identify(path: &Path) -> Result<Self> {
        let mount = if path.is_dir() {
            Some(path)
        } else {
            path.parent().and_then(Path::parent)
        };
        if let Some(info) = mount.and_then(Self::read) {
            return Ok(info);
        }
        Ok(Self {
            serial: format!("file:{}", std::fs::canonicalize(path)?.display()),
            firmware: None,
            model_id: None,
        })
    }
}

/// Mounted Kobos under the usual removable-media roots for this user.
pub fn find_mounted_kobos() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let roots = [
        PathBuf::from("/media").join(&user),
        PathBuf::from("/run/media").join(&user),
    ];
    let mut found: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_kobo_mount(p))
        .collect();
    found.sort();
    found
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
        let real = DeviceInfo::parse(
            "N000000000000,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390",
        )
        .unwrap();
        assert_eq!(real.firmware.as_deref(), Some("4.45.23697"));
        assert_eq!(real.model_name(), "Kobo Libra Colour");
    }
}
