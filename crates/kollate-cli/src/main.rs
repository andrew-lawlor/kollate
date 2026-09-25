//! Developer CLI for Kollate. `inspect` prints what would be imported from a
//! Kobo mount point or a `KoboReader.sqlite` file.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use kollate_core::kobo::{AnnotationKind, DeviceInfo, KoboDb, color_name, find_kobo_db};

#[derive(Parser)]
#[command(name = "kollate-cli", version, about = "Kollate command-line tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the highlights, notes and vocab found on a Kobo (read-only).
    Inspect {
        /// Kobo mount point (e.g. /media/$USER/KOBOeReader) or KoboReader.sqlite file.
        path: PathBuf,
        /// Print the full snapshot as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Inspect { path, json } => inspect(path, json),
    }
}

fn inspect(path: PathBuf, json: bool) -> Result<()> {
    let snapshot = KoboDb::open_copy(&find_kobo_db(&path)?)?.snapshot()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }

    if let Some(info) = DeviceInfo::read(&path) {
        println!(
            "Device {} (firmware {})",
            info.serial,
            info.firmware.as_deref().unwrap_or("?")
        );
    }
    println!(
        "DbVersion {} · {} annotations ({} hidden skipped) · {} vocab words · {} books\n",
        snapshot.db_version,
        snapshot.bookmarks.len(),
        snapshot.hidden_count,
        snapshot.words.len(),
        snapshot.books.len()
    );

    for book in &snapshot.books {
        println!(
            "━━ {} — {}",
            book.title,
            book.author.as_deref().unwrap_or("Unknown")
        );
        let mut chapter = None;
        for b in snapshot
            .bookmarks
            .iter()
            .filter(|b| b.volume_id == book.volume_id)
        {
            if b.chapter_title != chapter {
                chapter = b.chapter_title.clone();
                println!("  § {}", chapter.as_deref().unwrap_or("(unknown chapter)"));
            }
            let body = match b.kind {
                AnnotationKind::Markup => "[stylus markup]".to_owned(),
                _ => b.text.clone().unwrap_or_default().replace('\n', " / "),
            };
            println!(
                "    [{:<9} {:>6}] {}",
                format!("{:?}", b.kind).to_lowercase(),
                color_name(b.color),
                body
            );
            if let Some(note) = &b.note {
                println!("      ✎ {note}");
            }
        }
        let words: Vec<_> = snapshot
            .words
            .iter()
            .filter(|w| w.volume_id.as_deref() == Some(&book.volume_id))
            .map(|w| w.word.as_str())
            .collect();
        if !words.is_empty() {
            println!("  Vocab: {}", words.join(", "));
        }
        println!();
    }
    Ok(())
}
