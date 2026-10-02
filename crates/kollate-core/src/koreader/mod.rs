//! KOReader's highlights, notes and vocabulary on a Kobo (SPEC §8e). They
//! live in its own files, not in `KoboReader.sqlite`: a Lua table next to
//! each book (the "sidecar") and a vocabulary database. Read-only, like the
//! rest of Kollate.

pub mod lua;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use rusqlite::Connection;

use crate::kobo::{AnnotationKind, KoboBook, KoboBookmark, KoboSnapshot, KoboWord, Position};
use crate::normalize::clean_opt;
use crate::{Error, Result};
use lua::Value;

/// Prefix of the bookmark IDs Kollate gives KOReader annotations.
pub const ID_PREFIX: &str = "koreader:";

/// KOReader's folder on a mounted Kobo, if it's installed.
pub fn koreader_dir(mount: &Path) -> Option<PathBuf> {
    let dir = mount.join(".adds/koreader");
    dir.is_dir().then_some(dir)
}

/// Adds KOReader's books, annotations and words on `mount` to `snapshot`.
/// Only books with an annotation or a word are added, as with Nickel; one
/// already in the snapshot (from Nickel) keeps Nickel's details. A sidecar that can't be read is
/// noted in `koreader_unread`, so its annotations aren't taken as deleted.
/// Without KOReader, `koreader_read` stays false and nothing changes.
pub fn add_koreader(mount: &Path, snapshot: &mut KoboSnapshot) {
    let Some(dir) = koreader_dir(mount) else {
        return;
    };
    snapshot.koreader_read = true;
    let mut titles: HashMap<String, (String, Option<String>)> = HashMap::new();
    // Books KOReader has opened, added once something refers to them.
    let mut opened: Vec<KoboBook> = Vec::new();
    for sdr in sidecars(mount, &dir) {
        match read_sidecar(&sdr) {
            Ok(Some(book)) => {
                titles.insert(
                    title_key(&book.book.title),
                    (book.book.volume_id.clone(), book.book.language.clone()),
                );
                snapshot.bookmarks.extend(book.annotations);
                opened.push(book.book);
            }
            Ok(None) => {}
            Err(why) => snapshot
                .koreader_unread
                .push((sdr.display().to_string(), why.to_string())),
        }
    }
    // Nickel's books count too: a word looked up in KOReader names its book
    // only by title.
    for b in &snapshot.books {
        titles
            .entry(title_key(&b.title))
            .or_insert_with(|| (b.volume_id.clone(), b.language.clone()));
    }
    let vocab = dir.join("settings/vocabulary_builder.sqlite3");
    if vocab.is_file() {
        match read_words(&vocab, &titles) {
            Ok(words) => snapshot.words.extend(words),
            Err(why) => snapshot
                .koreader_unread
                .push((vocab.display().to_string(), why.to_string())),
        }
    }
    for book in opened {
        let id = book.volume_id.as_str();
        let used = snapshot.bookmarks.iter().any(|b| b.volume_id == id)
            || snapshot
                .words
                .iter()
                .any(|w| w.volume_id.as_deref() == Some(id));
        if used && !snapshot.books.iter().any(|b| b.volume_id == id) {
            snapshot.books.push(book);
        }
    }
}

/// Every book-settings folder (`*.sdr`) KOReader may have written, wherever
/// `document_metadata_folder` puts them: next to the books ("doc"), under
/// `docsettings/` ("dir") or under `hashdocsettings/` ("hash"). All three are
/// read, since the setting can change. Books can be in any folder, so as
/// well as walking the device, the books in KOReader's history are looked
/// up directly.
fn sidecars(mount: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_sdr(mount, 8, &mut found, true);
    collect_sdr(&dir.join("docsettings"), 16, &mut found, false);
    collect_sdr(&dir.join("hashdocsettings"), 2, &mut found, false);
    for book in history(dir) {
        let Some(relative) = book.strip_prefix("/mnt/onboard/") else {
            continue;
        };
        let path = mount.join(relative);
        let sdr = match path.extension() {
            Some(_) => path.with_extension("sdr"),
            None => continue,
        };
        if sdr.is_dir() {
            found.push(sdr);
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The books KOReader has opened (`history.lua`), as paths on the device.
fn history(dir: &Path) -> Vec<String> {
    let Ok(source) = std::fs::read(dir.join("history.lua")) else {
        return Vec::new();
    };
    let Ok(history) = lua::parse(&String::from_utf8_lossy(&source)) else {
        return Vec::new();
    };
    history
        .table()
        .map(|t| t.array())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.get_str("file").map(str::to_owned))
        .collect()
}

fn collect_sdr(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, skip_hidden: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".sdr") {
            found.push(path);
        } else if depth > 0 && !(skip_hidden && name.starts_with('.')) {
            collect_sdr(&path, depth - 1, found, skip_hidden);
        }
    }
}

struct SidecarBook {
    book: KoboBook,
    annotations: Vec<KoboBookmark>,
}

/// Reads one book's settings. `None` when there's nothing to import (no
/// settings file, or a file KOReader opened that isn't on the device).
fn read_sidecar(sdr: &Path) -> Result<Option<SidecarBook>> {
    let Some(file) = std::fs::read_dir(sdr)?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("metadata.") && n.ends_with(".lua") && !n.ends_with(".old")
            })
        })
    else {
        return Ok(None);
    };
    let source = std::fs::read(&file)?;
    let settings = lua::parse(&String::from_utf8_lossy(&source)).map_err(invalid)?;
    let Some(doc_path) = settings.get_str("doc_path") else {
        return Ok(None);
    };
    let Some(relative) = doc_path.strip_prefix("/mnt/onboard/") else {
        return Ok(None);
    };
    // KOReader's own help pages and the like aren't books.
    if relative.starts_with('.') {
        return Ok(None);
    }
    let volume_id = format!("file:///mnt/onboard/{relative}");
    let props = settings.get("doc_props");
    let prop = |k| {
        props
            .and_then(|p| p.get_str(k))
            .and_then(|s| clean_opt(Some(s)))
    };
    let file_stem = Path::new(relative)
        .file_stem()
        .map(|s| s.to_string_lossy().trim_end_matches(".kepub").to_owned())
        .unwrap_or_default();
    let title = prop("title").unwrap_or(file_stem);
    // Several authors are written one per line.
    let author = prop("authors").map(|a| a.lines().collect::<Vec<_>>().join(" & "));
    let isbn = prop("identifiers").and_then(|ids| {
        ids.lines()
            .find_map(|l| l.trim().strip_prefix("ISBN:").map(|i| i.trim().to_owned()))
    });
    let book = KoboBook {
        volume_id: volume_id.clone(),
        title,
        author,
        publisher: None,
        isbn,
        language: prop("language"),
        series: prop("series").filter(|s| s != "N/A"),
        series_number: None,
        image_id: None,
        percent_read: settings
            .get_num("percent_finished")
            .map(|p| (p * 100.0).round() as i64),
        last_read: None,
    };
    let identity = settings
        .get_str("partial_md5_checksum")
        .unwrap_or(doc_path)
        .to_owned();
    let annotations = settings
        .get("annotations")
        .and_then(Value::table)
        .map(|t| t.array())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| annotation(a, &volume_id, &identity))
        .collect();
    Ok(Some(SidecarBook { book, annotations }))
}

/// One highlight or note. Bookmarks (no text) aren't imported, like the
/// Kobo's dog-ears.
fn annotation(a: &Value, volume_id: &str, identity: &str) -> Option<KoboBookmark> {
    let pos0 = a.get_str("pos0")?;
    let pos1 = a.get_str("pos1").unwrap_or(pos0);
    let text = clean_opt(a.get_str("text"))?;
    let note = clean_opt(a.get_str("note"));
    let created_raw = a.get_str("datetime").unwrap_or_default();
    let created = local_time(created_raw);
    let modified = a
        .get_str("datetime_updated")
        .and_then(local_time)
        .or(created);
    let id = blake3::hash(format!("{identity}\n{created_raw}\n{pos0}").as_bytes()).to_hex();
    let (start, spine) = xpointer(pos0);
    let (end, _) = xpointer(pos1);
    Some(KoboBookmark {
        bookmark_id: format!("{ID_PREFIX}{}", &id[..32]),
        volume_id: volume_id.to_owned(),
        content_id: volume_id.to_owned(),
        kind: if note.is_some() {
            AnnotationKind::Note
        } else {
            AnnotationKind::Highlight
        },
        text: Some(text),
        note,
        color: a
            .get_str("color")
            .unwrap_or(crate::color::DEFAULT_COLOR)
            .to_owned(),
        start,
        end,
        chapter_progress: 0.0,
        chapter_title: clean_opt(a.get_str("chapter")),
        spine_index: spine,
        created,
        modified,
        extra_data: None,
    })
}

/// A crengine XPointer (`/body/DocFragment[9]/body/div/p[18]/span[3]/text().0`)
/// as a position (the path, then the character offset) and its spine index
/// (`DocFragment[n]` is the n-th spine item, counted from 1).
fn xpointer(xp: &str) -> (Position, Option<i64>) {
    let (path, offset) = match xp.rsplit_once('.') {
        Some((p, o)) if o.chars().all(|c| c.is_ascii_digit()) && !o.is_empty() => {
            (p, o.parse().unwrap_or(0))
        }
        _ => (xp, 0),
    };
    let spine = path
        .split_once("DocFragment[")
        .and_then(|(_, rest)| rest.split_once(']'))
        .and_then(|(n, _)| n.parse::<i64>().ok())
        .map(|n| n - 1);
    (
        Position {
            container_path: path.to_owned(),
            child_index: 0,
            offset,
        },
        spine,
    )
}

/// KOReader writes times as the device's local time, without a zone.
fn local_time(s: &str) -> Option<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
}

fn title_key(title: &str) -> String {
    crate::normalize::comparable_text(title)
}

/// The vocabulary builder's words. Each names its book by title only; a
/// title that matches exactly one known book links the word to it.
fn read_words(
    path: &Path,
    titles: &HashMap<String, (String, Option<String>)>,
) -> Result<Vec<KoboWord>> {
    // Read a copy, so no handle is ever held on the device.
    let dir = tempfile::Builder::new()
        .prefix("kollate-koreader-")
        .tempdir()?;
    crate::kobo::ensure_not_on_kobo(dir.path())?;
    let copy = dir.path().join("vocabulary_builder.sqlite3");
    std::fs::copy(path, &copy)?;
    let conn = Connection::open(&copy)?;
    conn.pragma_update(None, "query_only", true)?;
    let mut stmt = conn.prepare(
        "SELECT v.word, t.name, v.create_time, v.prev_context, v.next_context
         FROM vocabulary v LEFT JOIN title t ON t.id = v.title_id ORDER BY v.create_time",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut words = Vec::new();
    for (word, title, created, prev, next) in rows {
        let Some(word) = clean_opt(Some(&word)) else {
            continue;
        };
        let book = title.as_deref().and_then(|t| titles.get(&title_key(t)));
        words.push(KoboWord {
            context: sentence(prev.as_deref(), &word, next.as_deref()),
            volume_id: book.map(|(v, _)| v.clone()),
            // Kobo files English words under no language; others under their
            // code ("es"), which the book's language gives here.
            language: book
                .and_then(|(_, l)| l.as_deref())
                .and_then(|l| l.split(['-', '_']).next())
                .map(str::to_lowercase)
                .filter(|l| !l.is_empty() && l != "en"),
            created: created.and_then(|t| DateTime::from_timestamp(t, 0)),
            word,
        });
    }
    Ok(words)
}

/// The sentence around a looked-up word, from the text KOReader kept on
/// either side of it.
fn sentence(prev: Option<&str>, word: &str, next: Option<&str>) -> Option<String> {
    let ends = |c: char| matches!(c, '.' | '!' | '?' | '…');
    let prev = prev.unwrap_or_default();
    let next = next.unwrap_or_default();
    // After the last sentence end that's followed by a space (or a quote).
    let start = prev
        .char_indices()
        .rev()
        .find(|&(i, c)| {
            ends(c)
                && prev[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(|n| n.is_whitespace() || "\"'”’".contains(n))
        })
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prev = prev[start..].trim_start_matches(['"', '\'', '”', '’']);
    let end = next
        .char_indices()
        .find(|&(_, c)| ends(c))
        .map_or(next.len(), |(i, c)| i + c.len_utf8());
    let text = format!("{}{word}{}", prev, &next[..end]);
    clean_opt(Some(&text.split_whitespace().collect::<Vec<_>>().join(" ")))
}

fn invalid(e: impl std::fmt::Display) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_xpointers() {
        let (p, spine) = xpointer("/body/DocFragment[9]/body/div/div/p[20]/span[1]/text().78");
        assert_eq!(
            p.container_path,
            "/body/DocFragment[9]/body/div/div/p[20]/span[1]/text()"
        );
        assert_eq!(p.offset, 78);
        assert_eq!(spine, Some(8));
        let (p, spine) = xpointer("/body/DocFragment[2]/body/p");
        assert_eq!((p.offset, spine), (0, Some(1)));
    }

    #[test]
    fn finds_the_sentence_around_a_word() {
        // From the sample: KOReader's context runs across sentences.
        let s = sentence(
            Some("ships out of the bay,  and that was the last Asmund ever saw of him.  \nThe "),
            "dragons",
            Some(
                " turned their tails to the low gray moors and the high cloudy sky of Himmerland. With",
            ),
        );
        assert_eq!(
            s.as_deref(),
            Some(
                "The dragons turned their tails to the low gray moors and the high cloudy sky of Himmerland."
            )
        );
        assert_eq!(sentence(None, "word", None).as_deref(), Some("word"));
    }
}
