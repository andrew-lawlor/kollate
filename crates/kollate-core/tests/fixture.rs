//! Tests against a real Kobo Libra Colour database (DbVersion 176).

use std::path::PathBuf;

use kollate_core::kobo::{AnnotationKind, KoboDb, find_kobo_db};

fn snapshot() -> kollate_core::kobo::KoboSnapshot {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite");
    KoboDb::open_copy(&find_kobo_db(&path).unwrap())
        .unwrap()
        .snapshot()
        .unwrap()
}

#[test]
fn reads_all_items() {
    let s = snapshot();
    assert_eq!(s.db_version, 176);
    assert_eq!(s.bookmarks.len(), 52);
    assert_eq!(s.hidden_count, 0);
    assert_eq!(s.words.len(), 12);
    assert_eq!(s.books.len(), 5);
    assert!(
        s.books
            .iter()
            .all(|b| b.is_sideloaded() && b.author.is_some())
    );
}

#[test]
fn classifies_kinds_and_cleans_text() {
    let s = snapshot();
    let count = |k: AnnotationKind| s.bookmarks.iter().filter(|b| b.kind == k).count();
    assert_eq!(count(AnnotationKind::Highlight), 49);
    assert_eq!(count(AnnotationKind::Note), 2);
    assert_eq!(count(AnnotationKind::Markup), 1);

    let markup = s
        .bookmarks
        .iter()
        .find(|b| b.kind == AnnotationKind::Markup)
        .unwrap();
    assert!(markup.text.is_none() && markup.extra_data.is_some());

    let note = s
        .bookmarks
        .iter()
        .find(|b| b.note.as_deref() == Some("Nice simile"))
        .unwrap();
    assert_eq!(
        note.text.as_deref(),
        Some("The moon began to rise, lean and haggard, like a skull among the stars.")
    );
    for b in s.bookmarks.iter().filter_map(|b| b.text.as_deref()) {
        assert_eq!(b, b.trim());
    }
}

#[test]
fn resolves_chapter_titles() {
    let s = snapshot();
    let titled = s
        .bookmarks
        .iter()
        .filter(|b| b.chapter_title.is_some())
        .count();
    assert_eq!(
        titled,
        s.bookmarks.len(),
        "every bookmark should resolve a chapter"
    );
    let ash = s
        .bookmarks
        .iter()
        .find(|b| b.content_id.contains("chapter009.xhtml#Ref_16114"))
        .unwrap();
    assert_eq!(ash.chapter_title.as_deref(), Some("9. Inroads"));
    let kjv = s
        .bookmarks
        .iter()
        .find(|b| b.content_id.ends_with("#pgepubid00042"))
        .unwrap();
    assert_eq!(
        kjv.chapter_title.as_deref(),
        Some("The Gospel According to Saint Matthew")
    );
    // The markup has no anchor, but follows Matthew highlights in the same file.
    let markup = s
        .bookmarks
        .iter()
        .find(|b| b.kind == AnnotationKind::Markup)
        .unwrap();
    assert_eq!(
        markup.chapter_title.as_deref(),
        Some("The Gospel According to Saint Matthew")
    );
}

#[test]
fn reads_vocab() {
    let s = snapshot();
    let w = s.words.iter().find(|w| w.word == "séances").unwrap();
    assert_eq!(w.language.as_deref(), Some("en"));
    assert!(w.volume_id.as_deref().unwrap().contains("Hellenic Tantra"));
    assert!(w.created.is_some());
}

/// A writable copy of the fixture, changed by `sql`.
fn modified_fixture(sql: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("KoboReader.sqlite");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite"),
        &path,
    )
    .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
    (dir, path)
}

#[test]
fn reads_untested_versions() {
    assert!(kollate_core::kobo::is_tested_db_version(176));
    let (_dir, path) = modified_fixture("UPDATE DbVersion SET version = 999");
    let s = KoboDb::open_copy(&path).unwrap().snapshot().unwrap();
    assert_eq!(s.db_version, 999);
    assert!(!kollate_core::kobo::is_tested_db_version(s.db_version));
    assert_eq!(s.bookmarks.len(), 52);
}

#[test]
fn explains_failures_on_untested_versions() {
    let (_dir, path) = modified_fixture(
        "UPDATE DbVersion SET version = 999; ALTER TABLE WordList RENAME COLUMN Text TO Word",
    );
    let err = KoboDb::open_copy(&path).unwrap().snapshot().unwrap_err();
    assert!(matches!(
        err,
        kollate_core::Error::UntestedDb { version: 999, .. }
    ));
    assert!(err.to_string().contains("version 999"), "{err}");

    // On a tested version the plain error comes through.
    let (_dir, path) = modified_fixture("ALTER TABLE WordList RENAME COLUMN Text TO Word");
    let err = KoboDb::open_copy(&path).unwrap().snapshot().unwrap_err();
    assert!(matches!(err, kollate_core::Error::Sqlite(_)), "{err}");
}
