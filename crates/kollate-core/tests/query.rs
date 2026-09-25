//! Views, search, tags and curation actions used by the UI.

use std::path::PathBuf;

use kollate_core::Library;
use kollate_core::kobo::{DeviceInfo, KoboDb};
use kollate_core::store::{AnnotationFilter, Status, View, VocabStatus};

fn library() -> Library {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite");
    let snap = KoboDb::open_copy(&path).unwrap().snapshot().unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let device = DeviceInfo {
        serial: "A".into(),
        firmware: None,
        model_id: None,
    };
    lib.import(&snap, &device, false).unwrap();
    lib
}

fn view(lib: &Library, view: View) -> Vec<kollate_core::store::Annotation> {
    lib.query_annotations(&AnnotationFilter {
        view,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn views_and_counts_follow_status() {
    let lib = library();
    let c = lib.sidebar_counts().unwrap();
    assert_eq!(
        (c.inbox, c.all, c.notes, c.markups, c.archive, c.vocab),
        (52, 52, 2, 1, 0, 12)
    );

    let first = view(&lib, View::Inbox)[0].id;
    lib.set_status(first, Status::Archived).unwrap();
    assert!(!lib.annotation_in_view(first, View::Inbox).unwrap());
    assert!(lib.annotation_in_view(first, View::Archive).unwrap());
    lib.set_starred(first, true).unwrap();
    assert!(lib.annotation_in_view(first, View::Starred).unwrap());

    let c = lib.sidebar_counts().unwrap();
    assert_eq!((c.inbox, c.all, c.archive, c.starred), (51, 51, 1, 1));
}

#[test]
fn inbox_is_grouped_by_book_in_reading_order() {
    let lib = library();
    let items = view(&lib, View::Inbox);
    // Most recently annotated book first (Solomon Kane: Sep 2026).
    assert!(items[0].book_title.contains("Solomon Kane"));
    let mut seen = Vec::new();
    for a in &items {
        if seen.last() != Some(&a.book_id) {
            assert!(!seen.contains(&a.book_id), "books must be contiguous");
            seen.push(a.book_id);
        }
    }
    assert_eq!(seen.len(), 5);
}

#[test]
fn search_matches_text_notes_books_and_tags() {
    let mut lib = library();
    let search = |lib: &Library, q: &str| {
        lib.query_annotations(&AnnotationFilter {
            view: View::All,
            search: Some(q.into()),
            id: None,
        })
        .unwrap()
        .len()
    };
    assert_eq!(search(&lib, "SIMILE"), 1);
    assert_eq!(search(&lib, "Iamblichus"), 41);
    assert_eq!(search(&lib, "Inroads"), 1);
    assert_eq!(search(&lib, "100%"), 0, "wildcards are escaped");

    let id = view(&lib, View::Notes)[0].id;
    lib.set_annotation_tags(id, &["Xyzzy", " ", "prose"])
        .unwrap();
    assert_eq!(
        lib.annotation(id).unwrap().unwrap().tags,
        vec!["prose", "Xyzzy"]
    );
    assert_eq!(search(&lib, "xyz"), 1);
    let tag = lib
        .tags()
        .unwrap()
        .into_iter()
        .find(|t| t.name == "prose")
        .unwrap();
    assert_eq!(tag.count, 1);
    assert_eq!(view(&lib, View::Tag(tag.id)).len(), 1);

    lib.set_annotation_tags(id, &[]).unwrap();
    assert!(lib.tags().unwrap().is_empty(), "unused tags are removed");
}

#[test]
fn vocab_status_and_filters() {
    let lib = library();
    let words = lib.vocab().unwrap();
    assert_eq!(words[0].word, "debouched", "newest first");
    lib.set_vocab_status(words[0].id, VocabStatus::Known)
        .unwrap();
    assert_eq!(lib.vocab().unwrap()[0].status, VocabStatus::Known);

    let tantra = lib
        .books()
        .unwrap()
        .into_iter()
        .find(|b| b.title.starts_with("Hellenic"))
        .unwrap();
    assert_eq!(lib.query_vocab(Some(tantra.id), None).unwrap().len(), 10);
    assert_eq!(lib.query_vocab(None, Some("theoph")).unwrap().len(), 2);
}

#[test]
fn book_view_is_in_true_reading_order() {
    let lib = library();
    let book = lib
        .books()
        .unwrap()
        .into_iter()
        .find(|b| b.title.starts_with("Children of Ash"))
        .unwrap();
    let texts: Vec<String> = view(&lib, View::Book(book.id))
        .iter()
        .map(|a| a.text().unwrap().chars().take(12).collect())
        .collect();
    // kobo.3.1 < kobo.10.2 < kobo.10.4 (numeric, not text, comparison).
    assert_eq!(texts[..3], ["encircling o", "laughing aga", "they wave to"]);
}

#[test]
fn shelf_hides_books_with_nothing_active() {
    use kollate_core::store::BookSort;
    let lib = library();
    assert_eq!(lib.shelf(BookSort::Recent, None).unwrap().len(), 5);

    // Trash every highlight of one book, archive every highlight of another.
    let books = lib.books().unwrap();
    let ash = books
        .iter()
        .find(|b| b.title.starts_with("Children of Ash"))
        .unwrap()
        .id;
    let sagas = books
        .iter()
        .find(|b| b.title.starts_with("The Sagas"))
        .unwrap()
        .id;
    for a in view(&lib, View::Book(ash)) {
        lib.set_status(a.id, Status::Trashed).unwrap();
    }
    for a in view(&lib, View::Book(sagas)) {
        lib.set_status(a.id, Status::Archived).unwrap();
    }
    let shelf: Vec<i64> = lib
        .shelf(BookSort::Title, None)
        .unwrap()
        .iter()
        .map(|b| b.id)
        .collect();
    assert_eq!(shelf.len(), 3);
    assert!(!shelf.contains(&ash) && !shelf.contains(&sagas));
    assert_eq!(
        lib.books().unwrap().len(),
        5,
        "books() still lists everything"
    );

    // A book kept alive only by its words disappears when they're all ignored.
    let kane = books
        .iter()
        .find(|b| b.title.contains("Solomon Kane"))
        .unwrap()
        .id;
    for a in view(&lib, View::Book(kane)) {
        lib.set_status(a.id, Status::Archived).unwrap();
    }
    assert!(
        lib.shelf(BookSort::Title, None)
            .unwrap()
            .iter()
            .any(|b| b.id == kane),
        "has 2 words"
    );
    for w in lib.query_vocab(Some(kane), None).unwrap() {
        lib.set_vocab_status(w.id, VocabStatus::Ignored).unwrap();
    }
    assert!(
        !lib.shelf(BookSort::Title, None)
            .unwrap()
            .iter()
            .any(|b| b.id == kane)
    );
}

#[test]
fn shelf_sorts_and_searches() {
    use kollate_core::store::BookSort;
    let lib = library();
    let titles = |sort| {
        lib.shelf(sort, None)
            .unwrap()
            .into_iter()
            .map(|b| b.title)
            .collect::<Vec<_>>()
    };

    // Most recent activity: Solomon Kane (Sep 25 lookups), then the Bible/Tantra (late Aug)…
    let recent = titles(BookSort::Recent);
    assert!(recent[0].contains("Solomon Kane"), "{recent:?}");
    assert!(
        recent.last().unwrap().starts_with("Children of Ash"),
        "{recent:?}"
    );
    assert!(
        lib.shelf(BookSort::Recent, None)
            .unwrap()
            .iter()
            .all(|b| b.last_activity.is_some())
    );

    assert!(titles(BookSort::Title)[0].starts_with("Children of Ash"));
    let by_author: Vec<_> = lib
        .shelf(BookSort::Author, None)
        .unwrap()
        .into_iter()
        .map(|b| b.author)
        .collect();
    assert_eq!(by_author[0].as_deref(), Some("Jane Smilely"));

    let found = lib.shelf(BookSort::Title, Some("price")).unwrap();
    assert_eq!(found.len(), 1, "matches author Neil Price");
    assert_eq!(BookSort::parse(Some("author")), BookSort::Author);
    assert_eq!(BookSort::parse(None), BookSort::Recent);
}
