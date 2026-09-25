//! Reads sideloaded (DRM-free) EPUBs on the device to find the sentence a
//! Vocab Builder word was looked up in. Kobo doesn't record where a word was
//! looked up, so matches are ranked by closeness to what was being read
//! around that time (the nearest highlight in time).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

use super::{KoboBookmark, KoboSnapshot};
use crate::Result;

/// Maps a sideloaded `VolumeID` (`file:///mnt/onboard/…`) to its path under
/// the mount point. Store books (UUID volume IDs) return `None`.
pub fn volume_path(mount: &Path, volume_id: &str) -> Option<PathBuf> {
    let relative = volume_id.strip_prefix("file:///mnt/onboard/")?;
    Some(mount.join(relative)).filter(|p| p.is_file())
}

/// The zip entry of a bookmark's chapter file: `…epub!OEBPS!ch09.xhtml#x`
/// becomes `OEBPS/ch09.xhtml`, and `…epub!!ch09.html` becomes `ch09.html`.
fn chapter_entry(content_id: &str, volume_id: &str) -> Option<String> {
    let book = volume_id.strip_prefix("file://")?;
    let rest = content_id.strip_prefix(book)?.strip_prefix('!')?;
    let file = rest.split('#').next()?;
    Some(file.replace('!', "/").trim_start_matches('/').to_owned())
}

/// A book's text, one entry per spine document, in reading order.
pub struct BookText {
    chapters: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub sentence: String,
    /// Position of its chapter in the spine.
    pub chapter: usize,
}

fn io_err(msg: impl std::fmt::Display) -> crate::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string()).into()
}

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Resolves `href` relative to the directory of `base` (a zip path).
fn join_zip_path(base: &str, href: &str) -> String {
    let mut parts: Vec<&str> = base
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    for seg in href.split('#').next().unwrap_or(href).split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    // hrefs are URL-encoded (e.g. spaces as %20).
    percent_decode(&joined)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "blockquote",
    "br",
    "tr",
    "td",
    "section",
    "dd",
    "dt",
    "figcaption",
    "pre",
];

fn html_entity(name: &str) -> &'static str {
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "rsquo" | "lsquo" => "’",
        "nbsp" | "ensp" | "emsp" | "thinsp" => " ",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "ldquo" => "“",
        "rdquo" => "”",
        _ => "",
    }
}

/// Paragraph texts of one XHTML document.
fn paragraphs(xhtml: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xhtml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;
    let mut out = Vec::new();
    let mut current = String::new();
    let mut skip_depth = 0usize;
    let mut in_body = false;
    let flush = |current: &mut String, out: &mut Vec<String>| {
        let text = current.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            out.push(text);
        }
        current.clear();
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name().as_ref().to_ascii_lowercase();
                match name.as_str() {
                    "body" => in_body = true,
                    "script" | "style" | "head" | "rt" => skip_depth += 1,
                    n if BLOCK_TAGS.contains(&n) => flush(&mut current, &mut out),
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if BLOCK_TAGS.contains(&e.local_name().as_ref().to_ascii_lowercase().as_str()) {
                    flush(&mut current, &mut out);
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().as_ref().to_ascii_lowercase();
                match name.as_str() {
                    "script" | "style" | "head" | "rt" => skip_depth = skip_depth.saturating_sub(1),
                    n if BLOCK_TAGS.contains(&n) => flush(&mut current, &mut out),
                    _ => {}
                }
            }
            Ok(Event::Text(t)) if in_body && skip_depth == 0 => {
                current.push_str(&t.xml10_content())
            }
            Ok(Event::CData(t)) if in_body && skip_depth == 0 => {
                current.push_str(&t.xml10_content())
            }
            Ok(Event::GeneralRef(r)) if in_body && skip_depth == 0 => match r.resolve_char_ref() {
                Ok(Some(c)) => current.push(c),
                _ => current.push_str(html_entity(&r)),
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    flush(&mut current, &mut out);
    out
}

impl BookText {
    /// Reads the spine of an EPUB. Returns `None` for encrypted (DRM) books.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(io_err)?;
        if zip.by_name("META-INF/encryption.xml").is_ok()
            || zip.by_name("META-INF/rights.xml").is_ok()
        {
            return Ok(None);
        }
        let mut read = |name: &str| -> Result<String> {
            let mut file = zip.by_name(name).map_err(io_err)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        };

        let container = read("META-INF/container.xml")?;
        let mut opf_path = None;
        let mut reader = quick_xml::Reader::from_str(&container);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e))
                    if e.local_name().as_ref() == "rootfile" =>
                {
                    opf_path = attr(&e, "full-path");
                    break;
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        let opf_path = opf_path.ok_or_else(|| io_err("no rootfile in container.xml"))?;
        let opf = read(&opf_path)?;

        let mut manifest = HashMap::new();
        let mut spine = Vec::new();
        let mut reader = quick_xml::Reader::from_str(&opf);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                    "item" => {
                        if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                            manifest.insert(id, join_zip_path(&opf_path, &href));
                        }
                    }
                    "itemref" => spine.extend(attr(&e, "idref")),
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }

        let mut chapters = Vec::new();
        for idref in spine {
            let Some(href) = manifest.get(&idref) else {
                continue;
            };
            if let Ok(xhtml) = read(href) {
                chapters.push((href.clone(), paragraphs(&xhtml)));
            }
        }
        Ok(Some(Self { chapters }))
    }

    fn chapter_index(&self, entry: &str) -> Option<usize> {
        self.chapters.iter().position(|(href, _)| href == entry)
    }

    /// Sentences containing `word` (whole word, case-insensitive), nearest
    /// to `near_chapter` first, at most `max`.
    pub fn contexts(&self, word: &str, near_chapter: Option<usize>, max: usize) -> Vec<Context> {
        let mut found = Vec::new();
        for (index, (_, paragraphs)) in self.chapters.iter().enumerate() {
            for paragraph in paragraphs {
                for sentence in sentences(paragraph) {
                    if contains_word(sentence, word) {
                        let sentence = strip_note_refs(sentence);
                        found.push(Context {
                            sentence: trim_around(&sentence, word),
                            chapter: index,
                        });
                    }
                }
            }
        }
        if let Some(near) = near_chapter {
            // Stable sort keeps reading order among equally distant chapters.
            found.sort_by_key(|c| c.chapter.abs_diff(near));
        }
        found.dedup_by(|a, b| a.sentence == b.sentence);
        found.truncate(max);
        found
    }
}

/// Splits a paragraph after `.`, `!`, `?` or `…` (plus closing quotes)
/// when followed by whitespace.
fn sentences(paragraph: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = paragraph.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i].1, '.' | '!' | '?' | '…') {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '"' | '\'' | '”' | '’' | ')' | ']' | '.')
            {
                j += 1;
            }
            if j < chars.len() && chars[j].1.is_whitespace() {
                let end = chars[j].0;
                out.push(paragraph[start..end].trim());
                start = end;
                i = j;
            }
        }
        i += 1;
    }
    let rest = paragraph[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn contains_word(sentence: &str, word: &str) -> bool {
    find_word(sentence, word).is_some()
}

/// Byte range of the first whole-word, case-insensitive match of `word`.
pub fn word_span(text: &str, word: &str) -> Option<(usize, usize)> {
    find_word(text, word).map(|start| (start, start + word.to_lowercase().len()))
}

/// Byte offset of `word` in `text` as a whole word, ignoring case.
fn find_word(text: &str, word: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    let target = word.to_lowercase();
    // Lowercasing can change byte lengths; only trust offsets when it didn't.
    if lower.len() != text.len() || target.is_empty() {
        return None;
    }
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&target) {
        let start = from + pos;
        let end = start + target.len();
        let before = lower[..start].chars().next_back();
        let after = lower[end..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        if boundary(before) && boundary(after) {
            return Some(start);
        }
        from = end;
    }
    None
}

/// Removes footnote markers such as `[39]` (and a dangling `[` at the end).
fn strip_note_refs(sentence: &str) -> String {
    let mut out = String::with_capacity(sentence.len());
    let mut rest = sentence;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            out.push_str(&rest[..open]);
            rest = &after[digits + 1..];
        } else if after.trim().is_empty() {
            out.push_str(&rest[..open]);
            rest = "";
        } else {
            out.push_str(&rest[..=open]);
            rest = after;
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// Shortens very long sentences to a window around the word.
fn trim_around(sentence: &str, word: &str) -> String {
    const MAX: usize = 320;
    if sentence.chars().count() <= MAX {
        return sentence.to_owned();
    }
    let Some(pos) = find_word(sentence, word) else {
        return sentence.to_owned();
    };
    let char_pos = sentence[..pos].chars().count();
    let start = char_pos.saturating_sub(MAX / 2);
    let snippet: String = sentence.chars().skip(start).take(MAX).collect();
    let mut out = snippet.trim().to_owned();
    if start > 0 {
        out = format!("…{out}");
    }
    if start + MAX < sentence.chars().count() {
        out.push('…');
    }
    out
}

/// Context candidates for one looked-up word.
#[derive(Debug, Clone)]
pub struct WordContexts {
    pub volume_id: String,
    pub word: String,
    pub contexts: Vec<Context>,
}

/// The bookmark in `volume_id` made closest in time to `at` (within 3 days).
fn nearest_bookmark<'a>(
    bookmarks: &'a [KoboBookmark],
    volume_id: &str,
    at: DateTime<Utc>,
) -> Option<&'a KoboBookmark> {
    bookmarks
        .iter()
        .filter(|b| b.volume_id == volume_id)
        .filter_map(|b| Some((b, (b.created? - at).num_seconds().unsigned_abs())))
        .filter(|(_, secs)| *secs <= 3 * 24 * 3600)
        .min_by_key(|(_, secs)| *secs)
        .map(|(b, _)| b)
}

/// Finds context sentences for every looked-up word in sideloaded books on
/// a mounted Kobo. Each book is read once. Books that can't be read (store
/// books, DRM, damaged files) are skipped.
pub fn find_word_contexts(
    mount: &Path,
    snapshot: &KoboSnapshot,
    max_per_word: usize,
) -> Vec<WordContexts> {
    let mut by_book: HashMap<&str, Vec<&super::KoboWord>> = HashMap::new();
    for w in &snapshot.words {
        if let Some(v) = &w.volume_id {
            by_book.entry(v).or_default().push(w);
        }
    }
    let mut out = Vec::new();
    for (volume_id, words) in by_book {
        let Some(path) = volume_path(mount, volume_id) else {
            continue;
        };
        let Ok(Some(book)) = BookText::read(&path) else {
            continue;
        };
        for w in words {
            let near = w
                .created
                .and_then(|at| nearest_bookmark(&snapshot.bookmarks, volume_id, at))
                .and_then(|b| chapter_entry(&b.content_id, volume_id))
                .and_then(|entry| book.chapter_index(&entry));
            out.push(WordContexts {
                volume_id: volume_id.to_owned(),
                word: w.word.clone(),
                contexts: book.contexts(&w.word, near, max_per_word),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_paragraphs_from_kepub_xhtml() {
        let xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title>
            <style>p{}</style></head><body><div class="book-inner">
            <p><span class="koboSpan" id="kobo.1.1">The moon began to rise,</span> <span class="koboSpan" id="kobo.1.2">lean&#160;and&nbsp;haggard.</span></p>
            <p>Second&mdash;para<br/>graph</p></div></body></html>"#;
        assert_eq!(
            paragraphs(xhtml),
            [
                "The moon began to rise, lean and haggard.",
                "Second—para",
                "graph"
            ]
        );
    }

    #[test]
    fn splits_sentences_and_matches_whole_words() {
        let p = "He saw the Demiurge. “Was it a demiurge?” she asked! Demiurges differ… The end";
        assert_eq!(
            sentences(p),
            [
                "He saw the Demiurge.",
                "“Was it a demiurge?”",
                "she asked!",
                "Demiurges differ…",
                "The end"
            ]
        );
        assert!(contains_word("He saw the Demiurge.", "demiurge"));
        assert!(!contains_word("Demiurges differ", "demiurge"));
        assert!(contains_word("séances were held", "séances"));
    }

    #[test]
    fn maps_content_ids_to_zip_entries() {
        let v = "file:///mnt/onboard/A/B.kepub.epub";
        assert_eq!(
            chapter_entry("/mnt/onboard/A/B.kepub.epub!OEBPS!ch09.xhtml#r1", v).as_deref(),
            Some("OEBPS/ch09.xhtml")
        );
        assert_eq!(
            chapter_entry("/mnt/onboard/A/B.kepub.epub!!split_5.html", v).as_deref(),
            Some("split_5.html")
        );
        assert_eq!(
            join_zip_path("OEBPS/content.opf", "Text/ch%201.xhtml"),
            "OEBPS/Text/ch 1.xhtml"
        );
        assert_eq!(
            join_zip_path("OPS/x/content.opf", "../xhtml/a.xhtml"),
            "OPS/xhtml/a.xhtml"
        );
    }

    #[test]
    fn strips_footnote_markers() {
        assert_eq!(
            strip_note_refs("compared to séances.[39] Theurgy"),
            "compared to séances. Theurgy"
        );
        assert_eq!(
            strip_note_refs("the true mysteries.[80]"),
            "the true mysteries."
        );
        assert_eq!(strip_note_refs("a sacred rite”["), "a sacred rite”");
        assert_eq!(strip_note_refs("see [a] note"), "see [a] note");
    }

    #[test]
    fn trims_long_sentences_around_the_word() {
        let long = format!("{} theophany {}", "a ".repeat(300), "b ".repeat(300));
        let t = trim_around(&long, "theophany");
        assert!(t.starts_with('…') && t.ends_with('…') && t.contains("theophany"));
        assert!(t.chars().count() <= 322);
    }
}
