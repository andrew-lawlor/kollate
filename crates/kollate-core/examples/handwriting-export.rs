//! Exports the library's handwriting for an evaluation: each markup and
//! notebook page as a picture, its ink, page and book words, and a sheet to
//! type what was really written. No model's reading is exported, so the
//! answers are written blind.
//!
//! `cargo run --example handwriting-export -- <out dir> [--library <library.db>] [--trashed]`
//!
//! The library is opened read-only. Open `<out dir>/sheet.html` in a browser;
//! answers are kept in the browser as you type and saved with "Save Answers".

use std::path::{Path, PathBuf};

use kollate_core::markup::image::{RgbImage, compose_page, render_note};
use kollate_core::markup::segment::{self, Bounds, Note, Rotation};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

fn main() {
    if let Err(err) = run() {
        eprintln!("handwriting-export: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut out = None;
    let mut library = None;
    let mut trashed = false;
    let mut only_book: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--library" => library = args.next().map(PathBuf::from),
            "--trashed" => trashed = true,
            "--book" => only_book = args.next().map(|b| b.to_lowercase()),
            _ => out = Some(PathBuf::from(arg)),
        }
    }
    let out =
        out.ok_or("usage: handwriting-export <out dir> [--library <library.db>] [--trashed] [--book <title text>]")?;
    let library = library
        .or_else(default_library)
        .ok_or("no library found; pass --library")?;
    let db = Connection::open_with_flags(&library, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let items_dir = out.join("items");
    std::fs::create_dir_all(&items_dir)?;
    let mut stmt = db.prepare(
        "SELECT a.kind, a.markup_svg_path, a.markup_jpg_path, a.markup_context, a.created_at,
                a.chapter_title, a.status, COALESCE(b.user_title, b.title), b.language,
                EXISTS (SELECT 1 FROM annotation_source x WHERE x.annotation_id = a.id
                        AND x.bookmark_id LIKE 'koreader:%')
         FROM annotation a JOIN book b ON b.id = a.book_id
         WHERE a.kind IN ('markup', 'page') AND a.markup_svg_path IS NOT NULL
         ORDER BY COALESCE(b.user_title, b.title), a.created_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, Option<String>>(8)?,
            r.get::<_, bool>(9)?,
        ))
    })?;

    let mut items = Vec::new();
    let mut skipped = 0;
    for row in rows {
        let (kind, svg_path, jpg_path, context, created, chapter, status, book, language, koreader) =
            row?;
        if status == "trashed" && !trashed {
            continue;
        }
        if only_book
            .as_ref()
            .is_some_and(|b| !book.to_lowercase().contains(b.as_str()))
        {
            continue;
        }
        let svg_path = PathBuf::from(svg_path);
        let Ok(svg) = std::fs::read_to_string(&svg_path) else {
            skipped += 1;
            continue;
        };
        // The Kobo's own ID for the markup or page: stable across libraries.
        let key = svg_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("odd file name")?
            .to_owned();
        let page = kind == "page";
        let jpg = jpg_path
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .and_then(|p| std::fs::read(p).ok());

        std::fs::write(items_dir.join(format!("{key}.svg")), &svg)?;
        if let Some(jpg) = &jpg {
            std::fs::write(items_dir.join(format!("{key}.page.jpg")), jpg)?;
        }
        if let Some(words) = &context {
            std::fs::write(items_dir.join(format!("{key}.words.json")), words)?;
        }
        // A KOReader markup's page words, with their boxes (SPEC §8f).
        let page_words = std::fs::read(kollate_core::store::page_words_path(&svg_path)).ok();
        if let Some(words) = &page_words {
            std::fs::write(items_dir.join(format!("{key}.page-words.json")), words)?;
        }
        // What the writer looks at: the page with the ink on it, or a
        // notebook page's ink alone.
        let view = match (&jpg, page) {
            (Some(jpg), false) => compose_page(&svg, jpg, None)?,
            _ => jpeg(&ink_only(&svg)?)?,
        };
        std::fs::write(items_dir.join(format!("{key}.view.jpg")), view)?;

        items.push(json!({
            "key": key,
            "kind": if page { "notebook" } else { "markup" },
            "book": book,
            "language": language,
            "chapter": chapter,
            "created": created,
            "trashed": status == "trashed",
            "has_page": jpg.is_some(),
            "has_words": context.is_some(),
            "has_page_words": page_words.is_some(),
            "source": if koreader { "koreader" } else { "nickel" },
        }));
    }

    std::fs::write(
        out.join("manifest.json"),
        serde_json::to_string_pretty(&json!({ "library": library, "items": items }))?,
    )?;
    std::fs::write(
        out.join("data.js"),
        format!("const ITEMS = {};\n", serde_json::to_string(&items)?),
    )?;
    std::fs::write(out.join("sheet.html"), SHEET)?;
    println!(
        "{} items ({} skipped: ink file missing) → {}",
        items.len(),
        skipped,
        out.join("sheet.html").display()
    );
    Ok(())
}

/// Kollate's library: the Flatpak's, else the .deb's.
fn default_library() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".var/app/io.github.andrew_lawlor.Kollate/data/kollate/library.db"),
        home.join(".local/share/kollate/library.db"),
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file())
}

/// All of the ink, black on white, cropped to it.
fn ink_only(svg: &str) -> Result<RgbImage, Box<dyn std::error::Error>> {
    let strokes = segment::strokes(svg);
    let bounds = strokes
        .iter()
        .map(|s| s.bounds)
        .reduce(|a, b| Bounds {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })
        .ok_or("no ink")?;
    let note = Note {
        strokes: (0..strokes.len()).collect(),
        bounds,
        rotation: Rotation::None,
    };
    Ok(render_note(svg, &strokes, &note, 24)?)
}

fn jpeg(img: &RgbImage) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode(
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(bytes)
}

const SHEET: &str = include_str!("handwriting-sheet.html");
