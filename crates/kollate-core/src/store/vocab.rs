//! Vocabulary: listing, enrichment (context sentences and definitions) and
//! merging inflected forms of the same word.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::{Library, query::like_pattern};
use crate::Result;
use crate::dict::{Dictionary, fold, lookup_any};
use crate::kobo::DeviceInfo;
use crate::kobo::epub::WordContexts;
use crate::store::VocabStatus;

#[derive(Debug, Clone, Serialize)]
pub struct Vocab {
    pub id: i64,
    pub word: String,
    pub language: String,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub status: VocabStatus,
    /// Dictionary form, once looked up.
    pub lemma: Option<String>,
    pub definition: Option<String>,
    pub definition_source: Option<String>,
    /// The chosen context sentence of the earliest lookup, if any.
    pub context: Option<String>,
    /// Titles of the books the word was looked up in.
    pub books: Vec<String>,
}

/// One lookup of a word in a book.
#[derive(Debug, Clone, Serialize)]
pub struct Sighting {
    pub id: i64,
    pub book_id: Option<i64>,
    pub book_title: Option<String>,
    /// The word exactly as it appeared.
    pub surface_form: String,
    pub looked_up_at: Option<DateTime<Utc>>,
    pub context: Option<String>,
    /// Sentences from the book that contain the word, best guess first.
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VocabDetail {
    pub vocab: Vocab,
    pub sightings: Vec<Sighting>,
}

const VOCAB_SELECT: &str = "SELECT v.id, v.word, v.language, v.first_seen_at, v.status, v.lemma, v.definition,
        v.definition_source,
        (SELECT s.context_sentence FROM vocab_sighting s WHERE s.vocab_id = v.id AND s.context_sentence IS NOT NULL
         ORDER BY s.looked_up_at LIMIT 1),
        (SELECT group_concat(title, char(31)) FROM (SELECT DISTINCT coalesce(b.user_title, b.title) AS title
         FROM vocab_sighting s JOIN book b ON b.id = s.book_id WHERE s.vocab_id = v.id))
     FROM vocab v";

fn vocab_from_row(r: &rusqlite::Row) -> rusqlite::Result<Vocab> {
    Ok(Vocab {
        id: r.get(0)?,
        word: r.get(1)?,
        language: r.get(2)?,
        first_seen_at: r.get(3)?,
        status: VocabStatus::parse(&r.get::<_, String>(4)?),
        lemma: r.get(5)?,
        definition: r.get(6)?,
        definition_source: r.get(7)?,
        context: r.get(8)?,
        books: r
            .get::<_, Option<String>>(9)?
            .map(|s| s.split('\u{1f}').map(str::to_owned).collect())
            .unwrap_or_default(),
    })
}

impl Library {
    pub fn vocab(&self) -> Result<Vec<Vocab>> {
        self.query_vocab(None, None)
    }

    /// Vocab words, newest first, optionally limited to one book and/or
    /// matching a search string (word, lemma, definition or context).
    pub fn query_vocab(&self, book_id: Option<i64>, search: Option<&str>) -> Result<Vec<Vocab>> {
        let pattern = search
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(like_pattern);
        let mut stmt = self.conn.prepare(&format!(
            "{VOCAB_SELECT}
             WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM vocab_sighting s WHERE s.vocab_id = v.id AND s.book_id = ?1))
               AND (?2 IS NULL OR v.word LIKE ?2 ESCAPE '\\' OR v.lemma LIKE ?2 ESCAPE '\\'
                    OR v.definition LIKE ?2 ESCAPE '\\'
                    OR EXISTS (SELECT 1 FROM vocab_sighting s WHERE s.vocab_id = v.id
                               AND s.context_sentence LIKE ?2 ESCAPE '\\'))
             ORDER BY v.first_seen_at DESC, v.id DESC"
        ))?;
        Ok(stmt
            .query_map(params![book_id, pattern], vocab_from_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn vocab_detail(&self, id: i64) -> Result<Option<VocabDetail>> {
        let Some(vocab) = self
            .conn
            .query_row(
                &format!("{VOCAB_SELECT} WHERE v.id = ?1"),
                [id],
                vocab_from_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT s.id, coalesce(b.user_title, b.title), s.surface_form, s.looked_up_at, s.context_sentence,
                    s.context_candidates, s.book_id
             FROM vocab_sighting s LEFT JOIN book b ON b.id = s.book_id
             WHERE s.vocab_id = ?1 ORDER BY s.looked_up_at, s.id",
        )?;
        let sightings = stmt
            .query_map([id], |r| {
                Ok(Sighting {
                    id: r.get(0)?,
                    book_title: r.get(1)?,
                    surface_form: r.get(2)?,
                    looked_up_at: r.get(3)?,
                    context: r.get(4)?,
                    candidates: r
                        .get::<_, Option<String>>(5)?
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    book_id: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(VocabDetail { vocab, sightings }))
    }

    /// Stores context candidates found in a device's books. The first
    /// candidate becomes the context unless one was already chosen.
    pub fn set_word_contexts(&self, device: &DeviceInfo, found: &[WordContexts]) -> Result<usize> {
        let mut updated = 0;
        for wc in found.iter().filter(|wc| !wc.contexts.is_empty()) {
            let sentences: Vec<&str> = wc.contexts.iter().map(|c| c.sentence.as_str()).collect();
            updated += self.conn.execute(
                "UPDATE vocab_sighting SET context_candidates = ?4,
                        context_sentence = coalesce(context_sentence, ?5)
                 WHERE surface_form = ?3 AND book_id = (
                     SELECT s.book_id FROM book_source s JOIN device d ON d.id = s.device_id
                     WHERE d.serial = ?1 AND s.volume_id = ?2)",
                params![
                    device.serial,
                    wc.volume_id,
                    wc.word,
                    serde_json::to_string(&sentences).expect("serialize"),
                    sentences[0]
                ],
            )?;
        }
        Ok(updated)
    }

    /// Looks up words without a definition and merges words that share a
    /// dictionary form (e.g. `theophany` and `theophanies`). Returns the
    /// number of words defined.
    pub fn enrich_definitions(&mut self, dictionaries: &[Dictionary]) -> Result<usize> {
        let pending: Vec<(i64, String, String)> = self
            .conn
            .prepare("SELECT id, word, language FROM vocab WHERE definition IS NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut defined = 0;
        for (id, word, language) in pending {
            let Some(def) = lookup_any(dictionaries, &word, &language)? else {
                continue;
            };
            self.conn.execute(
                "UPDATE vocab SET lemma = ?2, definition = ?3, definition_source = ?4, updated_at = ?5 WHERE id = ?1",
                params![id, def.headword, def.to_text(4), def.source, Utc::now()],
            )?;
            defined += 1;
        }
        self.merge_by_lemma()?;
        Ok(defined)
    }

    /// Looks up again every word whose definition came from a dictionary
    /// (never ones the user wrote or edited), e.g. after the installed
    /// dictionaries change. Returns the number of definitions that changed.
    pub fn refresh_definitions(&mut self, dictionaries: &[Dictionary]) -> Result<usize> {
        let words: Vec<(i64, String, String, Option<String>)> = self
            .conn
            .prepare(
                "SELECT id, word, language, definition FROM vocab
                 WHERE definition_source IS NOT NULL AND definition_source != 'edited'",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut changed = 0;
        for (id, word, language, old) in words {
            let Some(def) = lookup_any(dictionaries, &word, &language)? else {
                continue;
            };
            let text = def.to_text(4);
            if old.as_deref() != Some(text.as_str()) {
                self.conn.execute(
                    "UPDATE vocab SET lemma = ?2, definition = ?3, definition_source = ?4, updated_at = ?5 WHERE id = ?1",
                    params![id, def.headword, text, def.source, Utc::now()],
                )?;
                changed += 1;
            }
        }
        self.merge_by_lemma()?;
        Ok(changed)
    }

    /// Merges words with the same lemma and language into the earliest one.
    fn merge_by_lemma(&mut self) -> Result<()> {
        let rows: Vec<(i64, String, String, String)> = self
            .conn
            .prepare("SELECT id, lemma, language, status FROM vocab WHERE lemma IS NOT NULL ORDER BY first_seen_at, id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut groups: HashMap<(String, String), Vec<(i64, String)>> = HashMap::new();
        for (id, lemma, language, status) in rows {
            groups
                .entry((fold(&lemma), language))
                .or_default()
                .push((id, status));
        }
        let tx = self.conn.transaction()?;
        for members in groups.values().filter(|m| m.len() > 1) {
            let keep = members[0].0;
            // Keep the most advanced learning status of the group.
            let rank = |s: &str| {
                ["new", "learning", "known", "ignored"]
                    .iter()
                    .position(|x| *x == s)
                    .unwrap_or(0)
            };
            let status = members
                .iter()
                .map(|(_, s)| s.as_str())
                .max_by_key(|s| rank(s))
                .unwrap_or("new");
            for (other, _) in &members[1..] {
                tx.execute(
                    "UPDATE OR IGNORE vocab_sighting SET vocab_id = ?1 WHERE vocab_id = ?2",
                    params![keep, other],
                )?;
                tx.execute(
                    "UPDATE OR IGNORE vocab_form SET vocab_id = ?1 WHERE vocab_id = ?2",
                    params![keep, other],
                )?;
                tx.execute("INSERT OR IGNORE INTO vocab_tag SELECT ?1, tag_id FROM vocab_tag WHERE vocab_id = ?2", params![keep, other])?;
                tx.execute(
                    "UPDATE vocab SET first_seen_at = min(first_seen_at, (SELECT first_seen_at FROM vocab WHERE id = ?2))
                     WHERE id = ?1",
                    params![keep, other],
                )?;
                tx.execute("DELETE FROM vocab WHERE id = ?1", [other])?;
            }
            tx.execute(
                "UPDATE vocab SET status = ?2 WHERE id = ?1",
                params![keep, status],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets (or with `None`, clears so it's looked up again) a definition.
    pub fn set_vocab_definition(&self, id: i64, definition: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE vocab SET definition = ?2, definition_source = CASE WHEN ?2 IS NULL THEN NULL ELSE 'edited' END,
                    updated_at = ?3 WHERE id = ?1",
            params![id, definition, Utc::now()],
        )?;
        Ok(())
    }

    pub fn set_sighting_context(&self, sighting_id: i64, context: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE vocab_sighting SET context_sentence = ?2 WHERE id = ?1",
            params![sighting_id, context],
        )?;
        Ok(())
    }
}
