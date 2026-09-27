//! Handwriting transcription state in the library (SPEC §8a), with a scripted
//! reader standing in for the model.

use std::path::PathBuf;

use kollate_core::Library;
use kollate_core::kobo::assets::{CopiedAssets, CopiedMarkup};
use kollate_core::kobo::{AnnotationKind, DeviceInfo, KoboDb};
use kollate_core::markup::{Reader, RgbImage};
use kollate_core::store::{AnnotationFilter, View};

struct Scripted(Vec<&'static str>);

impl Reader for Scripted {
    fn handwriting(&mut self, _: &RgbImage) -> kollate_core::Result<String> {
        Ok(self.0.remove(0).to_owned())
    }
    fn print(&mut self, _: &RgbImage) -> kollate_core::Result<String> {
        unreachable!("no page image in this test")
    }
}

/// Two handwritten words, far apart: two notes.
const INK: &str = "<svg width=\"1264\" height=\"1680\" viewBox=\"0 0 1264 1680\"><g>\
    <path d=\"M100,100 L130,100 L130,140 L100,140\"/>\
    <path d=\"M134,100 L164,100 L164,140 L134,140\"/>\
    <path d=\"M600,900 L630,900 L630,940 L600,940\"/></g></svg>";

#[test]
fn transcribes_markups_and_keeps_the_users_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/KoboReader.sqlite");
    let snap = KoboDb::open_copy(&path).unwrap().snapshot().unwrap();
    let device = DeviceInfo {
        serial: "A".into(),
        firmware: None,
        model_id: None,
    };
    let mut lib = Library::open_in_memory().unwrap();
    lib.import(&snap, &device, false).unwrap();

    let markup = snap
        .bookmarks
        .iter()
        .find(|b| b.kind == AnnotationKind::Markup)
        .unwrap();
    let svg = dir.path().join("m.svg");
    std::fs::write(&svg, INK).unwrap();
    lib.attach_assets(
        &device,
        &CopiedAssets {
            covers: vec![],
            markups: vec![CopiedMarkup {
                bookmark_id: markup.bookmark_id.clone(),
                svg: Some(svg.clone()),
                jpg: None,
                crop: None,
            }],
        },
    )
    .unwrap();

    let jobs = lib.pending_transcriptions("model-a").unwrap();
    assert_eq!(jobs.len(), 1);
    let t = jobs[0]
        .run(&mut Scripted(vec!["Woah, this\nworks!", "Ravens"]))
        .unwrap();
    assert_eq!(t.note.as_deref(), Some("Woah, this works!\nRavens"));
    lib.save_transcription(&jobs[0], &t, "model-a").unwrap();
    assert!(lib.pending_transcriptions("model-a").unwrap().is_empty());

    let id = jobs[0].annotation_id;
    let a = lib.annotation(id).unwrap().unwrap();
    assert_eq!(a.note(), Some("Woah, this works!\nRavens"));
    assert_eq!(a.ink_source.as_deref(), Some("model-a"));
    // A handwritten note is a note: it's in the Notes view and in search.
    let find = |view: View, search: Option<&str>| {
        lib.query_annotations(&AnnotationFilter {
            view,
            search: search.map(Into::into),
            id: Some(id),
        })
        .unwrap()
        .len()
    };
    assert_eq!(find(View::Notes, None), 1);
    assert_eq!(find(View::All, Some("ravens")), 1);

    // The user's correction wins, and survives a new transcription.
    lib.set_user_note(id, Some("Ravens, not havens")).unwrap();
    let jobs = lib.pending_transcriptions("model-b").unwrap();
    assert_eq!(jobs.len(), 1, "a different model reads it again");
    let t = jobs[0].run(&mut Scripted(vec!["Whoa", "Havens"])).unwrap();
    lib.save_transcription(&jobs[0], &t, "model-b").unwrap();
    let a = lib.annotation(id).unwrap().unwrap();
    assert_eq!(a.note(), Some("Ravens, not havens"));
    assert_eq!(a.original_note(), Some("Whoa\nHavens"));

    // New ink on the page means reading it again.
    std::fs::write(&svg, INK.replace("M600,900", "M610,900")).unwrap();
    assert_eq!(lib.pending_transcriptions("model-b").unwrap().len(), 1);

    // Book words saved at import are part of what it was made from.
    let jobs = lib.pending_transcriptions("model-b").unwrap();
    lib.save_transcription(&jobs[0], &t, "model-b").unwrap();
    let words = vec![("x".to_owned(), vec![])];
    assert_eq!(lib.set_markup_contexts(&device, &words).unwrap(), 0);
    let words = vec![(
        markup.bookmark_id.clone(),
        vec!["pale".to_owned(), "blood".to_owned()],
    )];
    assert_eq!(lib.set_markup_contexts(&device, &words).unwrap(), 1);
    let jobs = lib.pending_transcriptions("model-b").unwrap();
    assert_eq!(
        jobs[0].words.as_deref(),
        Some(&["pale".to_owned(), "blood".to_owned()][..])
    );
}
