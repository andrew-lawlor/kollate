//! Vocab enrichment: definitions, merging inflected forms, contexts.

use std::path::PathBuf;

use kollate_core::Library;
use kollate_core::dict::{Dictionary, build_from_wordnet_lmf};
use kollate_core::kobo::epub::{Context, WordContexts};
use kollate_core::kobo::{DeviceInfo, KoboDb, KoboSnapshot};
use kollate_core::store::VocabStatus;

const LMF: &str = r#"<?xml version="1.0"?><LexicalResource><Lexicon id="oewn" language="en" version="test">
  <LexicalEntry id="e1"><Lemma writtenForm="theophany" partOfSpeech="n"/><Sense id="s1" synset="y1"/></LexicalEntry>
  <LexicalEntry id="e2"><Lemma writtenForm="demiurge" partOfSpeech="n"/><Sense id="s2" synset="y2"/></LexicalEntry>
  <Synset id="y1"><Definition>a visible manifestation of a deity</Definition></Synset>
  <Synset id="y2"><Definition>a subordinate deity</Definition></Synset>
</Lexicon></LexicalResource>"#;

fn setup() -> (
    Library,
    KoboSnapshot,
    DeviceInfo,
    Dictionary,
    tempfile::TempDir,
) {
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
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wn.xml"), LMF).unwrap();
    build_from_wordnet_lmf(&dir.path().join("wn.xml"), &dir.path().join("wn.db")).unwrap();
    let dict = Dictionary::open(&dir.path().join("wn.db")).unwrap();
    (lib, snap, device, dict, dir)
}

fn find(lib: &Library, word: &str) -> kollate_core::store::Vocab {
    lib.vocab()
        .unwrap()
        .into_iter()
        .find(|v| v.word == word)
        .unwrap()
}

#[test]
fn defines_words_and_merges_forms() {
    let (mut lib, snap, device, dict, _dir) = setup();
    lib.set_vocab_status(find(&lib, "theophanies").id, VocabStatus::Learning)
        .unwrap();

    assert_eq!(
        lib.enrich_definitions(std::slice::from_ref(&dict)).unwrap(),
        3
    );
    let words = lib.vocab().unwrap();
    assert_eq!(words.len(), 11, "theophanies merged into theophany");
    let theophany = find(&lib, "theophany");
    assert_eq!(theophany.lemma.as_deref(), Some("theophany"));
    assert_eq!(
        theophany.definition.as_deref(),
        Some("noun: a visible manifestation of a deity")
    );
    assert_eq!(
        theophany.status,
        VocabStatus::Learning,
        "most advanced status wins"
    );
    let detail = lib.vocab_detail(theophany.id).unwrap().unwrap();
    let forms: Vec<_> = detail
        .sightings
        .iter()
        .map(|s| s.surface_form.as_str())
        .collect();
    assert_eq!(forms, ["theophany", "theophanies"]);
    assert!(find(&lib, "Demiurge").definition.is_some());
    assert!(find(&lib, "hierophant").definition.is_none());

    // Re-importing doesn't bring the merged form back as its own word.
    let stats = lib.import(&snap, &device, false).unwrap();
    assert_eq!((stats.words_new, stats.word_sightings_new), (0, 0));
    assert_eq!(lib.vocab().unwrap().len(), 11);

    // User edits are kept; clearing makes it eligible for lookup again.
    let id = find(&lib, "Demiurge").id;
    lib.set_vocab_definition(id, Some("my own words")).unwrap();
    lib.enrich_definitions(std::slice::from_ref(&dict)).unwrap();
    assert_eq!(
        find(&lib, "Demiurge").definition.as_deref(),
        Some("my own words")
    );
    lib.set_vocab_definition(id, None).unwrap();
    lib.enrich_definitions(std::slice::from_ref(&dict)).unwrap();
    assert_eq!(
        find(&lib, "Demiurge").definition.as_deref(),
        Some("noun: a subordinate deity")
    );
}

#[test]
fn stores_contexts_without_overriding_choice() {
    let (lib, snap, device, _dict, _dir) = setup();
    let volume_id = snap
        .words
        .iter()
        .find(|w| w.word == "monism")
        .unwrap()
        .volume_id
        .clone()
        .unwrap();
    let found = |sentences: &[&str]| {
        vec![WordContexts {
            volume_id: volume_id.clone(),
            word: "monism".into(),
            contexts: sentences
                .iter()
                .map(|s| Context {
                    sentence: s.to_string(),
                    chapter: 9,
                })
                .collect(),
        }]
    };
    assert_eq!(
        lib.set_word_contexts(&device, &found(&["First monism.", "Second monism."]))
            .unwrap(),
        1
    );
    let monism = find(&lib, "monism");
    assert_eq!(monism.context.as_deref(), Some("First monism."));

    let detail = lib.vocab_detail(monism.id).unwrap().unwrap();
    let sighting = &detail.sightings[0];
    assert_eq!(sighting.candidates, ["First monism.", "Second monism."]);
    lib.set_sighting_context(sighting.id, Some("Second monism."))
        .unwrap();

    lib.set_word_contexts(&device, &found(&["Another monism."]))
        .unwrap();
    let detail = lib.vocab_detail(monism.id).unwrap().unwrap();
    assert_eq!(
        detail.sightings[0].context.as_deref(),
        Some("Second monism."),
        "user choice kept"
    );
    assert_eq!(detail.sightings[0].candidates, ["Another monism."]);
    assert_eq!(
        lib.query_vocab(None, Some("another")).unwrap().len(),
        0,
        "search uses the chosen context"
    );
    assert_eq!(
        lib.query_vocab(None, Some("second monism")).unwrap().len(),
        1
    );
}

#[test]
fn refresh_replaces_dictionary_definitions_but_not_edits() {
    let (mut lib, _snap, _device, dict, dir) = setup();
    lib.enrich_definitions(std::slice::from_ref(&dict)).unwrap();
    let demiurge = find(&lib, "Demiurge").id;
    let theophany = find(&lib, "theophany").id;
    lib.set_vocab_definition(demiurge, Some("my own words"))
        .unwrap();

    // A better dictionary arrives.
    let df = dir.path().join("wikt.df");
    std::fs::write(
        &df,
        "@ theophany\n& theophanies\n<html><p><b>Noun</b></p><ol><li>A manifestation of a deity to a person.</li></ol>\n\n\
         @ demiurge\n<html><p><b>Noun</b></p><ol><li>(Platonic philosophy) The creator of the universe.</li></ol>\n\n\
         @ hierophant\n<html><p><b>Noun</b></p><ol><li>An interpreter of sacred mysteries.</li></ol>\n",
    )
    .unwrap();
    kollate_core::dict::build_from_dictfile(
        &df,
        &dir.path().join("wikt.db"),
        "Wiktionary",
        Some("en"),
        "test",
    )
    .unwrap();
    let wikt = Dictionary::open(&dir.path().join("wikt.db")).unwrap();

    assert_eq!(
        lib.refresh_definitions(std::slice::from_ref(&wikt))
            .unwrap(),
        1,
        "only theophany came from a dictionary"
    );
    let t = lib.vocab_detail(theophany).unwrap().unwrap().vocab;
    assert_eq!(
        t.definition.as_deref(),
        Some("noun: A manifestation of a deity to a person.")
    );
    assert_eq!(t.definition_source.as_deref(), Some("Wiktionary"));
    assert_eq!(
        find(&lib, "Demiurge").definition.as_deref(),
        Some("my own words"),
        "edits are never replaced"
    );

    // Words that had no definition get one from the new dictionary.
    assert_eq!(
        lib.enrich_definitions(std::slice::from_ref(&wikt)).unwrap(),
        1
    );
    assert!(find(&lib, "hierophant").definition.is_some());
}
