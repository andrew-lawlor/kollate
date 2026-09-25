//! Merges a [`KoboSnapshot`] into the [`Library`] without creating
//! duplicates. See SPEC §6 for the rules. In short:
//! - annotations match by Kobo `BookmarkID`, then by content fingerprint
//!   (catches factory resets and second devices);
//! - device changes update the `device_*` columns only, so user edits win;
//! - annotations that vanish from a device are flagged, never deleted.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::Result;
use crate::kobo::{AnnotationKind, DeviceInfo, KoboBook, KoboBookmark, KoboSnapshot, KoboWord};
use crate::normalize::{annotation_fingerprint, book_fingerprint, markup_fingerprint};
use crate::store::Library;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportStats {
    pub books_new: usize,
    pub annotations_new: usize,
    pub annotations_updated: usize,
    pub annotations_unchanged: usize,
    /// Present in the library from this device but gone from it now.
    pub annotations_removed: usize,
    /// Previously flagged as removed and now back on the device.
    pub annotations_restored: usize,
    /// Page bookmarks (dogears) and unknown kinds, which aren't imported.
    pub annotations_skipped: usize,
    pub words_new: usize,
    pub word_sightings_new: usize,
}

impl ImportStats {
    pub fn is_empty(&self) -> bool {
        self.books_new
            + self.annotations_new
            + self.annotations_updated
            + self.annotations_removed
            + self.annotations_restored
            + self.words_new
            + self.word_sightings_new
            == 0
    }
}

impl Library {
    /// Imports `snapshot` read from `device`. With `dry_run`, everything is
    /// computed and then rolled back.
    pub fn import(
        &mut self,
        snapshot: &KoboSnapshot,
        device: &DeviceInfo,
        dry_run: bool,
    ) -> Result<ImportStats> {
        let started = Utc::now();
        let tx = self.conn.transaction()?;
        let mut importer = Importer {
            tx: &tx,
            now: started,
            device_id: 0,
            stats: ImportStats::default(),
        };
        importer.device_id = importer.upsert_device(device)?;
        importer.run(snapshot)?;
        let stats = importer.stats;
        tx.execute(
            "INSERT INTO import_run (device_id, started_at, finished_at, db_version, stats_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                importer.device_id,
                started,
                Utc::now(),
                snapshot.db_version,
                serde_json::to_string(&stats).expect("stats serialize")
            ],
        )?;
        if dry_run {
            tx.rollback()?;
        } else {
            tx.commit()?;
        }
        Ok(stats)
    }
}

struct Importer<'a> {
    tx: &'a Transaction<'a>,
    now: DateTime<Utc>,
    device_id: i64,
    stats: ImportStats,
}

/// A book row as needed while importing.
#[derive(Clone)]
struct BookRef {
    id: i64,
    fingerprint: String,
}

/// Device-owned annotation fields, compared to detect changes.
#[derive(PartialEq)]
struct DeviceFields {
    kind: String,
    text: Option<String>,
    note: Option<String>,
    color: i64,
}

fn kind_str(kind: &AnnotationKind) -> Option<&'static str> {
    match kind {
        AnnotationKind::Highlight => Some("highlight"),
        AnnotationKind::Note => Some("note"),
        AnnotationKind::Markup => Some("markup"),
        AnnotationKind::Dogear | AnnotationKind::Other(_) => None,
    }
}

/// Case-insensitive key that groups lookups of the same word.
fn vocab_key(word: &str) -> String {
    word.nfc().collect::<String>().to_lowercase()
}

impl Importer<'_> {
    fn upsert_device(&self, d: &DeviceInfo) -> Result<i64> {
        Ok(self.tx.query_row(
            "INSERT INTO device (serial, model_id, firmware, first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT (serial) DO UPDATE SET
                model_id = coalesce(excluded.model_id, model_id),
                firmware = coalesce(excluded.firmware, firmware),
                last_seen_at = excluded.last_seen_at
             RETURNING id",
            params![d.serial, d.model_id, d.firmware, self.now],
            |r| r.get(0),
        )?)
    }

    fn run(&mut self, s: &KoboSnapshot) -> Result<()> {
        let mut books = HashMap::new();
        for b in &s.books {
            books.insert(b.volume_id.as_str(), self.upsert_book(b)?);
        }

        let mut seen = HashSet::new();
        for bm in &s.bookmarks {
            let Some(book) = books.get(bm.volume_id.as_str()) else {
                self.stats.annotations_skipped += 1;
                continue;
            };
            match self.merge_annotation(bm, book, &seen)? {
                Some(id) => {
                    seen.insert(id);
                }
                None => self.stats.annotations_skipped += 1,
            }
        }
        self.flag_removed(&seen)?;

        for w in &s.words {
            let book = w.volume_id.as_deref().and_then(|v| books.get(v));
            self.merge_word(w, book.map(|b| b.id))?;
        }
        Ok(())
    }

    fn upsert_book(&mut self, b: &KoboBook) -> Result<BookRef> {
        let existing: Option<(i64, String)> = self
            .tx
            .query_row(
                "SELECT b.id, b.fingerprint FROM book_source s JOIN book b ON b.id = s.book_id
                 WHERE s.device_id = ?1 AND s.volume_id = ?2",
                params![self.device_id, b.volume_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let fingerprint = book_fingerprint(&b.title, b.author.as_deref());
        let (id, fingerprint) = match existing {
            Some(found) => found,
            None => {
                let by_fp: Option<i64> = self
                    .tx
                    .query_row(
                        "SELECT id FROM book WHERE fingerprint = ?1",
                        [&fingerprint],
                        |r| r.get(0),
                    )
                    .optional()?;
                let id = match by_fp {
                    Some(id) => id,
                    None => {
                        self.stats.books_new += 1;
                        self.tx.query_row(
                            "INSERT INTO book (fingerprint, title, author, created_at, updated_at)
                             VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
                            params![fingerprint, b.title, b.author, self.now],
                            |r| r.get(0),
                        )?
                    }
                };
                self.tx.execute(
                    "INSERT INTO book_source (device_id, volume_id, book_id, image_id) VALUES (?1, ?2, ?3, ?4)",
                    params![self.device_id, b.volume_id, id, b.image_id],
                )?;
                (id, fingerprint)
            }
        };
        // Refresh device-owned metadata; user overrides live in user_* columns.
        self.tx.execute(
            "UPDATE book SET title = ?2, author = ?3, publisher = ?4, isbn = ?5, language = ?6,
                    series = ?7, series_number = ?8, percent_read = ?9, last_read_at = ?10
             WHERE id = ?1",
            params![
                id,
                b.title,
                b.author,
                b.publisher,
                b.isbn,
                b.language,
                b.series,
                b.series_number,
                b.percent_read,
                b.last_read
            ],
        )?;
        Ok(BookRef { id, fingerprint })
    }

    /// Returns the library annotation ID, or `None` if this kind isn't imported.
    fn merge_annotation(
        &mut self,
        bm: &KoboBookmark,
        book: &BookRef,
        seen: &HashSet<i64>,
    ) -> Result<Option<i64>> {
        let Some(kind) = kind_str(&bm.kind) else {
            return Ok(None);
        };
        let fingerprint = match (&bm.kind, bm.text.as_deref()) {
            (AnnotationKind::Markup, _) => Some(markup_fingerprint(
                &book.fingerprint,
                (&bm.start.container_path, bm.start.offset),
                (&bm.end.container_path, bm.end.offset),
                bm.created.map(|c| c.to_rfc3339()).as_deref(),
            )),
            (_, Some(t)) => Some(annotation_fingerprint(
                &book.fingerprint,
                t,
                &bm.start.container_path,
                bm.start.offset,
            )),
            (_, None) => None,
        };

        let by_id: Option<i64> = self
            .tx
            .query_row(
                "SELECT annotation_id FROM annotation_source WHERE bookmark_id = ?1",
                [&bm.bookmark_id],
                |r| r.get(0),
            )
            .optional()?;
        let existing = match (by_id, &fingerprint) {
            (Some(id), _) => Some(id),
            (None, Some(fp)) => {
                // Never match an annotation already claimed in this import.
                let mut stmt = self.tx.prepare_cached(
                    "SELECT id FROM annotation WHERE fingerprint = ?1 ORDER BY id",
                )?;
                let ids = stmt
                    .query_map([fp], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids.into_iter().find(|id| !seen.contains(id))
            }
            (None, None) => None,
        };

        let id = match existing {
            Some(id) => {
                self.update_annotation(id, kind, bm, fingerprint.as_deref())?;
                id
            }
            None => {
                self.stats.annotations_new += 1;
                self.insert_annotation(kind, bm, book.id, fingerprint.as_deref())?
            }
        };

        self.tx.execute(
            "INSERT INTO annotation_source (bookmark_id, device_id, annotation_id, first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT (bookmark_id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
            params![bm.bookmark_id, self.device_id, id, self.now],
        )?;
        Ok(Some(id))
    }

    fn insert_annotation(
        &self,
        kind: &str,
        bm: &KoboBookmark,
        book_id: i64,
        fingerprint: Option<&str>,
    ) -> Result<i64> {
        Ok(self.tx.query_row(
            "INSERT INTO annotation (book_id, fingerprint, kind, device_text, device_note, color, chapter_title,
                    content_id, spine_index, start_path, start_offset, end_path, end_offset, chapter_progress,
                    created_at, device_modified_at, imported_at, updated_at, position_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17, ?18)
             RETURNING id",
            params![
                book_id,
                fingerprint,
                kind,
                bm.text,
                bm.note,
                bm.color,
                bm.chapter_title,
                bm.content_id,
                bm.spine_index,
                bm.start.container_path,
                bm.start.offset,
                bm.end.container_path,
                bm.end.offset,
                bm.chapter_progress,
                bm.created,
                bm.modified,
                self.now,
                bm.reading_order_key().1
            ],
            |r| r.get(0),
        )?)
    }

    fn update_annotation(
        &mut self,
        id: i64,
        kind: &str,
        bm: &KoboBookmark,
        fingerprint: Option<&str>,
    ) -> Result<()> {
        let (old, user_text, user_note, removed): (DeviceFields, Option<String>, Option<String>, Option<String>) =
            self.tx.query_row(
                "SELECT kind, device_text, device_note, color, user_text, user_note, removed_on_device_at
                 FROM annotation WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        DeviceFields { kind: r.get(0)?, text: r.get(1)?, note: r.get(2)?, color: r.get(3)? },
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )?;
        let new = DeviceFields {
            kind: kind.to_owned(),
            text: bm.text.clone(),
            note: bm.note.clone(),
            color: bm.color,
        };

        if removed.is_some() {
            self.stats.annotations_restored += 1;
        }
        if old == new {
            self.stats.annotations_unchanged += 1;
        } else {
            self.stats.annotations_updated += 1;
            let revisions = [
                ("device_text", old.text.clone(), new.text.clone()),
                ("device_note", old.note.clone(), new.note.clone()),
                (
                    "color",
                    Some(old.color.to_string()),
                    Some(new.color.to_string()),
                ),
            ];
            for (field, before, after) in revisions.into_iter().filter(|(_, a, b)| a != b) {
                self.tx.execute(
                    "INSERT INTO annotation_revision (annotation_id, field, old_value, new_value, source, at)
                     VALUES (?1, ?2, ?3, ?4, 'device', ?5)",
                    params![id, field, before, after, self.now],
                )?;
            }
        }
        // The device changed something the user has overridden: flag it for review.
        let conflict = (old.text != new.text && user_text.is_some())
            || (old.note != new.note && user_note.is_some());

        self.tx.execute(
            "UPDATE annotation SET kind = ?2, device_text = ?3, device_note = ?4, color = ?5, chapter_title = ?6,
                    content_id = ?7, spine_index = ?8, start_path = ?9, start_offset = ?10, end_path = ?11,
                    end_offset = ?12, chapter_progress = ?13, device_modified_at = ?14,
                    fingerprint = coalesce(?15, fingerprint), removed_on_device_at = NULL, position_key = ?19,
                    device_changed_at = CASE WHEN ?16 THEN ?17 ELSE device_changed_at END,
                    updated_at = CASE WHEN ?18 THEN ?17 ELSE updated_at END
             WHERE id = ?1",
            params![
                id,
                new.kind,
                new.text,
                new.note,
                new.color,
                bm.chapter_title,
                bm.content_id,
                bm.spine_index,
                bm.start.container_path,
                bm.start.offset,
                bm.end.container_path,
                bm.end.offset,
                bm.chapter_progress,
                bm.modified,
                fingerprint,
                conflict,
                self.now,
                old != new || removed.is_some(),
                bm.reading_order_key().1,
            ],
        )?;
        Ok(())
    }

    /// Flags annotations previously seen on this device that are gone now.
    fn flag_removed(&mut self, seen: &HashSet<i64>) -> Result<()> {
        let mut stmt = self.tx.prepare(
            "SELECT DISTINCT a.id FROM annotation_source s JOIN annotation a ON a.id = s.annotation_id
             WHERE s.device_id = ?1 AND a.removed_on_device_at IS NULL",
        )?;
        let gone: Vec<i64> = stmt
            .query_map([self.device_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in &gone {
            self.tx.execute(
                "UPDATE annotation SET removed_on_device_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![id, self.now],
            )?;
        }
        self.stats.annotations_removed += gone.len();
        Ok(())
    }

    fn merge_word(&mut self, w: &KoboWord, book_id: Option<i64>) -> Result<()> {
        let key = vocab_key(&w.word);
        let language = w.language.clone().unwrap_or_default();
        let existing: Option<i64> = self
            .tx
            .query_row(
                "SELECT vocab_id FROM vocab_form WHERE key = ?1 AND language = ?2",
                params![key, language],
                |r| r.get(0),
            )
            .optional()?;
        let vocab_id = match existing {
            Some(id) => {
                self.tx.execute(
                    "UPDATE vocab SET first_seen_at = min(coalesce(first_seen_at, ?2), coalesce(?2, first_seen_at))
                     WHERE id = ?1",
                    params![id, w.created],
                )?;
                id
            }
            None => {
                self.stats.words_new += 1;
                self.tx.query_row(
                    "INSERT INTO vocab (word, key, language, first_seen_at, imported_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
                    params![w.word, key, language, w.created, self.now],
                    |r| r.get(0),
                )?
            }
        };
        self.tx.execute(
            "INSERT OR IGNORE INTO vocab_form (key, language, vocab_id) VALUES (?1, ?2, ?3)",
            params![key, language, vocab_id],
        )?;
        let inserted = self.tx.execute(
            "INSERT INTO vocab_sighting (vocab_id, book_id, device_id, surface_form, looked_up_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO NOTHING",
            params![vocab_id, book_id, self.device_id, w.word, w.created],
        )?;
        self.stats.word_sightings_new += inserted;
        Ok(())
    }
}
