//! Plain file exports: JSON (full backup), CSV and Readwise CSV.

use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use super::{ExportBook, ExportOptions};
use crate::Result;
use crate::kobo::color_name;
use crate::store::Library;

fn csv_err(e: csv::Error) -> crate::Error {
    std::io::Error::other(e.to_string()).into()
}

#[derive(Serialize)]
struct Backup {
    format: &'static str,
    version: u32,
    exported_at: String,
    books: Vec<ExportBook>,
}

/// Everything in the library (including trashed and ignored items) as JSON.
pub fn export_json(lib: &Library, out: &Path) -> Result<usize> {
    let books = lib.export_books(ExportOptions {
        everything: true,
        ..Default::default()
    })?;
    let count = books.len();
    let backup = Backup {
        format: "kollate-backup",
        version: 1,
        exported_at: Utc::now().to_rfc3339(),
        books,
    };
    std::fs::write(
        out,
        serde_json::to_vec_pretty(&backup).map_err(std::io::Error::other)?,
    )?;
    Ok(count)
}

/// One row per highlight or note.
pub fn export_highlights_csv(lib: &Library, out: &Path, options: ExportOptions) -> Result<usize> {
    let mut w = csv::Writer::from_path(out).map_err(csv_err)?;
    w.write_record([
        "Book",
        "Author",
        "Chapter",
        "Highlight",
        "Note",
        "Color",
        "Tags",
        "Created",
        "Status",
        "Starred",
    ])
    .map_err(csv_err)?;
    let mut n = 0;
    for eb in lib.export_books(options)? {
        for a in &eb.annotations {
            w.write_record([
                eb.book.title.as_str(),
                eb.book.author.as_deref().unwrap_or(""),
                a.chapter_title.as_deref().unwrap_or(""),
                a.text().unwrap_or(""),
                a.note().unwrap_or(""),
                color_name(a.color),
                &a.tags.join(", "),
                &a.created_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                a.status.as_str(),
                if a.starred { "yes" } else { "" },
            ])
            .map_err(csv_err)?;
            n += 1;
        }
    }
    w.flush()?;
    Ok(n)
}

/// One row per word and book it was looked up in.
pub fn export_vocab_csv(lib: &Library, out: &Path, options: ExportOptions) -> Result<usize> {
    let mut w = csv::Writer::from_path(out).map_err(csv_err)?;
    w.write_record([
        "Word",
        "Lemma",
        "Definition",
        "Context",
        "Book",
        "Looked Up",
        "Status",
    ])
    .map_err(csv_err)?;
    let mut n = 0;
    for eb in lib.export_books(options)? {
        for v in &eb.vocab {
            let sighting = v.sightings.first();
            w.write_record([
                v.vocab.word.as_str(),
                v.vocab.lemma.as_deref().unwrap_or(""),
                v.vocab.definition.as_deref().unwrap_or(""),
                sighting.and_then(|s| s.context.as_deref()).unwrap_or(""),
                eb.book.title.as_str(),
                &sighting
                    .and_then(|s| s.looked_up_at)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                v.vocab.status.as_str(),
            ])
            .map_err(csv_err)?;
            n += 1;
        }
    }
    w.flush()?;
    Ok(n)
}

/// Readwise's CSV import format. Tags become Readwise inline tags (`.tag`)
/// in the note.
pub fn export_readwise_csv(lib: &Library, out: &Path, options: ExportOptions) -> Result<usize> {
    let mut w = csv::Writer::from_path(out).map_err(csv_err)?;
    w.write_record([
        "Highlight",
        "Title",
        "Author",
        "URL",
        "Note",
        "Location",
        "Date",
    ])
    .map_err(csv_err)?;
    let mut n = 0;
    for eb in lib.export_books(options)? {
        for a in eb.annotations.iter().filter(|a| a.text().is_some()) {
            let mut note = a.note().unwrap_or("").to_owned();
            let tags: Vec<String> = a
                .tags
                .iter()
                .map(|t| format!(".{}", t.split_whitespace().collect::<Vec<_>>().join("-")))
                .collect();
            if !tags.is_empty() {
                note = format!("{} {}", tags.join(" "), note).trim().to_owned();
            }
            w.write_record([
                a.text().unwrap_or(""),
                eb.book.title.as_str(),
                eb.book.author.as_deref().unwrap_or(""),
                "",
                &note,
                "",
                &a.created_at
                    .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_default(),
            ])
            .map_err(csv_err)?;
            n += 1;
        }
    }
    w.flush()?;
    Ok(n)
}
