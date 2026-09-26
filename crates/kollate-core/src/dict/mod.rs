//! Offline dictionaries. Every source (the bundled English Wiktionary,
//! user-imported DictFile, StarDict or kaikki.org extracts, WordNet) is converted once into the
//! same small SQLite format and looked up with a rule-based lemmatizer.

mod build;
mod dictfile;
mod lemma;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

pub use build::{
    DictionaryBuilder, build_from_kaikki, build_from_stardict, build_from_wordnet_lmf,
};
pub use dictfile::build_from_dictfile;
pub use lemma::{candidates, fold};

use crate::Result;

pub(crate) const SCHEMA: &str = "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    -- One row per sense; `key` is the folded headword (see `fold`).
    CREATE TABLE entry (
        key      TEXT NOT NULL,
        headword TEXT NOT NULL,
        pos      TEXT,
        gloss    TEXT NOT NULL,
        example  TEXT,
        rank     INTEGER NOT NULL
    );
    -- Inflected or variant forms (mice → mouse), both folded.
    CREATE TABLE form (form TEXT NOT NULL, key TEXT NOT NULL);
";
pub(crate) const INDEXES: &str = "
    CREATE INDEX entry_key ON entry(key, rank);
    CREATE INDEX form_form ON form(form);
";

/// One meaning of a headword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sense {
    pub pos: Option<String>,
    pub gloss: String,
    pub example: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// Dictionary form of the word (e.g. `theophany` for `theophanies`).
    pub headword: String,
    pub senses: Vec<Sense>,
    /// Name of the dictionary it came from.
    pub source: String,
}

impl Definition {
    /// Plain text, one sense per line, e.g. `noun: a visible manifestation…`.
    pub fn to_text(&self, max_senses: usize) -> String {
        self.senses
            .iter()
            .take(max_senses)
            .map(|s| match s.pos.as_deref() {
                Some(pos) => format!("{}: {}", pos_label(pos), s.gloss),
                None => s.gloss.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn pos_label(pos: &str) -> &str {
    match pos {
        "n" => "noun",
        "v" => "verb",
        "a" | "s" => "adjective",
        "r" => "adverb",
        other => other,
    }
}

pub struct Dictionary {
    conn: Connection,
    pub name: String,
    /// ISO 639-1 language, when the source declares one.
    pub language: Option<String>,
    /// Where the data comes from, for credits (e.g. "Wiktionary contributors, via reader.dict (CC BY-SA 4.0)").
    pub source: Option<String>,
    pub path: PathBuf,
}

impl Dictionary {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let meta = |key: &str| -> Result<Option<String>> {
            Ok(conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()?)
        };
        let name = meta("name")?.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        });
        let language = meta("language")?;
        let source = meta("source")?;
        Ok(Self {
            conn,
            name,
            language,
            source,
            path: path.to_path_buf(),
        })
    }

    /// Whether this dictionary covers `language` (dictionaries without a
    /// declared language are tried for everything).
    pub fn covers(&self, language: &str) -> bool {
        language.is_empty() || self.language.as_deref().is_none_or(|l| l == language)
    }

    fn senses(&self, key: &str) -> Result<Option<(String, Vec<Sense>)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT headword, pos, gloss, example FROM entry WHERE key = ?1 ORDER BY rank",
        )?;
        let rows = stmt
            .query_map([key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Sense {
                        pos: r.get(1)?,
                        gloss: r.get(2)?,
                        example: r.get(3)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(headword) = rows.first().map(|(h, _)| h.clone()) else {
            return Ok(None);
        };
        Ok(Some((headword, rows.into_iter().map(|(_, s)| s).collect())))
    }

    /// Looks `word` up as typed, then via inflection rules and the
    /// dictionary's own irregular forms.
    pub fn lookup(&self, word: &str) -> Result<Option<Definition>> {
        let mut form_stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT key FROM form WHERE form = ?1")?;
        let candidates = candidates(word);
        let found = |headword: String, senses: Vec<Sense>| Definition {
            headword,
            senses,
            source: self.name.clone(),
        };
        for candidate in &candidates {
            if let Some((headword, senses)) = self.senses(candidate)? {
                return Ok(Some(found(headword, senses)));
            }
            let mut bases: Vec<String> = form_stmt
                .query_map([candidate], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            // An inflection can be listed under several headwords (debouched:
            // debouch and the variant debouche). Prefer the base the inflection
            // rules also point to, then the shortest.
            bases.sort_by_key(|b| {
                (
                    candidates.iter().position(|c| c == b).unwrap_or(usize::MAX),
                    b.len(),
                )
            });
            for base in bases {
                if let Some((headword, senses)) = self.senses(&base)? {
                    return Ok(Some(found(headword, senses)));
                }
            }
        }
        Ok(None)
    }
}

/// Folders searched for dictionaries, in priority order: ones the user
/// imported (beside the library), then `$KOLLATE_DATA_DIR/dictionaries`, then
/// `kollate/dictionaries` under each `$XDG_DATA_DIRS` entry (where the
/// bundled WordNet is installed, e.g. `/app/share` in the Flatpak).
pub fn search_dirs(library_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = library_dir.map(user_dir).into_iter().collect();
    if let Some(dir) = std::env::var_os("KOLLATE_DATA_DIR") {
        dirs.push(PathBuf::from(dir).join("dictionaries"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let data_dirs = if data_dirs.is_empty() {
        "/usr/local/share:/usr/share".to_owned()
    } else {
        data_dirs
    };
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join("kollate/dictionaries")),
    );
    dirs
}

/// Where dictionaries imported by the user are stored.
pub fn user_dir(library_dir: &Path) -> PathBuf {
    library_dir.join("dictionaries")
}

/// Opens every `*.db` dictionary in `dirs` (earlier directories first).
/// Unreadable files are skipped.
pub fn open_all(dirs: &[PathBuf]) -> Vec<Dictionary> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "db"))
            .collect();
        paths.sort();
        out.extend(paths.iter().filter_map(|p| Dictionary::open(p).ok()));
    }
    out
}

/// Looks `word` up in each dictionary covering `language`, returning the first hit.
pub fn lookup_any(dicts: &[Dictionary], word: &str, language: &str) -> Result<Option<Definition>> {
    for dict in dicts.iter().filter(|d| d.covers(language)) {
        if let Some(def) = dict.lookup(word)? {
            return Ok(Some(def));
        }
    }
    Ok(None)
}

pub(crate) fn insert_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}
