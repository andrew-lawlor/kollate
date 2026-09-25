//! The local library: Kollate's own SQLite database and the source of truth.
//! Nothing here ever touches the device.

mod query;
mod schema;

pub use query::{AnnotationFilter, SidebarCounts, Tag, View, VocabStatus};

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::Result;

pub struct Library {
    pub(crate) conn: Connection,
    /// Folder holding the database; assets are stored beside it.
    dir: Option<PathBuf>,
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

    pub(crate) fn parse(s: &str) -> Self {
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
    pub book_title: String,
    pub book_author: Option<String>,
    pub tags: Vec<String>,
    /// Copied page image of a stylus markup.
    pub markup_image: Option<PathBuf>,
}

impl Annotation {
    /// The text to display: the user's correction, else the device text.
    pub fn text(&self) -> Option<&str> {
        self.user_text.as_deref().or(self.device_text.as_deref())
    }

    /// The note to display. A user override of `""` hides the Kobo's note.
    pub fn note(&self) -> Option<&str> {
        self.user_note
            .as_deref()
            .or(self.device_note.as_deref())
            .filter(|n| !n.is_empty())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub author: Option<String>,
    pub annotation_count: i64,
    pub vocab_count: i64,
    pub cover: Option<PathBuf>,
    pub percent_read: Option<i64>,
    pub last_read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Vocab {
    pub id: i64,
    pub word: String,
    pub language: String,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub status: VocabStatus,
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

pub(crate) const ANNOTATION_SELECT: &str = "SELECT a.id, a.book_id, a.kind, a.device_text, a.device_note,
     a.user_text, a.user_note, a.color, a.chapter_title, a.created_at, a.starred, a.status,
     a.device_changed_at, a.removed_on_device_at, coalesce(b.user_title, b.title),
     coalesce(b.user_author, b.author),
     (SELECT group_concat(name, char(31)) FROM (SELECT t.name FROM annotation_tag x JOIN tag t ON t.id = x.tag_id
      WHERE x.annotation_id = a.id ORDER BY t.name COLLATE NOCASE)),
     a.markup_jpg_path
     FROM annotation a JOIN book b ON b.id = a.book_id";

pub(crate) fn annotation_from_row(r: &rusqlite::Row) -> rusqlite::Result<Annotation> {
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
        book_title: r.get(14)?,
        book_author: r.get(15)?,
        tags: r
            .get::<_, Option<String>>(16)?
            .map(|s| s.split('\u{1f}').map(str::to_owned).collect())
            .unwrap_or_default(),
        markup_image: r.get::<_, Option<String>>(17)?.map(PathBuf::from),
    })
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(
            Connection::open(path)?,
            path.parent().map(Path::to_path_buf),
        )
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    /// Where copied covers and markup images go (`None` for in-memory libraries).
    pub fn assets_dir(&self) -> Option<PathBuf> {
        self.dir.clone()
    }

    fn init(conn: Connection, dir: Option<PathBuf>) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        let version: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in schema::MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        let lib = Self { conn, dir };
        lib.backfill_position_keys()?;
        Ok(lib)
    }

    fn backfill_position_keys(&self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT id, start_path, start_offset, chapter_progress FROM annotation WHERE position_key IS NULL",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, path, offset, progress) in rows {
            self.conn.execute(
                "UPDATE annotation SET position_key = ?2 WHERE id = ?1",
                params![id, crate::kobo::position_key(&path, offset, progress)],
            )?;
        }
        Ok(())
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
        self.query_books(None)
    }

    pub fn book(&self, id: i64) -> Result<Option<Book>> {
        Ok(self.query_books(Some(id))?.into_iter().next())
    }

    fn query_books(&self, id: Option<i64>) -> Result<Vec<Book>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, coalesce(b.user_title, b.title), coalesce(b.user_author, b.author),
                    (SELECT count(*) FROM annotation a WHERE a.book_id = b.id AND a.status IN ('inbox', 'kept')),
                    (SELECT count(DISTINCT s.vocab_id) FROM vocab_sighting s WHERE s.book_id = b.id),
                    b.cover_path, b.percent_read, b.last_read_at
             FROM book b WHERE NOT b.hidden AND (?1 IS NULL OR b.id = ?1) ORDER BY 2 COLLATE NOCASE",
        )?;
        let books = stmt.query_map([id], |r| {
            Ok(Book {
                id: r.get(0)?,
                title: r.get(1)?,
                author: r.get(2)?,
                annotation_count: r.get(3)?,
                vocab_count: r.get(4)?,
                cover: r.get::<_, Option<String>>(5)?.map(PathBuf::from),
                percent_read: r.get(6)?,
                last_read_at: r.get(7)?,
            })
        })?;
        Ok(books.collect::<rusqlite::Result<_>>()?)
    }

    /// Records where a device's covers and markup images were copied to.
    pub fn attach_assets(
        &self,
        device: &crate::kobo::DeviceInfo,
        assets: &crate::kobo::assets::CopiedAssets,
    ) -> Result<()> {
        let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned());
        for (volume_id, cover) in &assets.covers {
            self.conn.execute(
                "UPDATE book SET cover_path = ?3 WHERE id = (
                    SELECT s.book_id FROM book_source s JOIN device d ON d.id = s.device_id
                    WHERE d.serial = ?1 AND s.volume_id = ?2)",
                params![device.serial, volume_id, cover.to_string_lossy()],
            )?;
        }
        for (bookmark_id, svg, jpg) in &assets.markups {
            self.conn.execute(
                "UPDATE annotation SET markup_svg_path = ?2, markup_jpg_path = ?3
                 WHERE id = (SELECT annotation_id FROM annotation_source WHERE bookmark_id = ?1)",
                params![bookmark_id, path(svg), path(jpg)],
            )?;
        }
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn annotation(&self, id: i64) -> Result<Option<Annotation>> {
        Ok(self
            .conn
            .query_row(
                &format!("{ANNOTATION_SELECT} WHERE a.id = ?1"),
                [id],
                annotation_from_row,
            )
            .optional()?)
    }

    /// Annotations of a book in reading order.
    pub fn annotations_for_book(&self, book_id: i64) -> Result<Vec<Annotation>> {
        self.query_annotations(&AnnotationFilter {
            view: View::Book(book_id),
            ..Default::default()
        })
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
        self.query_vocab(None, None)
    }

    /// Vocab words, newest first, optionally limited to one book and/or
    /// matching a search string.
    pub fn query_vocab(&self, book_id: Option<i64>, search: Option<&str>) -> Result<Vec<Vocab>> {
        let pattern = search
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(query::like_pattern);
        let mut stmt = self.conn.prepare(
            "SELECT v.id, v.word, v.language, v.first_seen_at, v.status,
                    (SELECT group_concat(title, char(31)) FROM (SELECT DISTINCT coalesce(b.user_title, b.title) AS title
                     FROM vocab_sighting s JOIN book b ON b.id = s.book_id WHERE s.vocab_id = v.id))
             FROM vocab v
             WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM vocab_sighting s WHERE s.vocab_id = v.id AND s.book_id = ?1))
               AND (?2 IS NULL OR v.word LIKE ?2 ESCAPE '\\' OR v.definition LIKE ?2 ESCAPE '\\')
             ORDER BY v.first_seen_at DESC, v.id DESC",
        )?;
        let vocab = stmt.query_map(params![book_id, pattern], |r| {
            Ok(Vocab {
                id: r.get(0)?,
                word: r.get(1)?,
                language: r.get(2)?,
                first_seen_at: r.get(3)?,
                status: VocabStatus::parse(&r.get::<_, String>(4)?),
                books: r
                    .get::<_, Option<String>>(5)?
                    .map(|s| s.split('\u{1f}').map(str::to_owned).collect())
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
