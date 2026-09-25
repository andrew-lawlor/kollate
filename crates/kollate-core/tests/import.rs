//! De-duplication and merge behaviour of `Library::import` (SPEC §6).

use std::path::PathBuf;

use kollate_core::kobo::{DeviceInfo, KoboDb, KoboSnapshot, KoboWord};
use kollate_core::store::Status;
use kollate_core::{ImportStats, Library};

fn fixture() -> KoboSnapshot {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite");
    KoboDb::open_copy(&path).unwrap().snapshot().unwrap()
}

fn device(serial: &str) -> DeviceInfo {
    DeviceInfo {
        serial: serial.into(),
        firmware: None,
        model_id: None,
    }
}

fn imported() -> (Library, KoboSnapshot) {
    let mut lib = Library::open_in_memory().unwrap();
    let snap = fixture();
    lib.import(&snap, &device("A"), false).unwrap();
    (lib, snap)
}

#[test]
fn first_import_brings_in_everything() {
    let mut lib = Library::open_in_memory().unwrap();
    let stats = lib.import(&fixture(), &device("A"), false).unwrap();
    assert_eq!(
        stats,
        ImportStats {
            books_new: 5,
            annotations_new: 52,
            words_new: 12,
            word_sightings_new: 12,
            ..Default::default()
        }
    );
    let counts = lib.counts().unwrap();
    assert_eq!(
        (counts.books, counts.annotations, counts.vocab),
        (5, 52, 12)
    );
}

#[test]
fn reimport_changes_nothing() {
    let (mut lib, snap) = imported();
    let stats = lib.import(&snap, &device("A"), false).unwrap();
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(stats.annotations_unchanged, 52);
    assert_eq!(lib.counts().unwrap().annotations, 52);
}

#[test]
fn dry_run_writes_nothing() {
    let mut lib = Library::open_in_memory().unwrap();
    let stats = lib.import(&fixture(), &device("A"), true).unwrap();
    assert_eq!(stats.annotations_new, 52);
    assert_eq!(lib.counts().unwrap().annotations, 0);
}

#[test]
fn device_edit_updates_but_user_edits_win() {
    let (mut lib, mut snap) = imported();
    let bm = snap
        .bookmarks
        .iter_mut()
        .find(|b| b.note.as_deref() == Some("Nice simile"))
        .unwrap();
    let id = lib
        .annotation_id_for_bookmark(&bm.bookmark_id)
        .unwrap()
        .unwrap();

    // Plain device edit: picked up.
    bm.note = Some("Great simile".into());
    let stats = lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!((stats.annotations_updated, stats.annotations_new), (1, 0));
    assert_eq!(
        lib.annotation(id).unwrap().unwrap().note(),
        Some("Great simile")
    );

    // User edits in Kollate, then the device changes again: user's version stays.
    lib.set_user_note(id, Some("My note")).unwrap();
    let bm = snap
        .bookmarks
        .iter_mut()
        .find(|b| b.note.as_deref() == Some("Great simile"))
        .unwrap();
    bm.note = Some("Device note v3".into());
    lib.import(&snap, &device("A"), false).unwrap();
    let a = lib.annotation(id).unwrap().unwrap();
    assert_eq!(a.note(), Some("My note"));
    assert_eq!(a.device_note.as_deref(), Some("Device note v3"));
    assert!(a.device_changed_at.is_some());

    lib.accept_device_version(id).unwrap();
    assert_eq!(
        lib.annotation(id).unwrap().unwrap().note(),
        Some("Device note v3")
    );
}

#[test]
fn deleted_on_device_is_kept_and_flagged_then_restored() {
    let (mut lib, snap) = imported();
    let mut fewer = snap.clone();
    let removed = fewer.bookmarks.remove(0);
    let id = lib
        .annotation_id_for_bookmark(&removed.bookmark_id)
        .unwrap()
        .unwrap();

    let stats = lib.import(&fewer, &device("A"), false).unwrap();
    assert_eq!(stats.annotations_removed, 1);
    assert_eq!(lib.counts().unwrap().annotations, 52);
    assert!(
        lib.annotation(id)
            .unwrap()
            .unwrap()
            .removed_on_device_at
            .is_some()
    );

    // Not flagged twice.
    assert_eq!(
        lib.import(&fewer, &device("A"), false)
            .unwrap()
            .annotations_removed,
        0
    );

    let stats = lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!((stats.annotations_restored, stats.annotations_new), (1, 0));
    assert!(
        lib.annotation(id)
            .unwrap()
            .unwrap()
            .removed_on_device_at
            .is_none()
    );
}

#[test]
fn factory_reset_with_new_ids_is_not_duplicated() {
    let (mut lib, snap) = imported();
    let mut reset = snap.clone();
    for (i, b) in reset.bookmarks.iter_mut().enumerate() {
        b.bookmark_id = format!("new-id-{i}");
    }
    let stats = lib.import(&reset, &device("A"), false).unwrap();
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(stats.annotations_unchanged, 52);
    assert_eq!(lib.counts().unwrap().annotations, 52);
}

#[test]
fn second_device_shares_the_library() {
    let (mut lib, snap) = imported();
    // Same highlights synced via Kobo cloud (same BookmarkIDs) on another device.
    let stats = lib.import(&snap, &device("B"), false).unwrap();
    assert_eq!(
        (stats.books_new, stats.annotations_new, stats.words_new),
        (0, 0, 0)
    );
    // The first device isn't affected by what the second one lacks.
    let mut partial = snap.clone();
    partial.bookmarks.clear();
    let stats = lib.import(&partial, &device("B"), false).unwrap();
    assert_eq!(stats.annotations_removed, 0, "sources belong to device A");
}

#[test]
fn vocab_merges_case_and_records_sightings() {
    let (mut lib, mut snap) = imported();
    let other_book = snap
        .books
        .iter()
        .find(|b| b.title.contains("Solomon Kane"))
        .unwrap()
        .volume_id
        .clone();
    snap.words.push(KoboWord {
        word: "demiurge".into(),
        volume_id: Some(other_book),
        language: Some("en".into()),
        created: None,
    });
    let stats = lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!((stats.words_new, stats.word_sightings_new), (0, 1));
    let demiurge = lib
        .vocab()
        .unwrap()
        .into_iter()
        .find(|v| v.word == "Demiurge")
        .unwrap();
    assert_eq!(demiurge.books.len(), 2);

    // Word cleared from Vocab Builder on the device: kept in the library.
    snap.words.clear();
    lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!(lib.counts().unwrap().vocab, 12);
}

#[test]
fn user_status_survives_reimport() {
    let (mut lib, snap) = imported();
    let id = lib
        .annotation_id_for_bookmark(&snap.bookmarks[3].bookmark_id)
        .unwrap()
        .unwrap();
    lib.set_status(id, Status::Archived).unwrap();
    lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!(
        lib.annotation(id).unwrap().unwrap().status,
        Status::Archived
    );
}

#[test]
fn books_list_in_reading_order() {
    let (lib, _) = imported();
    let books = lib.books().unwrap();
    assert_eq!(books.len(), 5);
    let tantra = books
        .iter()
        .find(|b| b.title.starts_with("Hellenic"))
        .unwrap();
    assert_eq!((tantra.annotation_count, tantra.vocab_count), (41, 10));
    let items = lib.annotations_for_book(tantra.id).unwrap();
    assert_eq!(items[0].chapter_title.as_deref(), Some("Preface"));
}

#[test]
fn attaches_copied_assets_and_stores_settings() {
    use kollate_core::kobo::assets::CopiedAssets;
    let (lib, snap) = imported();
    let markup = snap
        .bookmarks
        .iter()
        .find(|b| b.kind == kollate_core::kobo::AnnotationKind::Markup)
        .unwrap();
    let book = &snap.books[0];
    let assets = CopiedAssets {
        covers: vec![(book.volume_id.clone(), "/lib/covers/x.jpg".into())],
        markups: vec![(
            markup.bookmark_id.clone(),
            None,
            Some("/lib/markups/m.jpg".into()),
        )],
    };
    lib.attach_assets(&device("A"), &assets).unwrap();

    let id = lib
        .annotation_id_for_bookmark(&markup.bookmark_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        lib.annotation(id).unwrap().unwrap().markup_image,
        Some("/lib/markups/m.jpg".into())
    );
    let covered = lib
        .books()
        .unwrap()
        .into_iter()
        .filter(|b| b.cover.is_some())
        .count();
    assert_eq!(covered, 1);

    assert_eq!(lib.setting("on_connect").unwrap(), None);
    lib.set_setting("on_connect", "ask").unwrap();
    lib.set_setting("on_connect", "auto").unwrap();
    assert_eq!(lib.setting("on_connect").unwrap().as_deref(), Some("auto"));
}

#[test]
fn deleted_on_device_view_and_count() {
    use kollate_core::store::{AnnotationFilter, View};
    let (mut lib, snap) = imported();
    let mut fewer = snap.clone();
    let removed = fewer.bookmarks.remove(0);
    let id = lib
        .annotation_id_for_bookmark(&removed.bookmark_id)
        .unwrap()
        .unwrap();
    lib.import(&fewer, &device("A"), false).unwrap();

    let view = lib
        .query_annotations(&AnnotationFilter {
            view: View::RemovedOnDevice,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(view.iter().map(|a| a.id).collect::<Vec<_>>(), [id]);
    assert_eq!(lib.sidebar_counts().unwrap().removed_on_device, 1);

    // Trashed items leave the view.
    lib.set_status(id, Status::Trashed).unwrap();
    assert_eq!(lib.sidebar_counts().unwrap().removed_on_device, 0);
}

#[test]
fn auto_trash_policy_trashes_and_restores() {
    use kollate_core::store::DeviceDeletePolicy;
    let (mut lib, snap) = imported();
    assert_eq!(
        lib.device_delete_policy().unwrap(),
        DeviceDeletePolicy::Keep
    );
    lib.set_device_delete_policy(DeviceDeletePolicy::Trash)
        .unwrap();

    let first = lib
        .annotation_id_for_bookmark(&snap.bookmarks[0].bookmark_id)
        .unwrap()
        .unwrap();
    let second = lib
        .annotation_id_for_bookmark(&snap.bookmarks[1].bookmark_id)
        .unwrap()
        .unwrap();
    lib.set_status(first, Status::Kept).unwrap();
    let mut fewer = snap.clone();
    fewer.bookmarks.drain(0..2);

    let stats = lib.import(&fewer, &device("A"), false).unwrap();
    assert_eq!(
        (stats.annotations_removed, stats.annotations_trashed),
        (2, 2)
    );
    assert_eq!(
        lib.annotation(first).unwrap().unwrap().status,
        Status::Trashed
    );

    // The user rescues the second one by hand; it must stay where they put it.
    lib.set_status(second, Status::Archived).unwrap();

    // Both reappear on the Kobo.
    let stats = lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!(stats.annotations_restored, 2);
    let a = lib.annotation(first).unwrap().unwrap();
    assert_eq!(
        (a.status, a.removed_on_device_at),
        (Status::Kept, None),
        "back to its previous status"
    );
    assert_eq!(
        lib.annotation(second).unwrap().unwrap().status,
        Status::Archived,
        "manual choice kept"
    );

    // A highlight the user trashed themselves isn't un-trashed by a restore.
    lib.set_status(first, Status::Trashed).unwrap();
    let mut again = snap.clone();
    again.bookmarks.remove(0);
    let stats = lib.import(&again, &device("A"), false).unwrap();
    assert_eq!(stats.annotations_trashed, 0, "already in Trash");
    lib.import(&snap, &device("A"), false).unwrap();
    assert_eq!(
        lib.annotation(first).unwrap().unwrap().status,
        Status::Trashed
    );
}
