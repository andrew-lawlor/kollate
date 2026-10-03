//! Handwriting from KOReader's Pencil plugin (the maintained fork, SPEC
//! §8f): each page visit's ink, a picture of the page and the words on it,
//! in `<book>.sdr/pencil/markups/<id>/`. Read-only, like the rest.

use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::Deserialize;

use crate::Result;
use crate::kobo::{AnnotationKind, KoboBookmark};
use crate::markup::PageWord;
use crate::normalize::clean_opt;

/// Prefix of the bookmark IDs Kollate gives Pencil markups: under
/// KOReader's own prefix, so they're treated as KOReader's everywhere.
pub const ID_PREFIX: &str = "koreader:ink:";

/// The export format this reader understands.
const FORMAT: u32 = 1;

/// A markup's ink and page, for the library's copy (see
/// [`copy_assets`](crate::kobo::assets::copy_assets)).
#[derive(Debug, Clone)]
pub struct PencilMarkup {
    /// The ink as an SVG Kollate's handwriting reader takes: one stroked
    /// path per pen stroke, in page pixels.
    pub svg: String,
    pub width: u32,
    pub height: u32,
    /// The page as it looked, without the ink (`page.png`), if written.
    pub page_png: Option<PathBuf>,
    pub words: Vec<PageWord>,
    /// The ink's bounds with a margin: the part of the page worth showing.
    pub crop: Option<[i32; 4]>,
}

#[derive(Deserialize)]
struct MarkupJson {
    format: u32,
    id: String,
    created: Option<i64>,
    modified: Option<i64>,
    start: Option<String>,
    #[serde(rename = "end")]
    finish: Option<String>,
    chapter: Option<String>,
    screen: Option<Screen>,
    #[serde(default)]
    has_page_image: bool,
}

#[derive(Deserialize)]
struct Screen {
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct InkJson {
    strokes: Vec<InkStroke>,
}

#[derive(Deserialize)]
struct InkStroke {
    points: Vec<[f32; 2]>,
    width: Option<f32>,
    color: Option<String>,
    tool: Option<String>,
}

#[derive(Deserialize)]
struct WordsJson {
    words: Vec<PageWord>,
}

/// The markups in a book's settings folder `sdr`, as annotations of the book
/// `volume_id`, and their ink. A folder still being written (no
/// `markup.json` yet) is skipped; one that can't be read is an error, so the
/// book's markups aren't taken as deleted.
pub fn read_markups(sdr: &Path, volume_id: &str) -> Result<Vec<(KoboBookmark, PencilMarkup)>> {
    let dir = sdr.join("pencil/markups");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("markup.json").is_file())
        .collect();
    folders.sort();
    folders.iter().map(|f| read_markup(f, volume_id)).collect()
}

fn read_markup(folder: &Path, volume_id: &str) -> Result<(KoboBookmark, PencilMarkup)> {
    let markup: MarkupJson = read_json(&folder.join("markup.json"))?;
    if markup.format != FORMAT {
        return Err(invalid(format!(
            "unknown Pencil export format {}",
            markup.format
        )));
    }
    let ink: InkJson = read_json(&folder.join("ink.json"))?;
    let words = read_json::<WordsJson>(&folder.join("words.json"))
        .map(|w| w.words)
        .unwrap_or_default();
    let (width, height) = markup
        .screen
        .as_ref()
        .map_or_else(|| ink_extent(&ink), |s| (s.width, s.height));
    let start = markup.start.as_deref().unwrap_or_default();
    let (start_pos, spine) = super::xpointer(start);
    let (end_pos, _) = super::xpointer(markup.finish.as_deref().unwrap_or(start));
    let time = |t: Option<i64>| t.and_then(|t| DateTime::from_timestamp(t, 0));
    let bookmark = KoboBookmark {
        bookmark_id: format!("{ID_PREFIX}{}", markup.id),
        volume_id: volume_id.to_owned(),
        content_id: volume_id.to_owned(),
        kind: AnnotationKind::Markup,
        text: None,
        note: None,
        color: crate::color::DEFAULT_COLOR.to_owned(),
        start: start_pos,
        end: end_pos,
        chapter_progress: 0.0,
        chapter_title: clean_opt(markup.chapter.as_deref()),
        spine_index: spine,
        created: time(markup.created),
        modified: time(markup.modified.or(markup.created)),
        extra_data: None,
    };
    let page_png = Some(folder.join("page.png")).filter(|p| markup.has_page_image && p.is_file());
    let pencil = PencilMarkup {
        svg: ink_svg(&ink, width, height),
        width,
        height,
        page_png,
        words,
        crop: ink_bounds(&ink, 40.0, width, height),
    };
    Ok((bookmark, pencil))
}

/// The page size when the export has none: the ink's extent.
fn ink_extent(ink: &InkJson) -> (u32, u32) {
    let (w, h) = ink
        .strokes
        .iter()
        .flat_map(|s| &s.points)
        .fold((0.0f32, 0.0f32), |(w, h), p| (w.max(p[0]), h.max(p[1])));
    (w.ceil() as u32 + 1, h.ceil() as u32 + 1)
}

/// The ink as stroked paths. Each path carries its own colour and width, so
/// they stay strokes wherever they're drawn (the reader wraps notes in a
/// filled group). A highlighter stroke is drawn translucent.
fn ink_svg(ink: &InkJson, width: u32, height: u32) -> String {
    let mut paths = String::new();
    for s in &ink.strokes {
        let Some(first) = s.points.first() else {
            continue;
        };
        let mut d = format!("M{},{}", first[0], first[1]);
        if s.points.len() == 1 {
            // A dot: a stroke needs a length to be drawn.
            d.push_str(&format!(" L{},{}", first[0] + 0.5, first[1]));
        }
        for p in &s.points[1..] {
            d.push_str(&format!(" L{},{}", p[0], p[1]));
        }
        let highlighter = s.tool.as_deref() == Some("highlighter");
        paths.push_str(&format!(
            "<path fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"{} d=\"{d}\"/>\n",
            color_hex(s.color.as_deref()),
            s.width.unwrap_or(3.0),
            if highlighter { " stroke-opacity=\"0.4\"" } else { "" },
        ));
    }
    format!(
        "<svg width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" xmlns=\"http://www.w3.org/2000/svg\">\n<g>\n{paths}</g></svg>"
    )
}

/// The Pencil plugin's pen colours.
fn color_hex(name: Option<&str>) -> &'static str {
    match name.unwrap_or("Black") {
        "Red" => "#ff3300",
        "Orange" => "#ff8800",
        "Yellow" => "#ffff33",
        "Green" => "#00aa66",
        "Olive" => "#88ff77",
        "Cyan" => "#00ffee",
        "Blue" => "#0066ff",
        "Purple" => "#ee00ff",
        "Gray" => "#888888",
        _ => "#000000",
    }
}

/// The ink's bounds, `margin` wider, within the page.
fn ink_bounds(ink: &InkJson, margin: f32, width: u32, height: u32) -> Option<[i32; 4]> {
    let mut points = ink.strokes.iter().flat_map(|s| &s.points);
    let first = points.next()?;
    let (mut l, mut t, mut r, mut b) = (first[0], first[1], first[0], first[1]);
    for p in points {
        l = l.min(p[0]);
        t = t.min(p[1]);
        r = r.max(p[0]);
        b = b.max(p[1]);
    }
    Some([
        (l - margin).max(0.0) as i32,
        (t - margin).max(0.0) as i32,
        ((r + margin) as i32).min(width as i32 - 1),
        ((b + margin) as i32).min(height as i32 - 1),
    ])
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))
}

fn invalid(e: impl std::fmt::Display) -> crate::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::segment::strokes;

    #[test]
    fn ink_becomes_stroked_paths_the_segmenter_reads() {
        let ink: InkJson = serde_json::from_str(
            r#"{"strokes": [
                {"points": [[10, 20], [30, 40]], "width": 5, "color": "Red", "tool": "pen"},
                {"points": [[50, 60]], "color": "Black"}
            ]}"#,
        )
        .unwrap();
        let svg = ink_svg(&ink, 100, 200);
        let s = strokes(&svg);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].points, [(10.0, 20.0), (30.0, 40.0)]);
        assert!(s[0].xml.contains("stroke=\"#ff3300\"") && s[0].xml.contains("stroke-width=\"5\""));
        assert!(svg.contains("viewBox=\"0 0 100 200\""));
        assert_eq!(ink_bounds(&ink, 10.0, 100, 200), Some([0, 10, 60, 70]));
    }
}
