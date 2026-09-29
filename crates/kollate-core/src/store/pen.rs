//! Pen marks and glosses (SPEC §8c): what the reader asked for in the
//! margin, applied once, and circled words added to Vocabulary.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;

use super::Library;
use crate::Result;
use crate::markup::marks::PenMarks;
use crate::markup::snap_similarity;

/// Setting: whether circled words go to Vocabulary ("0" turns it off).
pub const GLOSSES_SETTING: &str = "circled_to_vocabulary";

/// Applies the marks not applied to annotation `id` before, and remembers
/// them, so a star or tag the user took away in Kollate stays away when the
/// note is read again. A star also keeps it, as starring in the Inbox does.
/// Returns whether anything changed.
pub(crate) fn apply_pen_marks(conn: &Connection, id: i64, marks: &PenMarks) -> Result<bool> {
    if marks.is_empty() {
        return Ok(false);
    }
    let applied: Option<String> = conn.query_row(
        "SELECT pen_marks FROM annotation WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let mut applied: PenMarks = applied
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let mut changed = false;
    if marks.star && !applied.star {
        conn.execute(
            "UPDATE annotation SET starred = 1,
                    status = CASE WHEN status = 'inbox' THEN 'kept' ELSE status END, updated_at = ?2
             WHERE id = ?1",
            params![id, Utc::now()],
        )?;
        applied.star = true;
        changed = true;
    }
    for tag in &marks.tags {
        if applied.tags.contains(tag) {
            continue;
        }
        let name = tag_name(conn, tag)?;
        conn.execute(
            "INSERT INTO tag (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
            [&name],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO annotation_tag (annotation_id, tag_id) SELECT ?1, id FROM tag WHERE name = ?2",
            params![id, name],
        )?;
        applied.tags.push(tag.clone());
        changed = true;
    }
    if changed {
        conn.execute(
            "UPDATE annotation SET pen_marks = ?2 WHERE id = ?1",
            params![
                id,
                serde_json::to_string(&applied).expect("marks serialize")
            ],
        )?;
    }
    Ok(changed)
}

/// The tag a written `#tag` means: an existing tag of that name, or one it's
/// a likely misreading or typo of (four letters or more, 0.8 similar, so
/// one letter off), or itself.
fn tag_name(conn: &Connection, tag: &str) -> Result<String> {
    let names: Vec<String> = conn
        .prepare("SELECT name FROM tag")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if let Some(same) = names.iter().find(|n| n.eq_ignore_ascii_case(tag)) {
        return Ok(same.clone());
    }
    if tag.chars().count() >= 4 {
        let close = names
            .iter()
            .map(|n| (snap_similarity(&n.to_lowercase(), tag), n))
            .filter(|(s, _)| *s >= 0.8)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, n)) = close {
            return Ok(n.clone());
        }
    }
    Ok(tag.to_owned())
}

impl Library {
    /// Adds words circled in markup `annotation_id` to Vocabulary, with the
    /// book's sentence around each. With one word circled, `gloss` (the note
    /// written beside it) becomes the word's own note if it has none; with
    /// several, which one a note is about can't be told. Each word is added
    /// once per markup. Returns how many words were added.
    pub fn add_glosses(
        &self,
        annotation_id: i64,
        words: &[String],
        gloss: Option<&str>,
    ) -> Result<usize> {
        if words.is_empty() || self.setting(GLOSSES_SETTING)?.as_deref() == Some("0") {
            return Ok(0);
        }
        // The markup's book, when it was made, where it came from, and the
        // book's words around it (saved at import).
        type Row = (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        );
        let Some((book_id, created, context, language, device_id)): Option<Row> = self
            .conn
            .query_row(
                "SELECT a.book_id, a.created_at, a.markup_context, b.language,
                        (SELECT device_id FROM annotation_source WHERE annotation_id = a.id LIMIT 1)
                 FROM annotation a JOIN book b ON b.id = a.book_id WHERE a.id = ?1",
                [annotation_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
        else {
            return Ok(0);
        };
        let Some(device_id) = device_id else {
            return Ok(0);
        };
        let book_words: Vec<String> = context
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
        // "en" from "en-US"; Kobo's lookups are keyed the same way.
        let language = language
            .map(|l| l.split(['-', '_']).next().unwrap_or("").to_lowercase())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "en".to_owned());
        let now = Utc::now();
        let mut added = 0;
        for word in words {
            let key: String = word.nfc().collect::<String>().to_lowercase();
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT vocab_id FROM vocab_form WHERE key = ?1 AND language = ?2",
                    params![key, language],
                    |r| r.get(0),
                )
                .optional()?;
            let vocab_id = match existing {
                Some(id) => id,
                None => {
                    let id: i64 = self.conn.query_row(
                        "INSERT INTO vocab (word, key, language, first_seen_at, imported_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
                        params![word, key, language, created, now],
                        |r| r.get(0),
                    )?;
                    self.conn.execute(
                        "INSERT OR IGNORE INTO vocab_form (key, language, vocab_id) VALUES (?1, ?2, ?3)",
                        params![key, language, id],
                    )?;
                    id
                }
            };
            let sentence = crate::kobo::epub::sentence_near(&book_words, word);
            let inserted = self.conn.execute(
                "INSERT INTO vocab_sighting (vocab_id, book_id, device_id, surface_form, looked_up_at,
                        context_sentence, annotation_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT DO NOTHING",
                params![vocab_id, book_id, device_id, word, created, sentence, annotation_id],
            )?;
            added += inserted;
            if let Some(gloss) = gloss.filter(|g| words.len() == 1 && !g.trim().is_empty()) {
                self.conn.execute(
                    "UPDATE vocab SET user_note = ?2, updated_at = ?3 WHERE id = ?1 AND user_note IS NULL",
                    params![vocab_id, gloss.trim(), now],
                )?;
            }
        }
        Ok(added)
    }
}
