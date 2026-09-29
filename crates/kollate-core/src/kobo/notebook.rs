//! Kobo notebooks (stylus models): the `.nebo` files in `My Notebooks/`.
//!
//! A `.nebo` is a zip written by MyScript's iink engine: a `meta.json` with
//! the page size, and per page a `meta.json` and the pen strokes in
//! `ink.bink`. The Kobo's own recognition (Advanced notebooks) lives in
//! `page.bdom`, which Kollate doesn't read: it reads the strokes itself.
//!
//! BINK isn't documented; this reader was worked out from a Libra Colour
//! (firmware 4.45, iink 2.0.6, `format-version` 4.0). After a header, a
//! `ff ff ff ff 00` marker and a `u32`, strokes follow back to back, with
//! runs of `ff` bytes between some of them. All numbers are little-endian.
//!
//! - plain stroke: `u32 0`, `u64` time, `u32` pen, `u32 n`, then `n` × (`f32`
//!   x, `f32` y) in millimetres, `n` × `u32` pressure and `n` × `u32` time.
//! - packed stroke: `u32 0x80000000`, `u64` time, `f32` x, `f32` y (mm),
//!   `u16`, `u16` pen, `u16`, `u32 n`, then `n` × `i16` dx and `n` × `i16` dy
//!   (steps of 2 µm, each from the previous point), and `n` bytes.
//!
//! Anything else ends the strokes (other data follows them).

use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::normalize::parse_kobo_date;
use crate::{Error, Result};

/// The notebook layout this reader understands.
const FORMAT_VERSION: &str = "4.0";
/// Notebook pages are drawn at 300 dpi, like the Kobo's screen.
const PX_PER_MM: f32 = 300.0 / 25.4;

/// One page with ink on it.
#[derive(Debug, Clone, PartialEq)]
pub struct NotebookPage {
    /// The page's folder name in the notebook (stable while it exists).
    pub id: String,
    /// Position in the notebook, from 0.
    pub index: usize,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
    /// Page width in pixels; the height is that of the notebook's page,
    /// or more for a page that scrolls (Advanced notebooks do).
    pub width: u32,
    pub height: u32,
    /// Pen strokes in page pixels, in the order they were written.
    pub strokes: Vec<Vec<(f32, f32)>>,
}

impl NotebookPage {
    /// The ink as an SVG like the Kobo writes for markups: one filled
    /// outline per stroke, so the markup renderer and segmenter work on it.
    pub fn svg(&self) -> String {
        let paths: String = self
            .strokes
            .iter()
            .map(|s| format!("<path d=\"{}\"/>\n", outline(s, 2.5)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <svg width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" xmlns=\"http://www.w3.org/2000/svg\" \
             version=\"1.2\" baseProfile=\"tiny\">\n<g fill=\"#000000\" stroke=\"none\">\n{paths}</g>\n</svg>\n",
            w = self.width,
            h = self.height
        )
    }

    /// The box around the ink, with `margin` pixels to spare, in page pixels.
    pub fn ink_bounds(&self, margin: f32) -> Option<[u32; 4]> {
        let points = self.strokes.iter().flatten();
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
        }
        (l <= r).then(|| {
            [
                (l - margin).max(0.0) as u32,
                (t - margin).max(0.0) as u32,
                ((r + margin) as u32).min(self.width - 1),
                ((b + margin) as u32).min(self.height - 1),
            ]
        })
    }
}

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "format-version")]
    format_version: Option<String>,
    #[serde(rename = "iink-user-metadata")]
    user: Option<UserMeta>,
}

#[derive(Deserialize)]
struct UserMeta {
    kobo: Option<KoboMeta>,
}

#[derive(Deserialize)]
struct KoboMeta {
    geometry: Option<Geometry>,
}

#[derive(Deserialize)]
struct Geometry {
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct PageMeta {
    #[serde(rename = "creationDate")]
    created: Option<String>,
    #[serde(rename = "lastModificationDate")]
    modified: Option<String>,
}

fn invalid(what: impl std::fmt::Display) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("notebook: {what}")).into()
}

/// Reads the pages with ink from a `.nebo` file, in the order they were
/// created. Fails on a notebook format Kollate doesn't know.
pub fn read_notebook(path: &Path) -> Result<Vec<NotebookPage>> {
    read_nebo(std::fs::File::open(path)?)
}

fn read_nebo(file: impl std::io::Read + std::io::Seek) -> Result<Vec<NotebookPage>> {
    let mut zip = zip::ZipArchive::new(file).map_err(invalid)?;
    fn read<R: std::io::Read + std::io::Seek>(
        zip: &mut zip::ZipArchive<R>,
        name: &str,
    ) -> Result<Vec<u8>> {
        let mut entry = zip.by_name(name).map_err(invalid)?;
        let mut out = Vec::new();
        entry.read_to_end(&mut out)?;
        Ok(out)
    }
    let meta: Meta = serde_json::from_slice(&read(&mut zip, "meta.json")?).map_err(invalid)?;
    if meta.format_version.as_deref() != Some(FORMAT_VERSION) {
        return Err(invalid(format!(
            "unknown format version {}",
            meta.format_version.as_deref().unwrap_or("(none)")
        )));
    }
    let (width, height) = meta
        .user
        .and_then(|u| u.kobo)
        .and_then(|k| k.geometry)
        .map_or((1264, 1680), |g| (g.width, g.height));

    let ids: Vec<String> = {
        let mut ids: Vec<String> = zip
            .file_names()
            .filter_map(|n| n.strip_prefix("pages/")?.strip_suffix("/ink.bink"))
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids
    };
    let mut pages = Vec::new();
    for id in ids {
        let page_meta: Option<PageMeta> = read(&mut zip, &format!("pages/{id}/meta.json"))
            .ok()
            .and_then(|m| serde_json::from_slice(&m).ok());
        let strokes: Vec<Vec<(f32, f32)>> =
            strokes(&read(&mut zip, &format!("pages/{id}/ink.bink"))?)
                .into_iter()
                .map(|s| {
                    s.into_iter()
                        .map(|(x, y)| (x * PX_PER_MM, y * PX_PER_MM))
                        .collect()
                })
                .collect();
        if strokes.is_empty() {
            continue;
        }
        let bottom = strokes.iter().flatten().map(|p| p.1).fold(0.0, f32::max);
        pages.push(NotebookPage {
            id,
            index: 0,
            created: page_meta
                .as_ref()
                .and_then(|m| parse_kobo_date(m.created.as_deref())),
            modified: page_meta
                .as_ref()
                .and_then(|m| parse_kobo_date(m.modified.as_deref())),
            width,
            // A page that scrolls is as tall as its ink, plus a margin.
            height: height.max(bottom as u32 + 60),
            strokes,
        });
    }
    // Pages are added one after another, so creation order is page order.
    pages.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    for (i, p) in pages.iter_mut().enumerate() {
        p.index = i;
    }
    Ok(pages)
}

/// Reads the pages of the snapshot's notebooks from a mounted Kobo: each
/// notebook with ink becomes a book, and each page a bookmark of kind
/// [`Page`](super::AnnotationKind::Page) whose ID is `<volume ID>#<page>`.
/// A notebook that can't be read is noted in `unread_notebooks` and skipped.
pub fn add_notebook_pages(mount: &Path, snapshot: &mut super::KoboSnapshot) {
    use super::{AnnotationKind, KoboBookmark, Position};
    let notebooks = snapshot.notebooks.clone();
    for book in notebooks {
        let pages = super::epub::volume_path(mount, &book.volume_id)
            .ok_or_else(|| "file not found".to_owned())
            .and_then(|path| read_notebook(&path).map_err(|e| e.to_string()));
        let pages = match pages {
            Ok(pages) => pages,
            Err(why) => {
                snapshot
                    .unread_notebooks
                    .push((book.volume_id.clone(), why));
                continue;
            }
        };
        if pages.is_empty() {
            continue;
        }
        for page in pages {
            let position = Position {
                container_path: page.id.clone(),
                child_index: 0,
                offset: page.index as i64,
            };
            let bookmark_id = format!("{}#{}", book.volume_id, page.id);
            snapshot.bookmarks.push(KoboBookmark {
                bookmark_id: bookmark_id.clone(),
                volume_id: book.volume_id.clone(),
                content_id: book.volume_id.clone(),
                kind: AnnotationKind::Page,
                text: None,
                note: None,
                color: 0,
                start: position.clone(),
                end: position,
                chapter_progress: 0.0,
                chapter_title: Some(format!("Page {}", page.index + 1)),
                spine_index: Some(page.index as i64),
                created: page.created,
                modified: page.modified,
                extra_data: None,
            });
            snapshot.notebook_ink.insert(bookmark_id, page);
        }
        if !snapshot.books.iter().any(|b| b.volume_id == book.volume_id) {
            snapshot.books.push(book);
        }
    }
    snapshot.notebooks_read = true;
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    u32_at(b, at).map(f32::from_bits)
}

fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// A point on the page? (Anything else means the bytes aren't a stroke.)
fn plausible((x, y): (f32, f32)) -> bool {
    (-10.0..2000.0).contains(&x) && (-10.0..20000.0).contains(&y)
}

/// The pen strokes in a BINK file, in millimetres (see the module docs).
fn strokes(b: &[u8]) -> Vec<Vec<(f32, f32)>> {
    let mut out = Vec::new();
    let Some(start) = b
        .windows(5)
        .position(|w| w == [0xff, 0xff, 0xff, 0xff, 0x00])
    else {
        return out;
    };
    let mut p = start + 9;
    while p < b.len() {
        if b[p] == 0xff {
            p += 1;
            continue;
        }
        match stroke_at(b, p) {
            Some((points, size)) => {
                out.push(points);
                p += size;
            }
            None => break,
        }
    }
    out
}

/// The stroke at `p` and its size in bytes.
fn stroke_at(b: &[u8], p: usize) -> Option<(Vec<(f32, f32)>, usize)> {
    const LIMIT: usize = 1_000_000;
    match u32_at(b, p)? {
        0 => {
            let n = u32_at(b, p + 16)? as usize;
            let size = 20 + 16 * n;
            if n == 0 || n > LIMIT || p + size > b.len() {
                return None;
            }
            let points: Vec<_> = (0..n)
                .map(|i| Some((f32_at(b, p + 20 + 8 * i)?, f32_at(b, p + 24 + 8 * i)?)))
                .collect::<Option<_>>()?;
            points
                .iter()
                .all(|&q| plausible(q))
                .then_some((points, size))
        }
        0x8000_0000 => {
            let (mut x, mut y) = (f32_at(b, p + 12)?, f32_at(b, p + 16)?);
            let n = u32_at(b, p + 26)? as usize;
            let size = 30 + 5 * n;
            if n == 0 || n > LIMIT || p + size > b.len() || !plausible((x, y)) {
                return None;
            }
            let (dx, dy) = (p + 30, p + 30 + 2 * n);
            let mut points = Vec::with_capacity(n);
            for i in 0..n {
                x += f32::from(i16_at(b, dx + 2 * i)?) / 500.0;
                y += f32::from(i16_at(b, dy + 2 * i)?) / 500.0;
                points.push((x, y));
            }
            points
                .iter()
                .all(|&q| plausible(q))
                .then_some((points, size))
        }
        _ => None,
    }
}

/// A filled outline around a polyline, `half` pixels either side.
fn outline(points: &[(f32, f32)], half: f32) -> String {
    let points: Vec<(f32, f32)> = match points {
        [(x, y)] => vec![(x - 0.5, *y), (x + 0.5, *y)],
        _ => points.to_vec(),
    };
    let last = points.len() - 1;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (i, &(x, y)) in points.iter().enumerate() {
        let (a, b) = (points[i.saturating_sub(1)], points[(i + 1).min(last)]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(f32::EPSILON);
        let (nx, ny) = (-dy / len * half, dx / len * half);
        left.push((x + nx, y + ny));
        right.push((x - nx, y - ny));
    }
    let ring: Vec<String> = left
        .into_iter()
        .chain(right.into_iter().rev())
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect();
    format!("M{} Z", ring.join(" L"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A BINK file with one plain and one packed stroke, as the Kobo writes them.
    fn bink() -> Vec<u8> {
        let mut b = b"BINK\x00\x05\x00\x00".to_vec();
        b.extend([0; 8]);
        b.extend([0xff, 0xff, 0xff, 0xff, 0x00]);
        b.extend(2u32.to_le_bytes());
        // Plain: two points.
        b.extend(0u32.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(0x0c49u32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        for v in [10.0f32, 20.0, 11.0, 21.0] {
            b.extend(v.to_le_bytes());
        }
        b.extend([0; 16]); // pressure, time
        b.extend([0xff; 12]); // padding seen between strokes
        // Packed: starts at (5, 5) mm, three steps.
        b.extend(0x8000_0000u32.to_le_bytes());
        b.extend(2u64.to_le_bytes());
        b.extend(5.0f32.to_le_bytes());
        b.extend(5.0f32.to_le_bytes());
        b.extend([0x7e, 0x11, 0x49, 0x0c, 0, 0]);
        b.extend(3u32.to_le_bytes());
        for d in [0i16, 500, 500, 0, -250, 1000] {
            b.extend(d.to_le_bytes());
        }
        b.extend([0; 3]);
        // Other data after the strokes.
        b.extend([1, 0, 0, 0, 3, 0, 0, 0]);
        b
    }

    #[test]
    fn reads_plain_and_packed_strokes() {
        let s = strokes(&bink());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], vec![(10.0, 20.0), (11.0, 21.0)]);
        assert_eq!(s[1], vec![(5.0, 5.0), (6.0, 4.5), (7.0, 6.5)]);
        assert!(strokes(b"BINK").is_empty());
    }

    fn nebo(version: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        let meta = format!(
            r#"{{"format-version": "{version}", "iink-user-metadata": {{"kobo": {{"geometry": {{"width": 1188, "height": 1485}}}}}}}}"#
        );
        for (name, data) in [
            ("meta.json", meta.into_bytes()),
            (
                "pages/bbbb/meta.json",
                br#"{"creationDate": "2026-09-29 18:37:02.2"}"#.to_vec(),
            ),
            ("pages/bbbb/ink.bink", bink()),
            (
                "pages/aaaa/meta.json",
                br#"{"creationDate": "2026-09-29 18:38:00.0"}"#.to_vec(),
            ),
            ("pages/aaaa/ink.bink", bink()),
            ("pages/empty/meta.json", b"{}".to_vec()),
            ("pages/empty/ink.bink", b"BINK".to_vec()),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(&data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_pages_in_creation_order() {
        let pages = read_nebo(std::io::Cursor::new(nebo("4.0"))).unwrap();
        let ids: Vec<_> = pages.iter().map(|p| (p.id.as_str(), p.index)).collect();
        assert_eq!(ids, [("bbbb", 0), ("aaaa", 1)]);
        let p = &pages[0];
        assert_eq!((p.width, p.height), (1188, 1485));
        let (x, y) = p.strokes[0][0];
        assert!((x - 10.0 * PX_PER_MM).abs() < 0.01 && (y - 20.0 * PX_PER_MM).abs() < 0.01);
        let svg = p.svg();
        assert_eq!(svg.matches("<path").count(), 2);
        let [l, t, r, b] = p.ink_bounds(10.0).unwrap();
        assert!(l < 60 && t < 60 && r > 120 && b > 240);
        assert!(read_nebo(std::io::Cursor::new(nebo("5.0"))).is_err());
    }
}

#[cfg(test)]
mod real {
    /// Reads real notebooks from `$KOLLATE_NEBO_DIR` (not in the repo).
    #[test]
    fn reads_real_notebooks() {
        let Some(dir) = std::env::var_os("KOLLATE_NEBO_DIR") else {
            return;
        };
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "nebo") {
                let pages = super::read_notebook(&entry.path()).unwrap();
                let counts: Vec<_> = pages
                    .iter()
                    .map(|p| (p.index, p.strokes.len(), p.width, p.height))
                    .collect();
                eprintln!("{}: {counts:?}", entry.path().display());
            }
        }
    }
}
