use chrono::{DateTime, Utc};
use serde::Serialize;

/// Kind of `Bookmark` row. Kobo stores highlights, notes (highlight + text
/// annotation), stylus markups and page bookmarks ("dogear") in one table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    Highlight,
    Note,
    Markup,
    Dogear,
    Other(String),
}

impl AnnotationKind {
    pub fn from_kobo(s: Option<&str>) -> Self {
        match s.unwrap_or("highlight") {
            "highlight" => Self::Highlight,
            "note" => Self::Note,
            "markup" => Self::Markup,
            "dogear" => Self::Dogear,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// Kobo highlight colour index (verified on a Libra Colour).
pub fn color_name(color: i64) -> &'static str {
    match color {
        0 => "yellow",
        1 => "pink",
        2 => "blue",
        3 => "green",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Position {
    pub container_path: String,
    pub child_index: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KoboBook {
    /// `content.ContentID` of the book (`file:///mnt/onboard/...` when sideloaded).
    pub volume_id: String,
    pub title: String,
    pub author: Option<String>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    pub series_number: Option<String>,
    pub image_id: Option<String>,
    pub percent_read: Option<i64>,
    pub last_read: Option<DateTime<Utc>>,
}

impl KoboBook {
    /// Sideloaded books live on the device's filesystem and can be opened
    /// (e.g. for vocab context); store books are identified by a UUID.
    pub fn is_sideloaded(&self) -> bool {
        self.volume_id.starts_with("file://")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KoboBookmark {
    pub bookmark_id: String,
    pub volume_id: String,
    pub content_id: String,
    pub kind: AnnotationKind,
    /// Cleaned highlighted text; `None` for markups and dogears.
    pub text: Option<String>,
    /// Cleaned user note; `None` when empty.
    pub note: Option<String>,
    pub color: i64,
    pub start: Position,
    pub end: Position,
    pub chapter_progress: f64,
    pub chapter_title: Option<String>,
    /// Spine (reading-order) index of the chapter file, when known.
    pub spine_index: Option<i64>,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
    /// Raw Qt-serialized `ExtraAnnotationData` (markups).
    #[serde(skip)]
    pub extra_data: Option<Vec<u8>>,
}

impl KoboBookmark {
    /// Sort key that orders bookmarks in reading order within a book.
    pub fn reading_order_key(&self) -> (i64, String) {
        (
            self.spine_index.unwrap_or(i64::MAX),
            position_key(
                &self.start.container_path,
                self.start.offset,
                self.chapter_progress,
            ),
        )
    }
}

/// A string that sorts in reading order within one chapter file. Kobo
/// positions are a paragraph path (`span#kobo\.153\.2`) plus a character
/// offset inside that span, so both must be compared numerically. Falls back
/// to chapter progress for positions without Kobo spans.
pub fn position_key(start_path: &str, start_offset: i64, chapter_progress: f64) -> String {
    let spans = kobo_span_numbers(start_path);
    if spans.is_empty() {
        return format!("p{:012}", (chapter_progress.clamp(0.0, 1.0) * 1e11) as u64);
    }
    let mut key = String::from("s");
    for n in spans {
        key.push_str(&format!("{n:08}."));
    }
    key.push_str(&format!("{:08}", start_offset.max(0)));
    key
}

/// Parses `span#kobo\.10\.4` into `[10, 4]`.
fn kobo_span_numbers(path: &str) -> Vec<i64> {
    path.rsplit_once("kobo")
        .map(|(_, rest)| {
            rest.split(|c: char| !c.is_ascii_digit())
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct KoboWord {
    /// The word exactly as looked up.
    pub word: String,
    pub volume_id: Option<String>,
    /// Dictionary language, from `DictSuffix` (`-en` becomes `en`).
    pub language: Option<String>,
    pub created: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct KoboSnapshot {
    pub db_version: i64,
    /// Books referenced by at least one bookmark or word.
    pub books: Vec<KoboBook>,
    pub bookmarks: Vec<KoboBookmark>,
    pub words: Vec<KoboWord>,
    /// Bookmarks the device marks as hidden (not included above).
    pub hidden_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kobo_span_paths() {
        assert_eq!(kobo_span_numbers(r"span#kobo\.10\.4"), vec![10, 4]);
        assert_eq!(kobo_span_numbers("span#kobo.42.3"), vec![42, 3]);
        assert!(kobo_span_numbers("div#foo").is_empty());
    }

    #[test]
    fn position_key_orders_spans_before_offsets() {
        // Real positions from four highlights in one paragraph group.
        let mut keys = [
            ("green", position_key(r"span#kobo\.153\.3", 27, 0.1)),
            ("blue", position_key(r"span#kobo\.153\.2", 30, 0.1)),
            ("yellow", position_key(r"span#kobo\.153\.1", 309, 0.1)),
            ("pink", position_key(r"span#kobo\.153\.1", 356, 0.1)),
            ("later", position_key(r"span#kobo\.1000\.1", 0, 0.1)),
        ];
        keys.sort_by(|a, b| a.1.cmp(&b.1));
        let order: Vec<_> = keys.iter().map(|k| k.0).collect();
        assert_eq!(order, ["yellow", "pink", "blue", "green", "later"]);
    }
}
