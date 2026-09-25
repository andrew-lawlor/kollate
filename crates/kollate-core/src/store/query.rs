//! Queries and curation actions used by the UI.

use chrono::Utc;
use rusqlite::{ToSql, params};
use serde::Serialize;

use super::{ANNOTATION_SELECT, Annotation, Library, annotation_from_row};
use crate::Result;

/// A list of annotations the UI can show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub enum View {
    /// New since the last review.
    #[default]
    Inbox,
    /// Everything that is neither archived nor trashed.
    All,
    Notes,
    Markups,
    /// Starred items that aren't trashed.
    Starred,
    Archive,
    Trash,
    /// A book's active annotations, in reading order.
    Book(i64),
    /// Active annotations with a tag.
    Tag(i64),
}

#[derive(Debug, Clone, Default)]
pub struct AnnotationFilter {
    pub view: View,
    /// Case-insensitive substring over text, note, chapter, book and tags.
    pub search: Option<String>,
    /// Restrict to one annotation (used to check whether it still matches).
    pub id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SidebarCounts {
    pub inbox: i64,
    pub all: i64,
    pub notes: i64,
    pub markups: i64,
    pub starred: i64,
    pub archive: i64,
    pub trash: i64,
    pub vocab: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// Active annotations with this tag.
    pub count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VocabStatus {
    New,
    Learning,
    Known,
    Ignored,
}

impl VocabStatus {
    pub const ALL: [Self; 4] = [Self::New, Self::Learning, Self::Known, Self::Ignored];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Learning => "learning",
            Self::Known => "known",
            Self::Ignored => "ignored",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "learning" => Self::Learning,
            "known" => Self::Known,
            "ignored" => Self::Ignored,
            _ => Self::New,
        }
    }
}

const ACTIVE: &str = "a.status IN ('inbox', 'kept')";

/// `LIKE` pattern matching `s` anywhere, with wildcards escaped by `\`.
pub(crate) fn like_pattern(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

impl Library {
    /// Annotations for a view. A book view is in reading order; other views
    /// are grouped by book (most recently annotated book first) and in
    /// reading order within each book.
    pub fn query_annotations(&self, filter: &AnnotationFilter) -> Result<Vec<Annotation>> {
        let mut conditions: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        fn arg(value: Box<dyn ToSql>, args: &mut Vec<Box<dyn ToSql>>) -> String {
            args.push(value);
            format!("?{}", args.len())
        }

        match filter.view {
            View::Inbox => conditions.push("a.status = 'inbox'".into()),
            View::All => conditions.push(ACTIVE.into()),
            View::Notes => conditions.push(format!(
                "{ACTIVE} AND nullif(coalesce(a.user_note, a.device_note), '') IS NOT NULL"
            )),
            View::Markups => conditions.push(format!("{ACTIVE} AND a.kind = 'markup'")),
            View::Starred => conditions.push("a.starred AND a.status != 'trashed'".into()),
            View::Archive => conditions.push("a.status = 'archived'".into()),
            View::Trash => conditions.push("a.status = 'trashed'".into()),
            View::Book(id) => {
                let p = arg(Box::new(id), &mut args);
                conditions.push(format!("{ACTIVE} AND a.book_id = {p}"));
            }
            View::Tag(id) => {
                let p = arg(Box::new(id), &mut args);
                conditions.push(format!(
                    "{ACTIVE} AND EXISTS (SELECT 1 FROM annotation_tag x WHERE x.annotation_id = a.id AND x.tag_id = {p})"
                ));
            }
        }
        if let Some(id) = filter.id {
            let p = arg(Box::new(id), &mut args);
            conditions.push(format!("a.id = {p}"));
        }
        if let Some(search) = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let p = arg(Box::new(like_pattern(search)), &mut args);
            conditions.push(format!(
                "(coalesce(a.user_text, a.device_text, '') LIKE {p} ESCAPE '\\'
                  OR coalesce(a.user_note, a.device_note, '') LIKE {p} ESCAPE '\\'
                  OR coalesce(a.chapter_title, '') LIKE {p} ESCAPE '\\'
                  OR coalesce(b.user_title, b.title) LIKE {p} ESCAPE '\\'
                  OR coalesce(b.user_author, b.author, '') LIKE {p} ESCAPE '\\'
                  OR EXISTS (SELECT 1 FROM annotation_tag x JOIN tag t ON t.id = x.tag_id
                             WHERE x.annotation_id = a.id AND t.name LIKE {p} ESCAPE '\\'))"
            ));
        }

        let sql = format!(
            "{ANNOTATION_SELECT} WHERE {}
             ORDER BY (SELECT max(created_at) FROM annotation y WHERE y.book_id = a.book_id) DESC, a.book_id,
                      a.spine_index IS NULL, a.spine_index, a.position_key, a.id",
            conditions.join(" AND ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn ToSql> = args.iter().map(|a| a.as_ref()).collect();
        Ok(stmt
            .query_map(params.as_slice(), annotation_from_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Whether annotation `id` still belongs in `view` (e.g. after archiving).
    pub fn annotation_in_view(&self, id: i64, view: View) -> Result<bool> {
        Ok(!self
            .query_annotations(&AnnotationFilter {
                view,
                id: Some(id),
                search: None,
            })?
            .is_empty())
    }

    pub fn sidebar_counts(&self) -> Result<SidebarCounts> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT count(*) FILTER (WHERE a.status = 'inbox'),
                        count(*) FILTER (WHERE {ACTIVE}),
                        count(*) FILTER (WHERE {ACTIVE} AND nullif(coalesce(a.user_note, a.device_note), '') IS NOT NULL),
                        count(*) FILTER (WHERE {ACTIVE} AND a.kind = 'markup'),
                        count(*) FILTER (WHERE a.starred AND a.status != 'trashed'),
                        count(*) FILTER (WHERE a.status = 'archived'),
                        count(*) FILTER (WHERE a.status = 'trashed'),
                        (SELECT count(*) FROM vocab)
                 FROM annotation a"
            ),
            [],
            |r| {
                Ok(SidebarCounts {
                    inbox: r.get(0)?,
                    all: r.get(1)?,
                    notes: r.get(2)?,
                    markups: r.get(3)?,
                    starred: r.get(4)?,
                    archive: r.get(5)?,
                    trash: r.get(6)?,
                    vocab: r.get(7)?,
                })
            },
        )?)
    }

    pub fn set_starred(&self, id: i64, starred: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE annotation SET starred = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, starred, Utc::now()],
        )?;
        Ok(())
    }

    /// Tags in use, with counts of active annotations.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT t.id, t.name, (SELECT count(*) FROM annotation_tag x JOIN annotation a ON a.id = x.annotation_id
                                   WHERE x.tag_id = t.id AND {ACTIVE})
             FROM tag t ORDER BY t.name COLLATE NOCASE"
        ))?;
        let tags = stmt.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(tags.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces an annotation's tags. Blank names are ignored, and tags no
    /// longer used anywhere are deleted.
    pub fn set_annotation_tags(&mut self, id: i64, names: &[&str]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM annotation_tag WHERE annotation_id = ?1", [id])?;
        for name in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
            tx.execute(
                "INSERT INTO tag (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
                [name],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO annotation_tag (annotation_id, tag_id)
                 SELECT ?1, id FROM tag WHERE name = ?2",
                params![id, name],
            )?;
        }
        tx.execute(
            "DELETE FROM tag WHERE id NOT IN (SELECT tag_id FROM annotation_tag)
                               AND id NOT IN (SELECT tag_id FROM vocab_tag)",
            [],
        )?;
        tx.execute(
            "UPDATE annotation SET updated_at = ?2 WHERE id = ?1",
            params![id, Utc::now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_vocab_status(&self, id: i64, status: VocabStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE vocab SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), Utc::now()],
        )?;
        Ok(())
    }
}
