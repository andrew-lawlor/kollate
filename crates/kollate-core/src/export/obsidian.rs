//! Obsidian vault sync: one Markdown note per book plus a vocabulary note.
//! Kollate owns everything above the `%% kollate:user` marker in a note and
//! rewrites it on every sync (only when it changed); everything below the
//! marker belongs to the user and is kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{ExportBook, ExportOptions, safe_file_name};
use crate::Result;
use crate::kobo::color_name;
use crate::kobo::epub::word_span;
use crate::store::{Annotation, Library, VocabDetail};

const USER_MARKER: &str = "%% kollate:user";
const USER_MARKER_LINE: &str = "%% kollate:user — Kollate rewrites everything above this line when it syncs. \
     Write your own notes about this book below it; that part is never changed. %%";
const VOCAB_NOTE_ID: &str = "vocabulary";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObsidianStats {
    pub notes_written: usize,
    pub notes_unchanged: usize,
    pub attachments_copied: usize,
}

fn yaml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Obsidian tag from a Kollate tag (`Old Norse` → `#old-norse`).
fn obsidian_tag(tag: &str) -> String {
    let t: String = tag
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '/' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("#{}", t.trim_matches('-'))
}

/// `sentence` with the looked-up word in bold.
fn bold_word(sentence: &str, words: &[&str]) -> String {
    match words
        .iter()
        .filter(|w| !w.is_empty())
        .find_map(|w| word_span(sentence, w))
    {
        Some((a, b)) => format!(
            "{}**{}**{}",
            &sentence[..a],
            &sentence[a..b],
            &sentence[b..]
        ),
        None => sentence.to_owned(),
    }
}

/// Reads `kollate_id` from a note's front matter.
fn note_id(content: &str) -> Option<String> {
    let front = content.strip_prefix("---\n")?;
    let end = front.find("\n---")?;
    front[..end].lines().find_map(|l| {
        l.strip_prefix("kollate_id:")
            .map(|v| v.trim().trim_matches('"').to_owned())
    })
}

/// The user's part of an existing note (from the marker on), if any.
fn user_part(content: &str) -> Option<&str> {
    content.find(USER_MARKER).map(|i| &content[i..])
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn annotation_md(a: &Annotation, attachment: Option<&str>, out: &mut String) {
    let block_id = format!("^k{}", a.id);
    match (a.text(), attachment) {
        (Some(text), _) => out.push_str(&format!("{} {block_id}\n", quote(text))),
        (None, Some(file)) => out.push_str(&format!("![[{file}]] {block_id}\n")),
        (None, None) => out.push_str(&format!("> *(handwritten markup)* {block_id}\n")),
    }
    if let Some(note) = a.note() {
        out.push('\n');
        out.push_str(note);
        out.push('\n');
    }
    let mut meta = Vec::new();
    if let Some(date) = a.created_at {
        meta.push(
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string(),
        );
    }
    if a.kind == "markup" || a.color != 0 {
        meta.push(color_name(a.color).to_owned());
    }
    if a.starred {
        meta.push("★".to_owned());
    }
    let tags: Vec<String> = a.tags.iter().map(|t| obsidian_tag(t)).collect();
    meta.extend(tags);
    if !meta.is_empty() {
        out.push_str(&format!("\n<small>{}</small>\n", meta.join(" · ")));
    }
    out.push('\n');
}

fn vocab_md(v: &VocabDetail, out: &mut String) {
    let word = &v.vocab.word;
    let lemma = v.vocab.lemma.as_deref().unwrap_or("");
    out.push_str(&format!("- **{word}**"));
    if !lemma.is_empty() && !lemma.eq_ignore_ascii_case(word) {
        out.push_str(&format!(" ({lemma})"));
    }
    match v.vocab.definition.as_deref() {
        Some(def) => {
            let mut lines = def.lines();
            out.push_str(&format!(": {}\n", lines.next().unwrap_or_default()));
            for line in lines {
                out.push_str(&format!("  {line}\n"));
            }
        }
        None => out.push('\n'),
    }
    for s in &v.sightings {
        if let Some(context) = &s.context {
            out.push_str(&format!(
                "  > {}\n",
                bold_word(context, &[&s.surface_form, word, lemma])
            ));
        }
    }
}

fn book_note(eb: &ExportBook, attachments: &HashMap<i64, String>) -> String {
    let b = &eb.book;
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_string(&b.title)));
    if let Some(author) = &b.author {
        out.push_str(&format!("author: {}\n", yaml_string(author)));
    }
    for (key, value) in [
        ("isbn", &b.isbn),
        ("publisher", &b.publisher),
        ("series", &b.series),
    ] {
        if let Some(v) = value {
            out.push_str(&format!("{key}: {}\n", yaml_string(v)));
        }
    }
    if let Some(n) = &b.series_number {
        out.push_str(&format!("series_number: {}\n", yaml_string(n)));
    }
    out.push_str("tags: [book, kobo]\n");
    out.push_str(&format!("highlights: {}\n", eb.annotations.len()));
    out.push_str(&format!("kollate_id: {}\n", b.id));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", b.title));
    if let Some(author) = &b.author {
        out.push_str(&format!("*{author}*\n\n"));
    }

    let mut chapter: Option<&str> = None;
    for a in &eb.annotations {
        let this = a.chapter_title.as_deref();
        if this != chapter && this.is_some() {
            out.push_str(&format!("## {}\n\n", this.unwrap_or_default()));
        }
        chapter = this;
        annotation_md(a, attachments.get(&a.id).map(String::as_str), &mut out);
    }

    if !eb.vocab.is_empty() {
        out.push_str("## Vocabulary\n\n");
        for v in &eb.vocab {
            vocab_md(v, &mut out);
        }
        out.push('\n');
    }
    out
}

fn vocab_note(books: &[ExportBook], note_names: &HashMap<i64, String>) -> String {
    let mut words: Vec<(&VocabDetail, &str)> = Vec::new();
    for eb in books {
        for v in &eb.vocab {
            if !words.iter().any(|(w, _)| w.vocab.id == v.vocab.id) {
                words.push((
                    v,
                    note_names
                        .get(&eb.book.id)
                        .map(String::as_str)
                        .unwrap_or(""),
                ));
            }
        }
    }
    words.sort_by_key(|(v, _)| v.vocab.word.to_lowercase());
    let mut out = format!(
        "---\ntags: [vocabulary, kobo]\nwords: {}\nkollate_id: {VOCAB_NOTE_ID}\n---\n\n# Vocabulary\n\n",
        words.len()
    );
    for (v, note) in words {
        vocab_md(v, &mut out);
        if !note.is_empty() {
            out.push_str(&format!("  From [[{note}]]\n"));
        }
    }
    out.push('\n');
    out
}

/// Writes `generated` to `path`, keeping the user part of an existing note.
/// Returns whether the file changed.
fn write_note(path: &Path, generated: &str, existing: Option<&str>) -> Result<bool> {
    // Keep the user's text below the marker, refreshing the marker's own
    // wording (older notes carry an earlier explanation).
    let user = match existing.and_then(user_part) {
        Some(part) => {
            let rest = part.split_once('\n').map_or("", |(_, rest)| rest);
            format!("{USER_MARKER_LINE}\n{rest}")
        }
        None => format!("{USER_MARKER_LINE}\n"),
    };
    let content = format!("{generated}{user}");
    if existing == Some(content.as_str()) {
        return Ok(false);
    }
    let tmp = path.with_extension("md.part");
    std::fs::write(&tmp, &content)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Syncs the library into `folder` (inside an Obsidian vault). Notes are
/// found again by their `kollate_id`, so renamed books move their note.
pub fn sync_obsidian(
    lib: &Library,
    folder: &Path,
    options: ExportOptions,
) -> Result<ObsidianStats> {
    crate::kobo::ensure_not_on_kobo(folder)?;
    std::fs::create_dir_all(folder)?;
    let books = lib.export_books(options)?;
    let mut stats = ObsidianStats::default();

    // Existing Kollate notes in the folder, by ID.
    let mut existing: HashMap<String, (PathBuf, String)> = HashMap::new();
    for entry in std::fs::read_dir(folder)?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md")
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Some(id) = note_id(&content)
        {
            existing.insert(id, (path, content));
        }
    }

    // Unique note names (a clash gets the author appended).
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    for eb in &books {
        *taken
            .entry(safe_file_name(&eb.book.title).to_lowercase())
            .or_default() += 1;
    }
    for eb in &books {
        let base = safe_file_name(&eb.book.title);
        let name = match (&eb.book.author, taken[&base.to_lowercase()] > 1) {
            (Some(author), true) => safe_file_name(&format!("{base} ({author})")),
            (None, true) => format!("{base} ({})", eb.book.id),
            _ => base,
        };
        names.insert(eb.book.id, name);
    }

    // Markup page images go to attachments/.
    let mut attachments = HashMap::new();
    for a in books.iter().flat_map(|eb| &eb.annotations) {
        let Some(src) = a.markup_image.as_ref().filter(|p| p.is_file()) else {
            continue;
        };
        let file = format!("kollate-markup-{}.jpg", a.id);
        let dest = folder.join("attachments").join(&file);
        let same = std::fs::metadata(&dest).ok().map(|m| m.len())
            == std::fs::metadata(src).ok().map(|m| m.len());
        if !same {
            std::fs::create_dir_all(dest.parent().expect("has parent"))?;
            std::fs::copy(src, &dest)?;
            stats.attachments_copied += 1;
        }
        attachments.insert(a.id, file);
    }

    let mut write = |id: String, name: &str, generated: String| -> Result<()> {
        let path = folder.join(format!("{name}.md"));
        let previous = existing.remove(&id);
        let old_content = previous.as_ref().map(|(_, c)| c.as_str());
        // Renamed book: move the note, keeping the user's part.
        if let Some((old_path, _)) = &previous
            && old_path != &path
            && !path.exists()
        {
            std::fs::rename(old_path, &path)?;
        }
        if write_note(&path, &generated, old_content)? {
            stats.notes_written += 1;
        } else {
            stats.notes_unchanged += 1;
        }
        Ok(())
    };

    for eb in &books {
        write(
            eb.book.id.to_string(),
            &names[&eb.book.id],
            book_note(eb, &attachments),
        )?;
    }
    if books.iter().any(|eb| !eb.vocab.is_empty()) {
        write(
            VOCAB_NOTE_ID.to_owned(),
            "Vocabulary",
            vocab_note(&books, &names),
        )?;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tags_and_bold_words() {
        assert_eq!(obsidian_tag("Old Norse!"), "#old-norse");
        assert_eq!(
            bold_word("His soteriology is escapist.", &["soteriology"]),
            "His **soteriology** is escapist."
        );
    }

    #[test]
    fn reads_note_ids_and_user_parts() {
        let note = "---\ntitle: \"x\"\nkollate_id: 7\n---\n# x\n%% kollate:user: … %%\nmine";
        assert_eq!(note_id(note).as_deref(), Some("7"));
        assert_eq!(user_part(note), Some("%% kollate:user: … %%\nmine"));
        assert_eq!(note_id("# no front matter"), None);
    }
}
