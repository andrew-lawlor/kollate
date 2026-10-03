//! Handwriting transcription state (SPEC §8a): the book's words near each
//! markup, and which markups and notebook pages still need reading by the
//! current model.

use std::path::PathBuf;

use rusqlite::params;

use super::Library;
use crate::Result;
use crate::kobo::DeviceInfo;
use crate::markup::marks::take_marks;
use crate::markup::{Context, PageWord, Reader, Transcription, read_page, transcribe};
use crate::store::apply_pen_marks;

/// A markup or notebook page to transcribe: its copied ink and page, and
/// the book's words around it when they were saved at import.
#[derive(Debug, Clone)]
pub struct MarkupJob {
    pub annotation_id: i64,
    /// A notebook page: all its writing is read, as the page's text.
    pub page: bool,
    pub svg: PathBuf,
    pub jpg: Option<PathBuf>,
    pub words: Option<Vec<String>>,
    /// The page's words with their boxes, from KOReader's Pencil export
    /// (`<ink>.words.json` beside the ink): marks are resolved from them.
    pub page_words: Option<PathBuf>,
    /// What the transcription was made from; see [`Library::pending_transcriptions`].
    pub hash: String,
}

impl MarkupJob {
    /// Reads the markup with `reader`. Runs anywhere (no database access), so
    /// a slow model can work on a background thread. `known_word` enables
    /// correcting misread names (see [`Context::known_word`]).
    pub fn run(
        &self,
        reader: &mut dyn Reader,
        known_word: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Transcription> {
        let svg = std::fs::read_to_string(&self.svg)?;
        if self.page {
            return Ok(Transcription {
                text: read_page(&svg, reader)?,
                ..Default::default()
            });
        }
        let page = self.jpg.as_ref().and_then(|p| std::fs::read(p).ok());
        let page_words: Option<Vec<PageWord>> = self
            .page_words
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok());
        let context = Context {
            page_jpeg: page.as_deref(),
            page_words: page_words.as_deref(),
            book_words: self.words.as_deref(),
            known_word,
        };
        transcribe(&svg, context, reader)
    }
}

/// Where a markup's page words are kept: beside its ink, `<ink>.words.json`.
pub fn page_words_path(svg: &std::path::Path) -> PathBuf {
    svg.with_extension("words.json")
}

impl Library {
    /// Saves the book's words near each markup (from
    /// [`markup_words`](crate::kobo::epub::markup_words)), keyed by bookmark ID.
    pub fn set_markup_contexts(
        &self,
        device: &DeviceInfo,
        found: &[(String, Vec<String>)],
    ) -> Result<usize> {
        let mut updated = 0;
        for (bookmark_id, words) in found.iter().filter(|(_, w)| !w.is_empty()) {
            updated += self.conn.execute(
                "UPDATE annotation SET markup_context = ?3
                 WHERE id = (SELECT x.annotation_id FROM annotation_source x JOIN device d ON d.id = x.device_id
                             WHERE d.serial = ?1 AND x.bookmark_id = ?2)
                   AND markup_context IS NOT ?3",
                params![device.serial, bookmark_id, serde_json::to_string(words).expect("serialize")],
            )?;
        }
        Ok(updated)
    }

    /// Markups and notebook pages whose transcription is missing or out of
    /// date for `source` (the model in use): new ink, a page or book text
    /// that arrived since, or a different model. Trashed ones are skipped.
    pub fn pending_transcriptions(&self, source: &str) -> Result<Vec<MarkupJob>> {
        // (id, is a page, ink SVG, page JPG, book words, hash of the saved transcription)
        type Row = (
            i64,
            bool,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = self
            .conn
            .prepare(
                "SELECT id, kind = 'page', markup_svg_path, markup_jpg_path, markup_context, ink_hash
                 FROM annotation
                 WHERE kind IN ('markup', 'page') AND markup_svg_path IS NOT NULL AND status != 'trashed'
                 ORDER BY created_at",
            )?
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut jobs = Vec::new();
        for (annotation_id, page, svg, jpg, context, done) in rows {
            let svg = PathBuf::from(svg);
            let Ok(ink) = std::fs::read(&svg) else {
                continue;
            };
            let jpg = jpg.map(PathBuf::from).filter(|p| p.is_file());
            let page_words = Some(page_words_path(&svg)).filter(|p| p.is_file());
            let mut h = blake3::Hasher::new();
            // Bumped when reading changes in a way worth reading again for
            // (2: loops, stars and question marks known by shape, 0.4;
            // 3: marked passages in page order, overshoot trimmed;
            // 4: readings that loop are dropped).
            h.update(b"reading 4");
            h.update(&ink);
            h.update(&[u8::from(jpg.is_some()), u8::from(context.is_some())]);
            h.update(source.as_bytes());
            // Only KOReader markups have page words, so Nickel's keep their hash.
            if page_words.is_some() {
                h.update(b"page words");
            }
            let hash = h.finalize().to_hex()[..32].to_owned();
            if done.as_deref() != Some(hash.as_str()) {
                jobs.push(MarkupJob {
                    annotation_id,
                    page,
                    svg,
                    jpg,
                    words: context.and_then(|c| serde_json::from_str(&c).ok()),
                    page_words,
                    hash,
                });
            }
        }
        Ok(jobs)
    }

    /// Stores a transcription. The user's own text and note are untouched,
    /// and still shown in its place. Pen marks (SPEC §8c) are taken out of
    /// the note (or a page's text) and applied once; circled words go to
    /// Vocabulary. Returns how many words were added there.
    pub fn save_transcription(
        &self,
        job: &MarkupJob,
        t: &Transcription,
        source: &str,
    ) -> Result<usize> {
        let (text, note, marks) = if job.page {
            let (text, marks) = t.text.as_deref().map(take_marks).unwrap_or_default();
            (text, None, marks)
        } else {
            let (note, marks) = t.note.as_deref().map(take_marks).unwrap_or_default();
            (t.text.clone(), note, marks)
        };
        self.conn.execute(
            "UPDATE annotation SET ink_text = ?2, ink_note = ?3, ink_source = ?4, ink_hash = ?5
             WHERE id = ?1",
            params![job.annotation_id, text, note, source, job.hash],
        )?;
        apply_pen_marks(&self.conn, job.annotation_id, &marks)?;
        self.add_glosses(job.annotation_id, &t.circled, note.as_deref())
    }
}
