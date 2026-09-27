//! Handwriting transcription state (SPEC §8a): the book's words near each
//! markup, and which markups still need reading by the current model.

use std::path::PathBuf;

use rusqlite::params;

use super::Library;
use crate::Result;
use crate::kobo::DeviceInfo;
use crate::markup::{Context, Reader, Transcription, transcribe};

/// A markup to transcribe: its copied ink and page, and the book's words
/// around it when they were saved at import.
#[derive(Debug, Clone)]
pub struct MarkupJob {
    pub annotation_id: i64,
    pub svg: PathBuf,
    pub jpg: Option<PathBuf>,
    pub words: Option<Vec<String>>,
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
        let page = self.jpg.as_ref().and_then(|p| std::fs::read(p).ok());
        let context = Context {
            page_jpeg: page.as_deref(),
            book_words: self.words.as_deref(),
            known_word,
        };
        transcribe(&svg, context, reader)
    }
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

    /// Markups whose transcription is missing or out of date for `source`
    /// (the model in use): new ink, a page or book text that arrived since,
    /// or a different model. Trashed markups are skipped.
    pub fn pending_transcriptions(&self, source: &str) -> Result<Vec<MarkupJob>> {
        // (id, ink SVG, page JPG, book words, hash of the saved transcription)
        type Row = (i64, String, Option<String>, Option<String>, Option<String>);
        let rows: Vec<Row> = self
            .conn
            .prepare(
                "SELECT id, markup_svg_path, markup_jpg_path, markup_context, ink_hash FROM annotation
                 WHERE kind = 'markup' AND markup_svg_path IS NOT NULL AND status != 'trashed'
                 ORDER BY created_at",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut jobs = Vec::new();
        for (annotation_id, svg, jpg, context, done) in rows {
            let svg = PathBuf::from(svg);
            let Ok(ink) = std::fs::read(&svg) else {
                continue;
            };
            let jpg = jpg.map(PathBuf::from).filter(|p| p.is_file());
            let mut h = blake3::Hasher::new();
            h.update(&ink);
            h.update(&[u8::from(jpg.is_some()), u8::from(context.is_some())]);
            h.update(source.as_bytes());
            let hash = h.finalize().to_hex()[..32].to_owned();
            if done.as_deref() != Some(hash.as_str()) {
                jobs.push(MarkupJob {
                    annotation_id,
                    svg,
                    jpg,
                    words: context.and_then(|c| serde_json::from_str(&c).ok()),
                    hash,
                });
            }
        }
        Ok(jobs)
    }

    /// Stores a transcription. The user's own text and note are untouched,
    /// and still shown in its place.
    pub fn save_transcription(
        &self,
        job: &MarkupJob,
        t: &Transcription,
        source: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE annotation SET ink_text = ?2, ink_note = ?3, ink_source = ?4, ink_hash = ?5
             WHERE id = ?1",
            params![job.annotation_id, t.text, t.note, source, job.hash],
        )?;
        Ok(())
    }
}
