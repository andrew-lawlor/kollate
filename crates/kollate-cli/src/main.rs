//! Developer CLI for Kollate: inspect a Kobo, import it into the library,
//! and list what the library holds.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use kollate_core::kobo::{
    AnnotationKind, DeviceInfo, KoboDb, KoboSnapshot, color_name, find_kobo_db,
    is_tested_db_version,
};
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
    /// Handwriting models for transcribing stylus markups (list them, or add files).
    Models {
        #[command(subcommand)]
        action: Option<ModelsAction>,
    },
    /// Transcribe stylus markups that haven't been read by the current model.
    Transcribe {
        /// Model to use (see `models`); default: the one chosen in the app,
        /// else the recommended one.
        #[arg(long)]
        model: Option<String>,
        /// Don't use the GPU.
        #[arg(long)]
        cpu: bool,
    },
    /// Find context sentences for Vocab Builder words in the books on a mounted Kobo.
    Contexts { mount: PathBuf },
    /// Build or query offline dictionaries.
    #[command(subcommand)]
    Dict(DictCommand),
    /// Export the library.
    Export {
        #[command(subcommand)]
        format: ExportFormat,
        /// Include archived highlights.
        #[arg(long, global = true)]
        archived: bool,
        /// Include words marked Known.
        #[arg(long, global = true)]
        known: bool,
    },
}

#[derive(Subcommand)]
enum ExportFormat {
    /// Sync notes into a folder of an Obsidian vault.
    Obsidian { folder: PathBuf },
    /// Write an Anki package (.apkg).
    Anki {
        output: PathBuf,
        /// Also add a deck of highlights.
        #[arg(long)]
        highlights: bool,
    },
    /// Full JSON backup.
    Json { output: PathBuf },
    /// Highlights as CSV.
    Csv { output: PathBuf },
    /// Vocabulary as CSV.
    VocabCsv { output: PathBuf },
    /// Readwise-compatible CSV.
    Readwise { output: PathBuf },
}

#[derive(Subcommand)]
enum DictCommand {
    /// Convert Open English WordNet (WN-LMF XML, optionally .gz).
    BuildWordnet { input: PathBuf, output: PathBuf },
    /// Convert a StarDict dictionary (path to its .ifo file).
    BuildStardict { ifo: PathBuf, output: PathBuf },
    /// Convert a DictFile (.df, .df.bz2), e.g. reader.dict's Wiktionary extracts.
    BuildDictfile {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value = "English Wiktionary")]
        name: String,
        #[arg(long, default_value = "en")]
        language: String,
        #[arg(
            long,
            default_value = "Wiktionary contributors, via reader.dict (CC BY-SA 4.0)"
        )]
        source: String,
    },
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
        Command::Models { action } => models(&library_path, action),
        Command::Transcribe { model, cpu } => transcribe(&library_path, model.as_deref(), cpu),
        Command::Dict(cmd) => dict(cmd),
        Command::Export {
            format,
            archived,
            known,
        } => {
            use kollate_core::export::{self, ExportOptions};
            let lib = Library::open(&library_path)?;
            let opts = ExportOptions {
                include_archived: archived,
                include_known_words: known,
                everything: false,
            };
            match format {
                ExportFormat::Obsidian { folder } => {
                    let s = export::sync_obsidian(&lib, &folder, opts)?;
                    println!(
                        "{} notes written, {} unchanged, {} images copied",
                        s.notes_written, s.notes_unchanged, s.attachments_copied
                    );
                }
                ExportFormat::Anki { output, highlights } => {
                    let s = export::export_anki(&lib, &output, opts, highlights)?;
                    println!(
                        "{} words, {} highlights, {} cards",
                        s.words, s.highlights, s.cards
                    );
                }
                ExportFormat::Json { output } => {
                    println!("{} books", export::export_json(&lib, &output)?)
                }
                ExportFormat::Csv { output } => println!(
                    "{} rows",
                    export::export_highlights_csv(&lib, &output, opts)?
                ),
                ExportFormat::VocabCsv { output } => {
                    println!("{} rows", export::export_vocab_csv(&lib, &output, opts)?)
                }
                ExportFormat::Readwise { output } => {
                    println!("{} rows", export::export_readwise_csv(&lib, &output, opts)?)
                }
            }
            Ok(())
        }
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
        DictCommand::BuildDictfile {
            input,
            output,
            name,
            language,
            source,
        } => dict::build_from_dictfile(&input, &output, &name, Some(&language), &source)?,
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

/// Kobo database versions other than the tested ones are read anyway, with a
/// note on stderr.
fn warn_if_untested(snapshot: &KoboSnapshot) {
    if !is_tested_db_version(snapshot.db_version) {
        eprintln!(
            "warning: Kobo database version {} hasn't been tested with Kollate yet. \
             Please check the results, and report problems (or success) at {ISSUES_URL}",
            snapshot.db_version
        );
    }
}

const ISSUES_URL: &str = "https://github.com/andrew-lawlor/kollate/issues";

fn import(path: &Path, library_path: &Path, dry_run: bool) -> Result<()> {
    let device = DeviceInfo::identify(path)?;
    let snapshot = KoboDb::open_copy(&find_kobo_db(path)?)?.snapshot()?;
    warn_if_untested(&snapshot);
    let mut lib = Library::open(library_path)?;
    let s = lib.import(&snapshot, &device, dry_run)?;
    // From a mounted Kobo, also what the app copies: covers, markup images,
    // and the book text vocabulary and markups are resolved against.
    if !dry_run && path.is_dir() {
        use kollate_core::kobo::assets::copy_assets;
        use kollate_core::kobo::epub::{find_word_contexts, markup_contexts};
        if let Some(dir) = lib.assets_dir() {
            lib.attach_assets(&device, &copy_assets(path, &snapshot, &dir)?)?;
        }
        lib.set_word_contexts(&device, &find_word_contexts(path, &snapshot, 5))?;
        lib.set_markup_contexts(&device, &markup_contexts(path, &snapshot))?;
    }
    println!(
        "{} from {}{}",
        if dry_run { "Would import" } else { "Imported" },
        device.serial,
        if dry_run { " (dry run)" } else { "" }
    );
    println!("  books:        {} new", s.books_new);
    println!(
        "  annotations:  {} new, {} updated, {} unchanged, {} removed on device ({} moved to Trash), {} restored, {} skipped",
        s.annotations_new,
        s.annotations_updated,
        s.annotations_unchanged,
        s.annotations_removed,
        s.annotations_trashed,
        s.annotations_restored,
        s.annotations_skipped
    );
    println!(
        "  vocab:        {} new words, {} new sightings",
        s.words_new, s.word_sightings_new
    );
    Ok(())
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Add downloaded model files (or folders holding them). Each is checked.
    Add { files: Vec<PathBuf> },
    /// Delete a model's files.
    Remove { id: String },
}

fn models_dir(lib: &Library) -> Result<PathBuf> {
    Ok(lib
        .assets_dir()
        .ok_or_else(|| anyhow::anyhow!("no library folder"))?
        .join("models"))
}

fn models(library_path: &Path, action: Option<ModelsAction>) -> Result<()> {
    use kollate_transcribe::{Role, catalog};
    let dir = models_dir(&Library::open(library_path)?)?;
    match action {
        None => {
            let installed = kollate_transcribe::installed(&dir);
            for m in catalog() {
                let have = installed.iter().any(|i| i.model.id == m.id);
                println!(
                    "{} {:<12} {:<12} {:.1} GB{}  {}",
                    if have { "✓" } else { " " },
                    m.id,
                    m.name,
                    m.size() as f64 / 1e9,
                    if m.recommended { " (recommended)" } else { "" },
                    m.summary
                );
                if !have {
                    for role in [Role::Model, Role::Vision] {
                        println!("      {}", m.file(role).url);
                    }
                }
            }
        }
        Some(ModelsAction::Add { files }) => {
            let added = kollate_transcribe::add(&dir, &files)?;
            for m in added.complete {
                println!("Added {}", m.name);
            }
            for (m, role) in added.waiting {
                println!("{} still needs {}", m.name, m.file(role).name);
            }
            for f in added.unknown {
                println!("Not a known model file (or damaged): {}", f.display());
            }
        }
        Some(ModelsAction::Remove { id }) => {
            let m = catalog()
                .iter()
                .find(|m| m.id == id)
                .ok_or_else(|| anyhow::anyhow!("unknown model {id}"))?;
            kollate_transcribe::remove(&dir, m)?;
            println!("Removed {}", m.name);
        }
    }
    Ok(())
}

fn transcribe(library_path: &Path, model: Option<&str>, cpu: bool) -> Result<()> {
    let lib = Library::open(library_path)?;
    let preferred = model
        .map(str::to_owned)
        .or(lib.setting("transcribe_model")?);
    let installed = kollate_transcribe::installed(&models_dir(&lib)?);
    let Some(chosen) = kollate_transcribe::choose(&installed, preferred.as_deref()) else {
        anyhow::bail!("no handwriting model installed; see `kollate-cli models`");
    };
    let jobs = lib.pending_transcriptions(chosen.model.id)?;
    if jobs.is_empty() {
        println!("Every markup is transcribed with {}.", chosen.model.name);
        return Ok(());
    }
    // Dictionaries tell misread names apart from real words (SPEC §8a).
    let dicts = kollate_core::dict::open_all(&kollate_core::dict::search_dirs(
        lib.assets_dir().as_deref(),
    ));
    let known = |w: &str| dicts.iter().any(|d| d.lookup(w).ok().flatten().is_some());
    let known: Option<&dyn Fn(&str) -> bool> = (!dicts.is_empty()).then_some(&known);
    let started = std::time::Instant::now();
    let mut reader = chosen.load(!cpu)?;
    println!(
        "{} loaded in {:.1}s",
        chosen.model.name,
        started.elapsed().as_secs_f32()
    );
    for job in &jobs {
        let t0 = std::time::Instant::now();
        let t = job.run(&mut reader, known)?;
        lib.save_transcription(job, &t, chosen.model.id)?;
        println!(
            "#{} ({:.1}s)",
            job.annotation_id,
            t0.elapsed().as_secs_f32()
        );
        if let Some(text) = &t.text {
            println!("  marks: {}", text.replace('\n', "\n         "));
        }
        if let Some(note) = &t.note {
            println!("  notes: {}", note.replace('\n', "\n         "));
        }
    }
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
    warn_if_untested(&snapshot);
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
