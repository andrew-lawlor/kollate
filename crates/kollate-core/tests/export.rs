//! Exports: Obsidian sync, Anki package, CSV, Readwise and JSON.

use std::io::Read;
use std::path::PathBuf;

use kollate_core::Library;
use kollate_core::export::{self, ExportOptions};
use kollate_core::kobo::epub::{Context, WordContexts};
use kollate_core::kobo::{DeviceInfo, KoboDb};
use kollate_core::store::{AnnotationFilter, Status, View};

fn library() -> Library {
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
    let volume_id = snap
        .words
        .iter()
        .find(|w| w.word == "monism")
        .unwrap()
        .volume_id
        .clone()
        .unwrap();
    lib.set_word_contexts(
        &device,
        &[WordContexts {
            volume_id,
            word: "monism".into(),
            contexts: vec![Context {
                sentence: "Despite his monism, he was a dualist.".into(),
                chapter: 9,
            }],
        }],
    )
    .unwrap();
    lib
}

#[test]
fn obsidian_sync_is_idempotent_and_keeps_user_text() {
    let lib = library();
    let vault = tempfile::tempdir().unwrap();
    let dir = vault.path().join("Books");
    let first = export::sync_obsidian(&lib, &dir, ExportOptions::default()).unwrap();
    assert_eq!(first.notes_written, 6, "5 books + Vocabulary");

    let note_path = dir.join("Children of Ash and Elm.md");
    let note = std::fs::read_to_string(&note_path).unwrap();
    assert!(note.starts_with("---\ntitle: \"Children of Ash and Elm\"\nauthor: \"Neil Price\"\n"));
    assert!(note.contains("kollate_id: "));
    assert!(note.contains("## 9. Inroads\n\n> elements ^k"));
    assert!(note.contains("%% kollate:user"));
    let tantra = std::fs::read_to_string(
        dir.join("Hellenic Tantra The Theurgic Platonism of Iamblichus.md"),
    )
    .unwrap();
    assert!(tantra.contains("## Vocabulary\n\n"));
    assert!(tantra.contains("  > Despite his **monism**, he was a dualist."));
    let vocab = std::fs::read_to_string(dir.join("Vocabulary.md")).unwrap();
    assert!(vocab.contains("From [[Hellenic Tantra The Theurgic Platonism of Iamblichus]]"));

    let again = export::sync_obsidian(&lib, &dir, ExportOptions::default()).unwrap();
    assert_eq!((again.notes_written, again.notes_unchanged), (0, 6));

    // The user writes below the marker, then a highlight is archived.
    std::fs::write(&note_path, format!("{note}\nMy thoughts on Odin.\n")).unwrap();
    let item = lib
        .query_annotations(&AnnotationFilter {
            view: View::All,
            search: Some("elements".into()),
            id: None,
        })
        .unwrap()[0]
        .id;
    lib.set_status(item, Status::Archived).unwrap();
    let third = export::sync_obsidian(&lib, &dir, ExportOptions::default()).unwrap();
    assert_eq!(third.notes_written, 1);
    let updated = std::fs::read_to_string(&note_path).unwrap();
    assert!(
        !updated.contains("> elements"),
        "archived highlight left out"
    );
    assert!(
        updated.ends_with("My thoughts on Odin.\n"),
        "user text kept"
    );

    let with_archived = ExportOptions {
        include_archived: true,
        ..Default::default()
    };
    export::sync_obsidian(&lib, &dir, with_archived).unwrap();
    assert!(
        std::fs::read_to_string(&note_path)
            .unwrap()
            .contains("> elements")
    );
}

fn anki_counts(path: &std::path::Path) -> (i64, i64, Vec<String>) {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut media = String::new();
    zip.by_name("media")
        .unwrap()
        .read_to_string(&mut media)
        .unwrap();
    assert_eq!(media, "{}");
    let mut bytes = Vec::new();
    zip.by_name("collection.anki2")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("c.anki2");
    std::fs::write(&db, bytes).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let notes: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
        .unwrap();
    let cards: i64 = conn
        .query_row("SELECT count(*) FROM cards", [], |r| r.get(0))
        .unwrap();
    let guids = conn
        .prepare("SELECT guid FROM notes ORDER BY guid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<String>, _>>()
        .unwrap();
    let models: String = conn
        .query_row("SELECT models FROM col", [], |r| r.get(0))
        .unwrap();
    assert!(models.contains("Kollate Vocab"));
    (notes, cards, guids)
}

#[test]
fn anki_deck_has_stable_notes() {
    let lib = library();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kollate.apkg");
    let stats = export::export_anki(&lib, &path, ExportOptions::default(), false).unwrap();
    // 12 words; only "monism" has a context, so it alone gets a fill-in card.
    assert_eq!((stats.words, stats.highlights, stats.cards), (12, 0, 13));
    let (notes, cards, guids) = anki_counts(&path);
    assert_eq!((notes, cards), (12, 13));

    let stats = export::export_anki(&lib, &path, ExportOptions::default(), true).unwrap();
    assert_eq!(stats.highlights, 51, "all text highlights (not the markup)");
    let (_, _, guids_again) = anki_counts(&path);
    assert!(
        guids.iter().all(|g| guids_again.contains(g)),
        "vocab GUIDs unchanged between exports"
    );
}

#[test]
fn csv_readwise_and_json() {
    let lib = library();
    let dir = tempfile::tempdir().unwrap();
    let opts = ExportOptions::default();
    assert_eq!(
        export::export_highlights_csv(&lib, &dir.path().join("h.csv"), opts).unwrap(),
        52
    );
    assert_eq!(
        export::export_vocab_csv(&lib, &dir.path().join("v.csv"), opts).unwrap(),
        12
    );
    assert_eq!(
        export::export_readwise_csv(&lib, &dir.path().join("r.csv"), opts).unwrap(),
        51
    );
    let readwise = std::fs::read_to_string(dir.path().join("r.csv")).unwrap();
    assert!(readwise.starts_with("Highlight,Title,Author,URL,Note,Location,Date\n"));
    assert!(readwise.contains("Nice simile"));

    assert_eq!(
        export::export_json(&lib, &dir.path().join("b.json")).unwrap(),
        5
    );
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("b.json")).unwrap()).unwrap();
    assert_eq!(json["format"], "kollate-backup");
    let total: usize = json["books"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["annotations"].as_array().unwrap().len())
        .sum();
    assert_eq!(total, 52);
}

#[test]
fn nothing_can_be_written_to_a_kobo() {
    use kollate_core::Error;
    let lib = library();
    let dir = tempfile::tempdir().unwrap();
    let kobo = dir.path().join("KOBOeReader");
    std::fs::create_dir_all(kobo.join(".kobo")).unwrap();
    std::fs::write(kobo.join(".kobo/KoboReader.sqlite"), b"").unwrap();
    let before: Vec<_> = walk(&kobo);

    let on_kobo = |r: kollate_core::Result<_>| matches!(r, Err(Error::OnKobo(_)));
    let opts = ExportOptions::default();
    assert!(on_kobo(
        export::sync_obsidian(&lib, &kobo.join("Vault/Books"), opts).map(|_| ())
    ));
    assert!(on_kobo(
        export::export_anki(&lib, &kobo.join("Kollate.apkg"), opts, true).map(|_| ())
    ));
    assert!(on_kobo(
        export::export_json(&lib, &kobo.join("backup.json")).map(|_| ())
    ));
    assert!(on_kobo(
        export::export_highlights_csv(&lib, &kobo.join("h.csv"), opts).map(|_| ())
    ));
    assert!(on_kobo(
        export::export_vocab_csv(&lib, &kobo.join("v.csv"), opts).map(|_| ())
    ));
    assert!(on_kobo(
        export::export_readwise_csv(&lib, &kobo.join("r.csv"), opts).map(|_| ())
    ));
    assert!(on_kobo(
        Library::open(&kobo.join("kollate/library.db")).map(|_| ())
    ));
    assert!(on_kobo(
        kollate_core::dict::DictionaryBuilder::create(&kobo.join("d.db"), "x", None, "x")
            .map(|_| ())
    ));
    let err = export::export_json(&lib, &kobo.join("backup.json"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Kollate never writes to your Kobo"), "{err}");

    assert_eq!(
        walk(&kobo),
        before,
        "the fake Kobo is byte-for-byte untouched"
    );
}

fn walk(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push((p.clone(), std::fs::read(&p).unwrap()));
        }
    }
    out.sort();
    out
}
