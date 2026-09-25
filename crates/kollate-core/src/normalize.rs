//! Text cleanup, date parsing and fingerprints used for de-duplication.

use chrono::{DateTime, NaiveDateTime, Utc};
use unicode_normalization::UnicodeNormalization;

/// Cleans text for display: NFC, per-line whitespace collapsed, blank
/// leading/trailing lines removed. Paragraph breaks are kept.
pub fn clean_text(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    let lines: Vec<String> = nfc
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let start = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

/// Like [`clean_text`] but returns `None` for empty results.
pub fn clean_opt(s: Option<&str>) -> Option<String> {
    s.map(clean_text).filter(|t| !t.is_empty())
}

/// Canonical form for comparing text: cleaned, lowercased, quotes/dashes
/// unified, all whitespace collapsed to single spaces.
pub fn comparable_text(s: &str) -> String {
    let unified: String = clean_text(s)
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            c => c,
        })
        .collect();
    unified
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses Kobo's mixed timestamp formats (`2026-08-13T21:24:36.152`,
/// `2026-08-13T21:24:36Z`, `2026-08-13 21:24:36`). All are treated as UTC.
pub fn parse_kobo_date(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?.trim().trim_end_matches('Z');
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
        .map(|dt| dt.and_utc())
}

fn fingerprint(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(&[0x1f]);
    }
    h.finalize().to_hex()[..32].to_owned()
}

fn alnum_words(s: &str) -> String {
    comparable_text(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Identifies a book independently of its path on the device.
pub fn book_fingerprint(title: &str, author: Option<&str>) -> String {
    fingerprint(&[&alnum_words(title), &alnum_words(author.unwrap_or(""))])
}

/// Identifies a highlight independently of its Kobo `BookmarkID`, so the same
/// highlight is recognized after a factory reset or on a second device.
pub fn annotation_fingerprint(
    book_fp: &str,
    text: &str,
    start_path: &str,
    start_offset: i64,
) -> String {
    fingerprint(&[
        book_fp,
        &comparable_text(text),
        start_path,
        &start_offset.to_string(),
    ])
}

/// Identifies a stylus markup, which has no text: its anchor range and
/// creation time within the book.
pub fn markup_fingerprint(
    book_fp: &str,
    start: (&str, i64),
    end: (&str, i64),
    created: Option<&str>,
) -> String {
    fingerprint(&[
        book_fp,
        "markup",
        start.0,
        &start.1.to_string(),
        end.0,
        &end.1.to_string(),
        created.unwrap_or(""),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_whitespace_but_keeps_paragraphs() {
        assert_eq!(clean_text("  a  b \n\n"), "a b");
        assert_eq!(clean_text("\n one \n two  three\n"), "one\ntwo three");
        assert_eq!(clean_opt(Some("  ")), None);
    }

    #[test]
    fn comparable_text_unifies_typography() {
        assert_eq!(comparable_text("“It’s  —  Fine”"), "\"it's - fine\"");
    }

    #[test]
    fn parses_mixed_kobo_dates() {
        let a = parse_kobo_date(Some("2026-08-13T21:24:36.152")).unwrap();
        let b = parse_kobo_date(Some("2026-08-13T21:24:36Z")).unwrap();
        assert_eq!(a.timestamp(), b.timestamp());
        assert!(parse_kobo_date(Some("0000-00-00T00:00:00.000")).is_none());
        assert!(parse_kobo_date(None).is_none());
    }

    #[test]
    fn book_fingerprint_ignores_case_and_punctuation() {
        assert_eq!(
            book_fingerprint(
                "Hellenic Tantra: The Theurgic Platonism",
                Some("Shaw, Gregory")
            ),
            book_fingerprint(
                "hellenic tantra — the theurgic platonism",
                Some("Shaw Gregory")
            ),
        );
    }
}
