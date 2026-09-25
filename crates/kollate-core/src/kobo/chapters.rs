//! Resolves the chapter title for a bookmark from the book's table of
//! contents (`content` rows with `ContentType = 899`).
//!
//! IDs look like `<book path>!OEBPS!chapter009.xhtml#Ref_16114`. TOC entries
//! carry an extra `-N` suffix (`...#Ref_16114-1`), and spine files
//! (`ContentType = 9`) have no fragment and give reading order.

use std::collections::HashMap;

#[derive(Debug, Clone)]
struct TocEntry {
    fragment: Option<String>,
    title: String,
    toc_index: i64,
}

#[derive(Debug, Default)]
pub struct ChapterIndex {
    /// Chapter file → TOC entries in that file, in TOC order.
    toc: HashMap<String, Vec<TocEntry>>,
    /// Chapter file → spine index.
    spine: HashMap<String, i64>,
}

/// Splits a content ID into (file, fragment).
fn split_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once('#') {
        Some((file, frag)) => (file, Some(frag)),
        None => (id, None),
    }
}

/// Strips the trailing `-N` Kobo appends to TOC content IDs.
fn strip_toc_suffix(id: &str) -> &str {
    match id.rsplit_once('-') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => id,
    }
}

impl ChapterIndex {
    pub fn add_toc_entry(&mut self, content_id: &str, title: &str, toc_index: i64) {
        let (file, frag) = split_id(strip_toc_suffix(content_id));
        let entries = self.toc.entry(file.to_owned()).or_default();
        entries.push(TocEntry {
            fragment: frag.map(str::to_owned),
            title: title.to_owned(),
            toc_index,
        });
        entries.sort_by_key(|e| e.toc_index);
    }

    pub fn add_spine_file(&mut self, content_id: &str, spine_index: i64) {
        self.spine.insert(content_id.to_owned(), spine_index);
    }

    pub fn spine_index(&self, content_id: &str) -> Option<i64> {
        self.spine.get(split_id(content_id).0).copied()
    }

    /// Chapter title for a bookmark's `ContentID`:
    /// 1. the TOC entry with the same file and fragment ([`Resolution::Exact`]);
    /// 2. otherwise the first TOC entry in the same file ([`Resolution::FileStart`]);
    ///    callers may refine this using neighbouring bookmarks;
    /// 3. otherwise the last TOC entry of the nearest preceding spine file
    ///    (chapters split across several files).
    pub fn resolve(&self, content_id: &str) -> Option<(&str, Resolution)> {
        let (file, frag) = split_id(content_id);
        if let Some(entries) = self.toc.get(file) {
            if let Some(e) =
                frag.and_then(|f| entries.iter().find(|e| e.fragment.as_deref() == Some(f)))
            {
                return Some((&e.title, Resolution::Exact));
            }
            let kind = if entries.len() > 1 {
                Resolution::FileStart
            } else {
                Resolution::Exact
            };
            return entries.first().map(|e| (e.title.as_str(), kind));
        }
        let here = self.spine.get(file)?;
        self.toc
            .iter()
            .filter_map(|(f, entries)| Some((*self.spine.get(f)?, entries)))
            .filter(|(idx, _)| idx < here)
            .max_by_key(|(idx, _)| *idx)
            .and_then(|(_, entries)| entries.last())
            .map(|e| (e.title.as_str(), Resolution::Exact))
    }

    pub fn chapter_title(&self, content_id: &str) -> Option<&str> {
        self.resolve(content_id).map(|(t, _)| t)
    }
}

/// How confidently a chapter was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Exact,
    /// The file holds several TOC entries and none matched the bookmark's
    /// anchor; the first entry was used.
    FileStart,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> ChapterIndex {
        let mut ix = ChapterIndex::default();
        ix.add_toc_entry("b!OEBPS!a.xhtml#p1-1", "Part One", 1);
        ix.add_toc_entry("b!OEBPS!a.xhtml#p2-1", "Chapter 1", 2);
        ix.add_toc_entry("b!OEBPS!c.xhtml-1", "Chapter 2", 3);
        ix.add_spine_file("b!OEBPS!a.xhtml", 0);
        ix.add_spine_file("b!OEBPS!a_split.xhtml", 1);
        ix.add_spine_file("b!OEBPS!c.xhtml", 2);
        ix
    }

    #[test]
    fn resolves_by_fragment_file_and_preceding_spine() {
        let ix = index();
        assert_eq!(ix.chapter_title("b!OEBPS!a.xhtml#p2"), Some("Chapter 1"));
        assert_eq!(ix.chapter_title("b!OEBPS!a.xhtml#zzz"), Some("Part One"));
        assert_eq!(ix.chapter_title("b!OEBPS!c.xhtml"), Some("Chapter 2"));
        assert_eq!(
            ix.chapter_title("b!OEBPS!a_split.xhtml#x"),
            Some("Chapter 1")
        );
        assert_eq!(ix.spine_index("b!OEBPS!c.xhtml#frag"), Some(2));
        assert_eq!(ix.chapter_title("b!OEBPS!unknown.xhtml"), None);
        assert_eq!(
            ix.resolve("b!OEBPS!a.xhtml").unwrap().1,
            Resolution::FileStart
        );
        assert_eq!(ix.resolve("b!OEBPS!c.xhtml").unwrap().1, Resolution::Exact);
    }
}
