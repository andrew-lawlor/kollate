//! The local library: Kollate's own SQLite database and the source of truth.
//! Nothing here ever touches the device.

mod schema;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::Result;

pub struct Library {
    pub(crate) conn: Connection,
}

/// `$XDG_DATA_HOME/kollate/library.db`, defaulting to `~/.local/share`.
pub fn default_library_path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("kollate/library.db")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Inbox,
    Kept,
    Archived,
    Trashed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Kept => "kept",
            Self::Archived => "archived",
            Self::Trashed => "trashed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "kept" => Self::Kept,
            "archived" => Self::Archived,
            "trashed" => Self::Trashed,
            _ => Self::Inbox,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Annotation {
    pub id: i64,
    pub book_id: i64,
    pub kind: String,
    pub device_text: Option<String>,
    pub device_note: Option<String>,
    pub user_text: Option<String>,
    pub user_note: Option<String>,
    pub color: i64,
    pub chapter_title: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub starred: bool,
    pub status: Status,
    pub device_changed_at: Option<DateTime<Utc>>,
    pub removed_on_device_at: Option<DateTime<Utc>>,
}

impl Annotation {
    /// The text to display: the user's correction, else the device text.
    pub fn text(&self) -> Option<&str> {
        self.user_text.as_deref().or(self.device_text.as_deref())
    }

    pub fn note(&self) -> Option<&str> {
        self.user_note.as_deref().or(self.device_note.as_deref())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub author: Option<String>,
    pub annotation_count: i64,
    pub vocab_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Vocab {
    pub id: i64,
    pub word: String,
    pub language: String,
    pub first_seen_at: Option<DateTime<Utc>>,
    /// Titles of the books the word was looked up in.
    pub books: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LibraryCounts {
    pub books: i64,
    pub annotations: i64,
    pub vocab: i64,
    pub sightings: i64,
    pub removed_on_device: i64,
}

const ANNOTATION_COLUMNS: &str =
    "id, book_id, kind, device_text, device_note, user_text, user_note, color,
     chapter_title, created_at, starred, status, device_changed_at, removed_on_device_at";

fn annotation_from_row(r: &rusqlite::Row) -> rusqlite::Result<Annotation> {
    Ok(Annotation {
        id: r.get(0)?,
        book_id: r.get(1)?,
        kind: r.get(2)?,
        device_text: r.get(3)?,
        device_note: r.get(4)?,
        user_text: r.get(5)?,
        user_note: r.get(6)?,
        color: r.get(7)?,
        chapter_title: r.get(8)?,
        created_at: r.get(9)?,
        starred: r.get(10)?,
        status: Status::parse(&r.get::<_, String>(11)?),
        device_changed_at: r.get(12)?,
        removed_on_device_at: r.get(13)?,
    })
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        let version: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in schema::MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    pub fn counts(&self) -> Result<LibraryCounts> {
        Ok(self.conn.query_row(
            "SELECT (SELECT count(*) FROM book), (SELECT count(*) FROM annotation),
                    (SELECT count(*) FROM vocab), (SELECT count(*) FROM vocab_sighting),
                    (SELECT count(*) FROM annotation WHERE removed_on_device_at IS NOT NULL)",
            [],
            |r| {
                Ok(LibraryCounts {
                    books: r.get(0)?,
                    annotations: r.get(1)?,
                    vocab: r.get(2)?,
                    sightings: r.get(3)?,
                    removed_on_device: r.get(4)?,
                })
            },
        )?)
    }

    pub fn books(&self) -> Result<Vec<Book>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, coalesce(b.user_title, b.title), coalesce(b.user_author, b.author),
                    (SELECT count(*) FROM annotation a WHERE a.book_id = b.id AND a.status != 'trashed'),
                    (SELECT count(DISTINCT s.vocab_id) FROM vocab_sighting s WHERE s.book_id = b.id)
             FROM book b WHERE NOT b.hidden ORDER BY 2 COLLATE NOCASE",
        )?;
        let books = stmt.query_map([], |r| {
            Ok(Book {
                id: r.get(0)?,
                title: r.get(1)?,
                author: r.get(2)?,
                annotation_count: r.get(3)?,
                vocab_count: r.get(4)?,
            })
        })?;
        Ok(books.collect::<rusqlite::Result<_>>()?)
    }

    pub fn annotation(&self, id: i64) -> Result<Option<Annotation>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ANNOTATION_COLUMNS} FROM annotation WHERE id = ?1"),
                [id],
                annotation_from_row,
            )
            .optional()?)
    }

    /// Annotations of a book in reading order.
    pub fn annotations_for_book(&self, book_id: i64) -> Result<Vec<Annotation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ANNOTATION_COLUMNS} FROM annotation WHERE book_id = ?1
             ORDER BY spine_index IS NULL, spine_index, chapter_progress, start_offset, id"
        ))?;
        Ok(stmt
            .query_map([book_id], annotation_from_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn annotation_id_for_bookmark(&self, bookmark_id: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT annotation_id FROM annotation_source WHERE bookmark_id = ?1",
                [bookmark_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn vocab(&self) -> Result<Vec<Vocab>> {
        let mut stmt = self.conn.prepare(
            "SELECT v.id, v.word, v.language, v.first_seen_at,
                    (SELECT group_concat(DISTINCT coalesce(b.user_title, b.title))
                     FROM vocab_sighting s JOIN book b ON b.id = s.book_id WHERE s.vocab_id = v.id)
             FROM vocab v ORDER BY v.first_seen_at, v.id",
        )?;
        let vocab = stmt.query_map([], |r| {
            Ok(Vocab {
                id: r.get(0)?,
                word: r.get(1)?,
                language: r.get(2)?,
                first_seen_at: r.get(3)?,
                books: r
                    .get::<_, Option<String>>(4)?
                    .map(|s| s.split(',').map(str::to_owned).collect())
                    .unwrap_or_default(),
            })
        })?;
        Ok(vocab.collect::<rusqlite::Result<_>>()?)
    }

    fn record_revision(
        &self,
        id: i64,
        field: &str,
        old: Option<&str>,
        new: Option<&str>,
        source: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO annotation_revision (annotation_id, field, old_value, new_value, source, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, field, old, new, source, Utc::now()],
        )?;
        Ok(())
    }

    /// Sets (or with `None`, clears) the user's version of the note. The
    /// device's note is kept and future device changes never overwrite it.
    pub fn set_user_note(&self, id: i64, note: Option<&str>) -> Result<()> {
        self.set_user_field(id, "user_note", note)
    }

    /// Sets (or clears) a corrected highlight text.
    pub fn set_user_text(&self, id: i64, text: Option<&str>) -> Result<()> {
        self.set_user_field(id, "user_text", text)
    }

    fn set_user_field(&self, id: i64, column: &str, value: Option<&str>) -> Result<()> {
        let old: Option<String> = self.conn.query_row(
            &format!("SELECT {column} FROM annotation WHERE id = ?1"),
            [id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            &format!("UPDATE annotation SET {column} = ?2, updated_at = ?3 WHERE id = ?1"),
            params![id, value, Utc::now()],
        )?;
        self.record_revision(id, column, old.as_deref(), value, "user")
    }

    pub fn set_status(&self, id: i64, status: Status) -> Result<()> {
        self.conn.execute(
            "UPDATE annotation SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), Utc::now()],
        )?;
        Ok(())
    }

    /// Accepts the device's latest values, dropping the user's overrides.
    pub fn accept_device_version(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE annotation SET user_text = NULL, user_note = NULL, device_changed_at = NULL,
                    updated_at = ?2 WHERE id = ?1",
            params![id, Utc::now()],
        )?;
        Ok(())
    }
}
