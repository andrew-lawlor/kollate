//! Pen marks and glosses (SPEC §8c): marks applied once, from typed and
//! handwritten notes, and circled words added to Vocabulary.

use std::path::PathBuf;

use kollate_core::Library;
use kollate_core::kobo::assets::{CopiedAssets, CopiedMarkup};
use kollate_core::kobo::{AnnotationKind, DeviceInfo, KoboDb, KoboSnapshot};
use kollate_core::markup::Transcription;
use kollate_core::store::{GLOSSES_SETTING, Status};

fn fixture() -> KoboSnapshot {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite");
    KoboDb::open_copy(&path).unwrap().snapshot().unwrap()
}

fn device() -> DeviceInfo {
    DeviceInfo {
        serial: "A".into(),
        firmware: None,
        model_id: None,
    }
}

fn annotation_id(lib: &Library, bookmark_id: &str) -> i64 {
    lib.annotation_for_bookmark(bookmark_id).unwrap().unwrap()
}

#[test]
fn typed_marks_apply_once() {
    let mut snap = fixture();
    let i = snap
        .bookmarks
        .iter()
        .position(|b| b.kind == AnnotationKind::Highlight)
        .unwrap();
    snap.bookmarks[i].note = Some("#Leadership\nNB".into());
    let bookmark = snap.bookmarks[i].bookmark_id.clone();
    let mut lib = Library::open_in_memory().unwrap();
    lib.import(&snap, &device(), false).unwrap();
    let id = annotation_id(&lib, &bookmark);
    let a = lib.annotation(id).unwrap().unwrap();
    assert!(a.starred);
    assert_eq!(a.status, Status::Kept);
    assert_eq!(a.tags, ["leadership"]);
    // The Kobo's note is kept as typed.
    assert_eq!(a.note(), Some("#Leadership\nNB"));

    // Taken away in Kollate, they stay away when imported again.
    lib.set_starred(id, false).unwrap();
    lib.set_annotation_tags(id, &[]).unwrap();
    lib.import(&snap, &device(), false).unwrap();
    let a = lib.annotation(id).unwrap().unwrap();
    assert!(!a.starred);
    assert!(a.tags.is_empty());

    // A new mark written later still applies, and a close misreading joins
    // the existing tag.
    lib.set_annotation_tags(id, &["leadership"]).unwrap();
    let j = snap
        .bookmarks
        .iter()
        .rposition(|b| b.kind == AnnotationKind::Highlight)
        .unwrap();
    snap.bookmarks[j].note = Some("?\n#leadershp #lead".into());
    let other = snap.bookmarks[j].bookmark_id.clone();
    lib.import(&snap, &device(), false).unwrap();
    let b = lib
        .annotation(annotation_id(&lib, &other))
        .unwrap()
        .unwrap();
    // "lead" is too far from "leadership" to be a typo of it.
    assert_eq!(b.tags, ["lead", "leadership", "question"]);
}

#[test]
fn handwritten_marks_and_glosses() {
    let dir = tempfile::tempdir().unwrap();
    let snap = fixture();
    let mut lib = Library::open_in_memory().unwrap();
    lib.import(&snap, &device(), false).unwrap();
    let markup = snap
        .bookmarks
        .iter()
        .find(|b| b.kind == AnnotationKind::Markup)
        .unwrap();
    let svg = dir.path().join("m.svg");
    std::fs::write(
        &svg,
        "<svg width=\"10\" height=\"10\"><g><path d=\"M1,1 L2,2\"/></g></svg>",
    )
    .unwrap();
    lib.attach_assets(
        &device(),
        &CopiedAssets {
            covers: vec![],
            markups: vec![CopiedMarkup {
                bookmark_id: markup.bookmark_id.clone(),
                svg: Some(svg),
                jpg: None,
                crop: None,
            }],
        },
    )
    .unwrap();
    let words: Vec<String> =
        "The walls were rebuilt. Like Nehemiah, let us build together. The end."
            .split(' ')
            .map(str::to_owned)
            .collect();
    lib.set_markup_contexts(&device(), &[(markup.bookmark_id.clone(), words)])
        .unwrap();
    let id = annotation_id(&lib, &markup.bookmark_id);
    let job = lib
        .pending_transcriptions("test")
        .unwrap()
        .into_iter()
        .find(|j| j.annotation_id == id)
        .unwrap();
    let t = Transcription {
        text: Some("Nehemiah,".into()),
        note: Some("*\nimportant figure #people".into()),
        circled: vec!["Nehemiah".into()],
    };
    assert_eq!(lib.save_transcription(&job, &t, "test").unwrap(), 1);
    let a = lib.annotation(id).unwrap().unwrap();
    assert!(a.starred);
    assert_eq!(a.tags, ["people"]);
    assert_eq!(a.note(), Some("important figure"));

    let word = lib
        .vocab()
        .unwrap()
        .into_iter()
        .find(|v| v.word == "Nehemiah")
        .unwrap();
    assert_eq!(word.gloss.as_deref(), Some("important figure"));
    let detail = lib.vocab_detail(word.id).unwrap().unwrap();
    let circled: Vec<_> = detail.sightings.iter().filter(|s| s.circled).collect();
    assert_eq!(circled.len(), 1);
    assert_eq!(
        circled[0].context.as_deref(),
        Some("Like Nehemiah, let us build together.")
    );

    // Read again: nothing added twice.
    assert_eq!(lib.save_transcription(&job, &t, "test").unwrap(), 0);

    // Turned off, circled words stay out of Vocabulary.
    lib.set_setting(GLOSSES_SETTING, "0").unwrap();
    let t2 = Transcription {
        circled: vec!["walls".into()],
        ..t
    };
    assert_eq!(lib.save_transcription(&job, &t2, "test").unwrap(), 0);
}
