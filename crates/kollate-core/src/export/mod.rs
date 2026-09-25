//! Exports: Obsidian vault sync, Anki decks, JSON backup, CSV and Readwise.
//! All of them read from the library; none touch the device.

mod anki;
mod files;
mod obsidian;

use serde::Serialize;

pub use anki::{AnkiStats, export_anki};
pub use files::{export_highlights_csv, export_json, export_readwise_csv, export_vocab_csv};
pub use obsidian::{ObsidianStats, sync_obsidian};

use crate::Result;
use crate::store::{Annotation, AnnotationFilter, Book, Library, View, VocabDetail, VocabStatus};

#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    /// Also export archived highlights (trashed ones never are).
    pub include_archived: bool,
    /// Also export words marked Known (Ignored words never are).
    pub include_known_words: bool,
    /// Everything, including trashed highlights and ignored words (backups).
    pub everything: bool,
}

/// A book with everything that gets exported for it.
#[derive(Debug, Clone, Serialize)]
pub struct ExportBook {
    pub book: Book,
    /// In reading order.
    pub annotations: Vec<Annotation>,
    /// Words looked up in this book; sightings are limited to this book.
    pub vocab: Vec<VocabDetail>,
}

impl Library {
    /// Books with at least one exportable highlight or word.
    pub fn export_books(&self, options: ExportOptions) -> Result<Vec<ExportBook>> {
        let mut out = Vec::new();
        for book in self.books()? {
            let view = match (options.everything, options.include_archived) {
                (true, _) => View::BookAll(book.id),
                (false, true) => View::BookWithArchived(book.id),
                (false, false) => View::Book(book.id),
            };
            let annotations = self.query_annotations(&AnnotationFilter {
                view,
                search: None,
                id: None,
            })?;
            let mut vocab = Vec::new();
            for v in self.query_vocab(Some(book.id), None)? {
                let wanted = match v.status {
                    _ if options.everything => true,
                    VocabStatus::Ignored => false,
                    VocabStatus::Known => options.include_known_words,
                    _ => true,
                };
                if !wanted {
                    continue;
                }
                if let Some(mut detail) = self.vocab_detail(v.id)? {
                    detail.sightings.retain(|s| s.book_id == Some(book.id));
                    vocab.push(detail);
                }
            }
            vocab.sort_by_key(|d| d.vocab.first_seen_at);
            if !annotations.is_empty() || !vocab.is_empty() {
                out.push(ExportBook {
                    book,
                    annotations,
                    vocab,
                });
            }
        }
        Ok(out)
    }
}

/// A file name safe on every filesystem Obsidian syncs to.
pub(crate) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|#^[]".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = cleaned.trim_matches('.').chars().take(120).collect();
    if trimmed.is_empty() {
        "Untitled".to_owned()
    } else {
        trimmed.trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_file_name;

    #[test]
    fn makes_safe_file_names() {
        assert_eq!(
            safe_file_name("Hellenic Tantra: The Theurgic Platonism"),
            "Hellenic Tantra The Theurgic Platonism"
        );
        assert_eq!(safe_file_name("A/B #1 [x]?"), "A B 1 x");
        assert_eq!(safe_file_name("..."), "Untitled");
    }
}
