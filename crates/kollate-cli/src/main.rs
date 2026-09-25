//! Developer CLI for Kollate: inspect a Kobo, import it into the library,
//! and list what the library holds.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use kollate_core::kobo::{AnnotationKind, DeviceInfo, KoboDb, color_name, find_kobo_db};
use kollate_core::{Library, default_library_path};

#[derive(Parser)]
#[command(name = "kollate-cli", version, about = "Kollate command-line tools")]
struct Cli {
    /// Library database (default: ~/.local/share/kollate/library.db).
    #[arg(long, global = true)]
    library: Option<PathBuf>,
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
    /// Import highlights, notes and vocab into the library. Never writes to the Kobo.
    Import {
        /// Kobo mount point or KoboReader.sqlite file.
        path: PathBuf,
        /// Show what would change without saving anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show what the library contains.
    Library,
    /// Find context sentences for Vocab Builder words in the books on a mounted Kobo.
    Contexts { mount: PathBuf },
    /// Build or query offline dictionaries.
    #[command(subcommand)]
    Dict(DictCommand),
}

#[derive(Subcommand)]
enum DictCommand {
    /// Convert Open English WordNet (WN-LMF XML, optionally .gz).
    BuildWordnet { input: PathBuf, output: PathBuf },
    /// Convert a StarDict dictionary (path to its .ifo file).
    BuildStardict { ifo: PathBuf, output: PathBuf },
    /// Convert a kaikki.org Wiktionary extract (.jsonl or .jsonl.gz).
    BuildKaikki {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value = "Wiktionary")]
        name: String,
    },
    /// Look words up in a dictionary database.
    Lookup {
        dictionary: PathBuf,
        words: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let library_path = cli.library.unwrap_or_else(default_library_path);
    match cli.command {
        Command::Inspect { path, json } => inspect(path, json),
        Command::Import { path, dry_run } => import(&path, &library_path, dry_run),
        Command::Library => library(&library_path),
        Command::Dict(cmd) => dict(cmd),
        Command::Contexts { mount } => {
            let snapshot = KoboDb::open_copy(&find_kobo_db(&mount)?)?.snapshot()?;
            for wc in kollate_core::kobo::epub::find_word_contexts(&mount, &snapshot, 3) {
                println!("{}", wc.word);
                for c in &wc.contexts {
                    println!("  [ch {}] {}", c.chapter, c.sentence);
                }
            }
            Ok(())
        }
    }
}

fn dict(cmd: DictCommand) -> Result<()> {
    use kollate_core::dict;
    let started = std::time::Instant::now();
    let built = match cmd {
        DictCommand::BuildWordnet { input, output } => {
            dict::build_from_wordnet_lmf(&input, &output)?
        }
        DictCommand::BuildStardict { ifo, output } => dict::build_from_stardict(&ifo, &output)?,
        DictCommand::BuildKaikki {
            input,
            output,
            name,
        } => dict::build_from_kaikki(&input, &output, &name)?,
        DictCommand::Lookup { dictionary, words } => {
            let d = dict::Dictionary::open(&dictionary)?;
            for word in words {
                match d.lookup(&word)? {
                    Some(def) => println!(
                        "{word} → {}\n  {}",
                        def.headword,
                        def.to_text(3).replace('\n', "\n  ")
                    ),
                    None => println!("{word} → (not found)"),
                }
            }
            return Ok(());
        }
    };
    println!(
        "Wrote {built} senses in {:.1}s",
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

fn import(path: &Path, library_path: &Path, dry_run: bool) -> Result<()> {
    let device = DeviceInfo::identify(path)?;
    let snapshot = KoboDb::open_copy(&find_kobo_db(path)?)?.snapshot()?;
    let mut lib = Library::open(library_path)?;
    let s = lib.import(&snapshot, &device, dry_run)?;
    println!(
        "{} from {}{}",
        if dry_run { "Would import" } else { "Imported" },
        device.serial,
        if dry_run { " (dry run)" } else { "" }
    );
    println!("  books:        {} new", s.books_new);
    println!(
        "  annotations:  {} new, {} updated, {} unchanged, {} removed on device, {} restored, {} skipped",
        s.annotations_new,
        s.annotations_updated,
        s.annotations_unchanged,
        s.annotations_removed,
        s.annotations_restored,
        s.annotations_skipped
    );
    println!(
        "  vocab:        {} new words, {} new sightings",
        s.words_new, s.word_sightings_new
    );
    Ok(())
}

fn library(library_path: &Path) -> Result<()> {
    let lib = Library::open(library_path)?;
    let c = lib.counts()?;
    println!(
        "{} · {} books · {} annotations ({} removed on device) · {} words\n",
        library_path.display(),
        c.books,
        c.annotations,
        c.removed_on_device,
        c.vocab
    );
    for b in lib.books()? {
        println!(
            "{:>4} annotations {:>3} words  {} — {}",
            b.annotation_count,
            b.vocab_count,
            b.title,
            b.author.as_deref().unwrap_or("Unknown")
        );
    }
    Ok(())
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
