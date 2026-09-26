//! In the Flatpak, files and folders picked in a file chooser arrive as
//! document-portal paths (`/run/user/1000/doc/<id>/<name>/…`). This maps them
//! back to their real location, for display and for the never-write-to-a-Kobo
//! check.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;

/// Splits a document-portal path into the document ID and whatever follows
/// the exported file or folder itself.
fn split_doc_path(path: &Path, doc_root: &Path) -> Option<(String, PathBuf)> {
    let rel = path.strip_prefix(doc_root).ok()?;
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.first().map(String::as_str) == Some("by-app") {
        parts.drain(..parts.len().min(2)); // by-app/<app id>/
    }
    let id = parts.first()?.clone();
    parts.get(1)?; // the exported file or folder's own name
    Some((id, parts[2..].iter().collect()))
}

/// The real location of a document-portal path, or `None` for ordinary paths
/// (or if the portal can't be reached).
pub fn host_path(path: &Path) -> Option<PathBuf> {
    let (id, rest) = split_doc_path(path, &glib::user_runtime_dir().join("doc"))?;
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let reply = bus
        .call_sync(
            Some("org.freedesktop.portal.Documents"),
            "/org/freedesktop/portal/documents",
            "org.freedesktop.portal.Documents",
            "GetHostPaths",
            Some(&(vec![id.clone()],).to_variant()),
            Some(glib::VariantTy::new("(a{say})").expect("valid type")),
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let paths: HashMap<String, Vec<u8>> = reply.child_value(0).get()?;
    let mut bytes = paths.get(&id)?.clone();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    (!bytes.is_empty()).then(|| PathBuf::from(OsString::from_vec(bytes)).join(rest))
}

/// The path to act on for safety checks: the real location when known.
pub fn real_path(path: &Path) -> PathBuf {
    host_path(path).unwrap_or_else(|| path.to_path_buf())
}

/// A path as a person would recognise it: the real location, with the home
/// folder shortened to `~`.
pub fn display_path(path: &Path) -> String {
    let real = real_path(path);
    match real.strip_prefix(glib::home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => real.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_document_portal_paths() {
        let root = Path::new("/run/user/1000/doc");
        assert_eq!(
            split_doc_path(Path::new("/run/user/1000/doc/c8fdd094/Reading"), root),
            Some(("c8fdd094".into(), PathBuf::new()))
        );
        assert_eq!(
            split_doc_path(
                Path::new("/run/user/1000/doc/c8fdd094/Reading/Books/x.md"),
                root
            ),
            Some(("c8fdd094".into(), PathBuf::from("Books/x.md")))
        );
        assert_eq!(
            split_doc_path(
                Path::new("/run/user/1000/doc/by-app/io.github.x/ab12/export.csv"),
                root
            ),
            Some(("ab12".into(), PathBuf::new()))
        );
        assert_eq!(split_doc_path(Path::new("/home/me/Obsidian"), root), None);
        assert_eq!(
            split_doc_path(Path::new("/run/user/1000/doc/c8fdd094"), root),
            None
        );
    }
}
