//! Files that live beside the database on the device: cover images and the
//! page images of stylus markups. Copied (never moved) into the library.

use std::path::{Path, PathBuf};

use super::KoboSnapshot;
use super::qvariant::{MarkupGeometry, Rect};
use crate::Result;
use crate::markup::image::compose_page;

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
/// This draws them together into a JPEG beside the copied SVG and returns its
/// path: `<id>.page.jpg` for the whole page, or `<id>.page-<top>-<bottom>.jpg`
/// cut to `crop`. It's rebuilt when missing or older than its inputs. With
/// only one of the two files, that file is returned as is. `svg` and `jpg` are
/// copies in the library, never on a Kobo.
pub fn markup_page(
    svg: Option<&Path>,
    jpg: Option<&Path>,
    crop: Option<Rect>,
) -> Result<Option<PathBuf>> {
    let (svg, jpg) = match (svg.filter(|p| p.is_file()), jpg.filter(|p| p.is_file())) {
        (Some(svg), Some(jpg)) => (svg, jpg),
        (svg, jpg) => return Ok(svg.or(jpg).map(Path::to_path_buf)),
    };
    let stem = svg.file_stem().unwrap_or_default().to_string_lossy();
    let page = svg.with_file_name(match crop {
        Some(c) => format!("{stem}.page-{}-{}.jpg", c.top, c.bottom),
        None => format!("{stem}.page.jpg"),
    });
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let fresh = modified(&page).is_some_and(|t| Some(t) >= modified(svg).max(modified(jpg)));
    if !fresh {
        crate::kobo::ensure_not_on_kobo(&page)?;
        let ink = std::fs::read_to_string(svg)?;
        let crop = crop.map(|c| [c.left, c.top, c.right, c.bottom]);
        std::fs::write(&page, compose_page(&ink, &std::fs::read(jpg)?, crop)?)?;
        // Kollate 0.1.6 wrote these as SVGs, which some renderers misplaced.
        if let Some(dir) = svg.parent()
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{stem}.page")) && name.ends_with(".svg") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    Ok(Some(page))
}

#[derive(Debug, Default, Clone)]
pub struct CopiedAssets {
    /// (volume ID, copied cover).
    pub covers: Vec<(String, PathBuf)>,
    pub markups: Vec<CopiedMarkup>,
}

#[derive(Debug, Clone)]
pub struct CopiedMarkup {
    pub bookmark_id: String,
    pub svg: Option<PathBuf>,
    pub jpg: Option<PathBuf>,
    /// The part of the page worth showing, from `ExtraAnnotationData`.
    pub crop: Option<Rect>,
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
        out.markups.push(CopiedMarkup {
            bookmark_id: bm.bookmark_id.clone(),
            svg: copy(svg, "svg")?,
            jpg: copy(jpg, "jpg")?,
            crop: bm
                .extra_data
                .as_deref()
                .and_then(|d| MarkupGeometry::parse(d).crop()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_markup_page_once() {
        let dir = tempfile::tempdir().unwrap();
        let (svg, jpg) = (dir.path().join("m.svg"), dir.path().join("m.jpg"));
        std::fs::write(&svg, "<svg width=\"20\" height=\"10\" viewBox=\"0 0 20 10\"><path d=\"M0,0 L4,0 L4,4\"/></svg>").unwrap();
        assert_eq!(
            markup_page(Some(&svg), Some(&jpg), None).unwrap(),
            Some(svg.clone())
        );
        assert_eq!(markup_page(None, None, None).unwrap(), None);

        let mut page = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut page)
            .encode_image(&image::RgbImage::from_pixel(
                20,
                10,
                image::Rgb([255, 255, 255]),
            ))
            .unwrap();
        std::fs::write(&jpg, page).unwrap();
        // Left over from 0.1.6, which wrote SVGs.
        std::fs::write(dir.path().join("m.page.svg"), "old").unwrap();
        let whole = markup_page(Some(&svg), Some(&jpg), None).unwrap().unwrap();
        assert_eq!(whole, dir.path().join("m.page.jpg"));
        assert_eq!(
            image::open(&whole).unwrap().to_rgb8().dimensions(),
            (20, 10)
        );
        assert!(!dir.path().join("m.page.svg").exists());

        let crop = Rect {
            left: 0,
            top: 5,
            right: 19,
            bottom: 9,
        };
        let cut = markup_page(Some(&svg), Some(&jpg), Some(crop))
            .unwrap()
            .unwrap();
        assert_eq!(cut, dir.path().join("m.page-5-9.jpg"));
        assert_eq!(image::open(&cut).unwrap().to_rgb8().dimensions(), (20, 5));

        let written = std::fs::metadata(&whole).unwrap().modified().unwrap();
        markup_page(Some(&svg), Some(&jpg), None).unwrap();
        assert_eq!(
            std::fs::metadata(&whole).unwrap().modified().unwrap(),
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
        let markup = &out.markups[0];
        assert!(markup.svg.is_none());
        assert_eq!(markup.crop, None);
        assert_eq!(std::fs::read(markup.jpg.as_ref().unwrap()).unwrap(), b"jpg");
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
