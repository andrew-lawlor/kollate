//! Reads a Kobo database. The device file is never opened directly: it is
//! copied (with its WAL, if any) to a temp dir first, so the device can be
//! ejected at any time and can never be written to.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, Row};
use tempfile::TempDir;

use super::chapters::{ChapterIndex, Resolution};
use super::model::*;
use crate::normalize::{clean_opt, parse_kobo_date};
use crate::{Error, Result};

pub struct KoboDb {
    conn: Connection,
    _copy_dir: TempDir,
}

/// Reads a text-ish column leniently: Kobo sometimes stores non-UTF-8 bytes.
fn text(row: &Row, idx: usize) -> rusqlite::Result<Option<String>> {
    Ok(row
        .get_ref(idx)?
        .as_bytes_or_null()?
        .map(|b| String::from_utf8_lossy(b).into_owned()))
}

fn blob(row: &Row, idx: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    Ok(row.get_ref(idx)?.as_bytes_or_null()?.map(<[u8]>::to_vec))
}

fn int(row: &Row, idx: usize) -> rusqlite::Result<Option<i64>> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Integer(i) => Some(i),
        ValueRef::Real(f) => Some(f as i64),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        _ => None,
    })
}

/// Kobo booleans are stored as `'true'`/`'false'` text or as integers.
fn boolean(row: &Row, idx: usize) -> rusqlite::Result<bool> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Integer(i) => i != 0,
        ValueRef::Text(t) => t.eq_ignore_ascii_case(b"true") || t == b"1",
        _ => false,
    })
}

impl KoboDb {
    /// Opens a copy of the database at `db_path` (see [`super::find_kobo_db`]).
    pub fn open_copy(db_path: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new().prefix("kollate-kobo-").tempdir()?;
        // Even the scratch copy must not land on the device (e.g. via TMPDIR).
        super::ensure_not_on_kobo(dir.path())?;
        let copy = dir.path().join("KoboReader.sqlite");
        std::fs::copy(db_path, &copy)?;
        let wal = db_path.with_extension("sqlite-wal");
        if wal.is_file() {
            std::fs::copy(&wal, dir.path().join("KoboReader.sqlite-wal"))?;
        }
        let conn = Connection::open(&copy)?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self {
            conn,
            _copy_dir: dir,
        })
    }

    pub fn db_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT version FROM DbVersion", [], |r| r.get(0))?)
    }

    /// Reads everything Kollate imports. On an untested database version, a
    /// failure is reported as [`Error::UntestedDb`] so the cause is clear.
    pub fn snapshot(&self) -> Result<KoboSnapshot> {
        let db_version = self.db_version()?;
        self.read_snapshot(db_version).map_err(|err| {
            if is_tested_db_version(db_version) {
                err
            } else {
                Error::UntestedDb {
                    version: db_version,
                    source: Box::new(err),
                }
            }
        })
    }

    fn read_snapshot(&self, db_version: i64) -> Result<KoboSnapshot> {
        let (bookmarks, hidden_count) = self.bookmarks()?;
        let words = self.words()?;
        let volume_ids: BTreeSet<&str> = bookmarks
            .iter()
            .map(|b| b.volume_id.as_str())
            .chain(words.iter().filter_map(|w| w.volume_id.as_deref()))
            .collect();
        let books = self.books(&volume_ids)?;
        Ok(KoboSnapshot {
            db_version,
            books,
            bookmarks,
            words,
            hidden_count,
        })
    }

    fn books(&self, volume_ids: &BTreeSet<&str>) -> Result<Vec<KoboBook>> {
        let mut stmt = self.conn.prepare(
            "SELECT ContentID, Title, Attribution, Publisher, ISBN, Language, Series,
                    SeriesNumber, ImageId, ___PercentRead, DateLastRead
             FROM content WHERE ContentType = 6 AND ContentID = ?1",
        )?;
        let mut books = Vec::new();
        for id in volume_ids {
            let book = stmt.query_row([id], |r| {
                Ok(KoboBook {
                    volume_id: text(r, 0)?.unwrap_or_default(),
                    title: clean_opt(text(r, 1)?.as_deref()).unwrap_or_else(|| "Untitled".into()),
                    author: clean_opt(text(r, 2)?.as_deref()),
                    publisher: clean_opt(text(r, 3)?.as_deref()),
                    isbn: clean_opt(text(r, 4)?.as_deref()),
                    language: clean_opt(text(r, 5)?.as_deref()),
                    series: clean_opt(text(r, 6)?.as_deref()),
                    series_number: clean_opt(text(r, 7)?.as_deref()),
                    image_id: clean_opt(text(r, 8)?.as_deref()),
                    percent_read: int(r, 9)?,
                    last_read: parse_kobo_date(text(r, 10)?.as_deref()),
                })
            });
            match book {
                Ok(b) => books.push(b),
                // Book removed from the device but its annotations remain.
                Err(rusqlite::Error::QueryReturnedNoRows) => books.push(KoboBook {
                    volume_id: id.to_string(),
                    title: title_from_volume_id(id),
                    author: None,
                    publisher: None,
                    isbn: None,
                    language: None,
                    series: None,
                    series_number: None,
                    image_id: None,
                    percent_read: None,
                    last_read: None,
                }),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(books)
    }

    fn chapter_indexes(
        &self,
        volume_ids: &BTreeSet<String>,
    ) -> Result<HashMap<String, ChapterIndex>> {
        let mut indexes: HashMap<String, ChapterIndex> = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT ContentID, ContentType, Title, VolumeIndex FROM content
             WHERE BookID = ?1 AND ContentType IN (9, 899)",
        )?;
        for vid in volume_ids {
            let ix = indexes.entry(vid.clone()).or_default();
            let mut rows = stmt.query([vid])?;
            while let Some(r) = rows.next()? {
                let id = text(r, 0)?.unwrap_or_default();
                let index = int(r, 3)?.unwrap_or(0);
                match int(r, 1)? {
                    Some(9) => ix.add_spine_file(&id, index),
                    _ => ix.add_toc_entry(
                        &id,
                        &clean_opt(text(r, 2)?.as_deref()).unwrap_or_default(),
                        index,
                    ),
                }
            }
        }
        Ok(indexes)
    }

    /// Returns visible bookmarks in reading order, plus the number of hidden ones.
    fn bookmarks(&self) -> Result<(Vec<KoboBookmark>, usize)> {
        let mut stmt = self.conn.prepare(
            "SELECT BookmarkID, VolumeID, ContentID, Type, Text, Annotation, Color,
                    StartContainerPath, StartContainerChildIndex, StartOffset,
                    EndContainerPath, EndContainerChildIndex, EndOffset,
                    ChapterProgress, DateCreated, DateModified, Hidden, ExtraAnnotationData
             FROM Bookmark",
        )?;
        let mut hidden = 0;
        let mut bookmarks = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            if boolean(r, 16)? {
                hidden += 1;
                continue;
            }
            bookmarks.push(KoboBookmark {
                bookmark_id: text(r, 0)?.unwrap_or_default(),
                volume_id: text(r, 1)?.unwrap_or_default(),
                content_id: text(r, 2)?.unwrap_or_default(),
                kind: AnnotationKind::from_kobo(text(r, 3)?.as_deref()),
                text: clean_opt(text(r, 4)?.as_deref()),
                note: clean_opt(text(r, 5)?.as_deref()),
                color: int(r, 6)?.unwrap_or(0),
                start: Position {
                    container_path: text(r, 7)?.unwrap_or_default(),
                    child_index: int(r, 8)?.unwrap_or(0),
                    offset: int(r, 9)?.unwrap_or(0),
                },
                end: Position {
                    container_path: text(r, 10)?.unwrap_or_default(),
                    child_index: int(r, 11)?.unwrap_or(0),
                    offset: int(r, 12)?.unwrap_or(0),
                },
                chapter_progress: r.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
                created: parse_kobo_date(text(r, 14)?.as_deref()),
                modified: parse_kobo_date(text(r, 15)?.as_deref()),
                extra_data: blob(r, 17)?,
                chapter_title: None,
                spine_index: None,
            });
        }

        let volume_ids = bookmarks.iter().map(|b| b.volume_id.clone()).collect();
        let indexes = self.chapter_indexes(&volume_ids)?;
        let mut exact = Vec::with_capacity(bookmarks.len());
        for b in &mut bookmarks {
            let resolved = indexes.get(&b.volume_id).and_then(|ix| {
                b.spine_index = ix.spine_index(&b.content_id);
                ix.resolve(&b.content_id)
            });
            b.chapter_title = resolved.map(|(t, _)| t.to_owned());
            exact.push(resolved.is_none_or(|(_, r)| r == Resolution::Exact));
        }
        let mut order: Vec<usize> = (0..bookmarks.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&bookmarks[a], &bookmarks[b]);
            (&a.volume_id, a.reading_order_key()).cmp(&(&b.volume_id, b.reading_order_key()))
        });
        carry_chapters_forward(&mut bookmarks, &exact, &order);
        let mut sorted: Vec<Option<KoboBookmark>> = bookmarks.into_iter().map(Some).collect();
        let bookmarks = order
            .iter()
            .map(|&i| sorted[i].take().expect("each index once"))
            .collect();
        Ok((bookmarks, hidden))
    }

    fn words(&self) -> Result<Vec<KoboWord>> {
        let mut stmt = self.conn.prepare(
            "SELECT Text, VolumeId, DictSuffix, DateCreated FROM WordList ORDER BY DateCreated",
        )?;
        let words = stmt
            .query_map([], |r| {
                Ok(KoboWord {
                    word: clean_opt(text(r, 0)?.as_deref()).unwrap_or_default(),
                    volume_id: text(r, 1)?.filter(|s| !s.is_empty()),
                    language: text(r, 2)?
                        .map(|s| s.trim_start_matches('-').to_owned())
                        .filter(|s| !s.is_empty()),
                    created: parse_kobo_date(text(r, 3)?.as_deref()),
                })
            })?
            .filter(|w| w.as_ref().map_or(true, |w| !w.word.is_empty()))
            .collect::<rusqlite::Result<_>>()?;
        Ok(words)
    }
}

/// For bookmarks whose chapter was only guessed from the start of a file that
/// holds several chapters, use the chapter of the closest preceding bookmark in
/// the same file whose chapter is known exactly. `order` is reading order.
fn carry_chapters_forward(bookmarks: &mut [KoboBookmark], exact: &[bool], order: &[usize]) {
    let file = |b: &KoboBookmark| {
        b.content_id
            .split('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    let mut last_exact: Option<(String, Option<String>)> = None;
    for &i in order {
        let f = file(&bookmarks[i]);
        if exact[i] {
            last_exact = Some((f, bookmarks[i].chapter_title.clone()));
        } else if let Some((lf, title)) = &last_exact
            && *lf == f
        {
            bookmarks[i].chapter_title = title.clone();
        }
    }
}

/// Best-effort title from a sideloaded path, e.g.
/// `file:///mnt/onboard/Author/Title - Author.kepub.epub` becomes `Title - Author`.
fn title_from_volume_id(id: &str) -> String {
    let name = id.rsplit('/').next().unwrap_or(id);
    name.trim_end_matches(".epub")
        .trim_end_matches(".kepub")
        .to_owned()
}
