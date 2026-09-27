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

/// The image to show for a stylus markup: the page with the ink on it.
///
/// The Kobo keeps the two apart: the `.jpg` is the page *without* ink, and
/// the `.svg` holds only the ink strokes (same 1264×1680 page coordinates).
/// This writes `<id>.page.svg` beside the copied SVG, with the page embedded
/// as the background, and returns its path. It's rebuilt when missing or
/// older than its inputs. With only one of the two files, that file is
/// returned as is. `svg` and `jpg` are copies in the library, never on a Kobo.
pub fn markup_page(svg: Option<&Path>, jpg: Option<&Path>) -> Result<Option<PathBuf>> {
    let (svg, jpg) = match (svg.filter(|p| p.is_file()), jpg.filter(|p| p.is_file())) {
        (Some(svg), Some(jpg)) => (svg, jpg),
        (svg, jpg) => return Ok(svg.or(jpg).map(Path::to_path_buf)),
    };
    let stem = svg.file_stem().unwrap_or_default().to_string_lossy();
    let page = svg.with_file_name(format!("{stem}.page.svg"));
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let fresh = modified(&page).is_some_and(|t| Some(t) >= modified(svg).max(modified(jpg)));
    if !fresh {
        crate::kobo::ensure_not_on_kobo(&page)?;
        let ink = std::fs::read_to_string(svg)?;
        std::fs::write(&page, compose_markup(&ink, &std::fs::read(jpg)?))?;
    }
    Ok(Some(page))
}

/// `ink` (a Kobo markup SVG) with `jpeg` inserted as its first element, so
/// the strokes are drawn over the page.
fn compose_markup(ink: &str, jpeg: &[u8]) -> String {
    let image = format!(
        r#"<image x="0" y="0" width="100%" height="100%" preserveAspectRatio="none" xlink:href="data:image/jpeg;base64,{}"/>"#,
        base64(jpeg)
    );
    // Right after the root element's start tag.
    let at = ink
        .find("<svg")
        .and_then(|start| ink[start..].find('>').map(|end| start + end + 1));
    match at {
        Some(at) => format!("{}\n{image}{}", &ink[..at], &ink[at..]),
        None => ink.to_owned(),
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
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
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn puts_the_page_under_the_ink() {
        let ink = "<?xml version=\"1.0\"?>\n<svg width=\"1264\" height=\"1680\"\n viewBox=\"0 0 1264 1680\">\n<path d=\"M1,1\"/></svg>";
        let page = compose_markup(ink, b"jpg");
        let image = page.find("<image").unwrap();
        assert!(page.find("viewBox").unwrap() < image);
        assert!(image < page.find("<path").unwrap());
        assert!(page.contains("data:image/jpeg;base64,anBn"));
    }

    #[test]
    fn writes_the_markup_page_once() {
        let dir = tempfile::tempdir().unwrap();
        let (svg, jpg) = (dir.path().join("m.svg"), dir.path().join("m.jpg"));
        std::fs::write(&svg, "<svg viewBox=\"0 0 2 2\"><path d=\"M0,0\"/></svg>").unwrap();
        assert_eq!(
            markup_page(Some(&svg), Some(&jpg)).unwrap(),
            Some(svg.clone())
        );
        assert_eq!(markup_page(None, None).unwrap(), None);

        std::fs::write(&jpg, b"jpg").unwrap();
        let page = markup_page(Some(&svg), Some(&jpg)).unwrap().unwrap();
        assert_eq!(page, dir.path().join("m.page.svg"));
        assert!(
            std::fs::read_to_string(&page)
                .unwrap()
                .contains("base64,anBn")
        );
        let written = std::fs::metadata(&page).unwrap().modified().unwrap();
        markup_page(Some(&svg), Some(&jpg)).unwrap();
        assert_eq!(
            std::fs::metadata(&page).unwrap().modified().unwrap(),
            written
        );
    }

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
