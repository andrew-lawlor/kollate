//! Files that live beside the database on the device: cover images and the
//! page images of stylus markups. Copied (never moved) into the library.

use std::path::{Path, PathBuf};

use super::KoboSnapshot;
use crate::Result;

/// Kobo's `qhash` of an `ImageId`, used to pick the `.kobo-images/<a>/<b>/`
/// directory (same algorithm as calibre's KoboTouch driver).
fn qhash(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = (h << 4).wrapping_add(b as u32);
        h ^= (h & 0xf000_0000) >> 23;
        h &= 0x0fff_ffff;
    }
    h
}

/// The best available cover image for `image_id` on a mounted Kobo.
pub fn cover_path(mount: &Path, image_id: &str) -> Option<PathBuf> {
    let h = qhash(image_id);
    let dir = mount
        .join(".kobo-images")
        .join((h & 0xff).to_string())
        .join(((h & 0xff00) >> 8).to_string());
    ["N3_LIBRARY_FULL", "N3_FULL", "N3_LIBRARY_GRID"]
        .iter()
        .map(|kind| dir.join(format!("{image_id} - {kind}.parsed")))
        .find(|p| p.is_file())
}

/// A markup's ink-only SVG and rendered page JPG, if present.
pub fn markup_paths(mount: &Path, bookmark_id: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    let base = mount.join(".kobo/markups");
    let existing =
        |ext: &str| Some(base.join(format!("{bookmark_id}.{ext}"))).filter(|p| p.is_file());
    (existing("svg"), existing("jpg"))
}

#[derive(Debug, Default, Clone)]
pub struct CopiedAssets {
    /// (volume ID, copied cover).
    pub covers: Vec<(String, PathBuf)>,
    /// (bookmark ID, copied SVG, copied JPG).
    pub markups: Vec<(String, Option<PathBuf>, Option<PathBuf>)>,
}

/// Copies `src` to `dest` unless an identical-size copy is already there.
fn copy_if_changed(src: &Path, dest: &Path) -> Result<()> {
    let same = match (std::fs::metadata(src), std::fs::metadata(dest)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if !same {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = dest.with_extension("part");
        std::fs::copy(src, &tmp)?;
        std::fs::rename(&tmp, dest)?;
    }
    Ok(())
}

/// Copies covers of the snapshot's books and all markup images into
/// `assets_dir` (`covers/` and `markups/`). Safe to re-run: unchanged files
/// are skipped, so an interrupted copy resumes on the next connect.
pub fn copy_assets(
    mount: &Path,
    snapshot: &KoboSnapshot,
    assets_dir: &Path,
) -> Result<CopiedAssets> {
    super::ensure_not_on_kobo(assets_dir)?;
    let mut out = CopiedAssets::default();
    for book in &snapshot.books {
        let Some(image_id) = &book.image_id else {
            continue;
        };
        let Some(src) = cover_path(mount, image_id) else {
            continue;
        };
        let name = blake3::hash(image_id.as_bytes()).to_hex()[..24].to_owned();
        let dest = assets_dir.join("covers").join(format!("{name}.jpg"));
        copy_if_changed(&src, &dest)?;
        out.covers.push((book.volume_id.clone(), dest));
    }
    for bm in &snapshot.bookmarks {
        let (svg, jpg) = markup_paths(mount, &bm.bookmark_id);
        if svg.is_none() && jpg.is_none() {
            continue;
        }
        let copy = |src: Option<PathBuf>, ext: &str| -> Result<Option<PathBuf>> {
            let Some(src) = src else { return Ok(None) };
            let dest = assets_dir
                .join("markups")
                .join(format!("{}.{ext}", bm.bookmark_id));
            copy_if_changed(&src, &dest)?;
            Ok(Some(dest))
        };
        out.markups
            .push((bm.bookmark_id.clone(), copy(svg, "svg")?, copy(jpg, "jpg")?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qhash_matches_device_layout() {
        // Verified against .kobo-images on a Libra Colour.
        let h =
            qhash("file____mnt_onboard_Neil_Price_Children_of_Ash_and_Elm_-_Neil_Price_kepub_epub");
        assert_eq!((h & 0xff, (h & 0xff00) >> 8), (50, 200));
    }

    #[test]
    fn copies_markups_and_skips_unchanged() {
        let mount = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(mount.path().join(".kobo/markups")).unwrap();
        std::fs::write(mount.path().join(".kobo/markups/abc.jpg"), b"jpg").unwrap();
        let mut snap = KoboSnapshot::default();
        snap.bookmarks.push(crate::kobo::KoboBookmark {
            bookmark_id: "abc".into(),
            volume_id: "v".into(),
            content_id: "c".into(),
            kind: crate::kobo::AnnotationKind::Markup,
            text: None,
            note: None,
            color: 0,
            start: crate::kobo::Position {
                container_path: String::new(),
                child_index: 0,
                offset: 0,
            },
            end: crate::kobo::Position {
                container_path: String::new(),
                child_index: 0,
                offset: 0,
            },
            chapter_progress: 0.0,
            chapter_title: None,
            spine_index: None,
            created: None,
            modified: None,
            extra_data: None,
        });
        let out = copy_assets(mount.path(), &snap, dest.path()).unwrap();
        let (_, svg, jpg) = &out.markups[0];
        assert!(svg.is_none());
        assert_eq!(std::fs::read(jpg.as_ref().unwrap()).unwrap(), b"jpg");
        // Second run is a no-op but still reports the file.
        assert_eq!(
            copy_assets(mount.path(), &snap, dest.path())
                .unwrap()
                .markups
                .len(),
            1
        );
    }
}
