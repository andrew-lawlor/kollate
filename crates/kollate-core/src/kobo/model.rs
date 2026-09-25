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

/// Kobo highlight colour index. Mapping still to be verified on the device.
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
    pub fn reading_order_key(&self) -> (i64, Vec<i64>, i64, u64) {
        (
            self.spine_index.unwrap_or(i64::MAX),
            kobo_span_numbers(&self.start.container_path),
            self.start.offset,
            (self.chapter_progress * 1e9) as u64,
        )
    }
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
}
