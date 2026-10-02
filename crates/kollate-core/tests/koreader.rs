//! KOReader's highlights, notes and words on a Kobo (SPEC §8e), from a fake
//! mount laid out like the Libra Colour sample.

use std::path::Path;

use kollate_core::Library;
use kollate_core::kobo::{DeviceInfo, KoboSnapshot};
use kollate_core::koreader::add_koreader;

const BOOK: &str = "Anderson, Poul/Broken Sword, The - Poul Anderson.kepub.epub";

/// Book settings as KOReader v2026.07 writes them (trimmed).
fn sidecar(annotations: &str) -> String {
    format!(
        r#"-- /mnt/onboard/{BOOK}.sdr/metadata.epub.lua
return {{
    ["annotations"] = {{
{annotations}    }},
    ["doc_path"] = "/mnt/onboard/{BOOK}",
    ["doc_props"] = {{
        ["authors"] = "Poul Anderson",
        ["identifiers"] = "uuid:4f6c50d9\
calibre:8cac1389\
ISBN:9781497695863",
        ["language"] = "en",
        ["title"] = "The Broken Sword",
    }},
    ["partial_md5_checksum"] = "4e1734f1c6c248aea3f2c068d276b1e5",
    ["percent_finished"] = 0.050656660412758,
}}
"#
    )
}

const BLUE: &str = r#"        [1] = {
            ["chapter"] = "Chapter 1",
            ["color"] = "blue",
            ["datetime"] = "2026-10-02 14:41:43",
            ["drawer"] = "lighten",
            ["pos0"] = "/body/DocFragment[9]/body/div/div/p[18]/span[3]/text().0",
            ["pos1"] = "/body/DocFragment[9]/body/div/div/p[18]/span[3]/text().29",
            ["text"] = "When he had sat there a year,",
        },
"#;
const GREEN_NOTE: &str = r#"        [2] = {
            ["chapter"] = "Chapter 1",
            ["color"] = "green",
            ["datetime"] = "2026-10-02 14:40:40",
            ["datetime_updated"] = "2026-10-02 14:41:06",
            ["drawer"] = "lighten",
            ["note"] = "woah\
*\
#paganism",
            ["pos0"] = "/body/DocFragment[9]/body/div/div/p[20]/span[4]/text().58",
            ["pos1"] = "/body/DocFragment[9]/body/div/div/p[20]/span[4]/text().70",
            ["text"] = "White Christ",
        },
        [3] = {
            ["datetime"] = "2026-10-02 14:50:00",
            ["page"] = "/body/DocFragment[9]/body/div/div/p[30]/text().0",
        },
"#;

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A Kobo with KOReader, one book with `annotations`, and two words.
fn mount(annotations: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let m = dir.path();
    write(&m.join(".kobo/KoboReader.sqlite"), "");
    write(&m.join(BOOK), "");
    let sdr = m.join(BOOK.replace(".epub", ".sdr"));
    write(&sdr.join("metadata.epub.lua"), sidecar(annotations));
    write(&sdr.join("metadata.epub.lua.old"), "not even Lua");
    let ko = m.join(".adds/koreader");
    write(&ko.join("settings.reader.lua"), "return {}\n");
    std::fs::create_dir_all(ko.join("settings")).unwrap();
    let conn = rusqlite::Connection::open(ko.join("settings/vocabulary_builder.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TABLE vocabulary (word TEXT NOT NULL UNIQUE, title_id INTEGER, create_time INTEGER NOT NULL,
             review_time INTEGER, due_time INTEGER NOT NULL, review_count INTEGER NOT NULL DEFAULT 0,
             prev_context TEXT, next_context TEXT, streak_count INTEGER NOT NULL DEFAULT 0, highlight TEXT);
         CREATE TABLE title (id INTEGER NOT NULL UNIQUE, name TEXT UNIQUE, filter INTEGER NOT NULL DEFAULT 1);
         INSERT INTO title VALUES (1, 'The Broken Sword', 1), (2, 'A Book Not Here', 1);
         INSERT INTO vocabulary (word, title_id, create_time, due_time, prev_context, next_context) VALUES
           ('ealdorman', 1, 1790966801, 0, 'were well if he had a wife. He rode with a great following  to the English ',
            ' Athelstane and asked for his daughter Aelfrida, who was said to be the fairest maiden in'),
           ('hauberk', 2, 1790966900, 0, NULL, NULL);",
    )
    .unwrap();
    dir
}

fn device() -> DeviceInfo {
    DeviceInfo {
        serial: "N428".into(),
        firmware: None,
        model_id: None,
    }
}

fn read(mount: &Path) -> KoboSnapshot {
    let mut snap = KoboSnapshot::default();
    add_koreader(mount, &mut snap);
    snap
}

#[test]
fn reads_highlights_notes_and_words() {
    let m = mount(&format!("{BLUE}{GREEN_NOTE}"));
    let snap = read(m.path());
    assert!(snap.koreader_read);
    assert!(
        snap.koreader_unread.is_empty(),
        "{:?}",
        snap.koreader_unread
    );

    assert_eq!(snap.books.len(), 1);
    let book = &snap.books[0];
    assert_eq!(book.volume_id, format!("file:///mnt/onboard/{BOOK}"));
    assert_eq!(
        (book.title.as_str(), book.author.as_deref()),
        ("The Broken Sword", Some("Poul Anderson"))
    );
    assert_eq!(book.isbn.as_deref(), Some("9781497695863"));
    assert_eq!(book.percent_read, Some(5));

    // The bookmark (no text) is left out, as dog-ears are.
    assert_eq!(snap.bookmarks.len(), 2);
    let note = snap.bookmarks.iter().find(|b| b.color == "green").unwrap();
    assert_eq!(note.note.as_deref(), Some("woah\n*\n#paganism"));
    assert_eq!(note.chapter_title.as_deref(), Some("Chapter 1"));
    assert_eq!(note.spine_index, Some(8));
    assert_eq!(note.start.offset, 58);
    assert!(note.bookmark_id.starts_with("koreader:"));

    let ealdorman = snap.words.iter().find(|w| w.word == "ealdorman").unwrap();
    assert_eq!(
        ealdorman.volume_id.as_deref(),
        Some(book.volume_id.as_str())
    );
    assert_eq!(ealdorman.language, None, "English, as Kobo files it");
    assert_eq!(
        ealdorman.context.as_deref(),
        Some(
            "He rode with a great following to the English ealdorman Athelstane and asked for his daughter Aelfrida, who was said to be the fairest maiden in"
        )
    );
    let hauberk = snap.words.iter().find(|w| w.word == "hauberk").unwrap();
    assert_eq!(hauberk.volume_id, None, "its book isn't on the device");
}

#[test]
fn imports_with_colours_and_pen_marks_and_keeps_track() {
    let m = mount(&format!("{BLUE}{GREEN_NOTE}"));
    let mut lib = Library::open_in_memory().unwrap();
    let stats = lib.import(&read(m.path()), &device(), false).unwrap();
    assert_eq!(
        (stats.books_new, stats.annotations_new, stats.words_new),
        (1, 2, 2)
    );

    let book = lib.books().unwrap().into_iter().next().unwrap();
    let notes = lib.annotations_for_book(book.id).unwrap();
    // In reading order: p[18] before p[20].
    let colours: Vec<_> = notes.iter().map(|a| a.color.as_str()).collect();
    assert_eq!(colours, ["blue", "green"]);
    // The marks typed in the note: a star and a tag.
    assert!(notes[1].starred);
    assert_eq!(notes[1].tags, ["paganism"]);

    let ealdorman = lib
        .vocab()
        .unwrap()
        .into_iter()
        .find(|v| v.word == "ealdorman")
        .unwrap();
    assert!(
        ealdorman
            .context
            .as_deref()
            .unwrap()
            .starts_with("He rode with")
    );

    // Again: nothing new.
    let again = lib.import(&read(m.path()), &device(), false).unwrap();
    assert_eq!(
        (
            again.annotations_new,
            again.annotations_unchanged,
            again.annotations_removed
        ),
        (0, 2, 0)
    );

    // KOReader wasn't read (a database-only import): nothing is "deleted".
    let nickel_only = lib
        .import(&KoboSnapshot::default(), &device(), false)
        .unwrap();
    assert_eq!(nickel_only.annotations_removed, 0);

    // A highlight deleted in KOReader.
    let m2 = mount(GREEN_NOTE);
    let gone = lib.import(&read(m2.path()), &device(), false).unwrap();
    assert_eq!(gone.annotations_removed, 1);
}

#[test]
fn an_unreadable_sidecar_deletes_nothing() {
    let m = mount(BLUE);
    let mut lib = Library::open_in_memory().unwrap();
    lib.import(&read(m.path()), &device(), false).unwrap();
    let broken = m
        .path()
        .join(BOOK.replace(".epub", ".sdr"))
        .join("metadata.epub.lua");
    std::fs::write(&broken, "return { [\"annotations\"] = ").unwrap();
    let snap = read(m.path());
    assert_eq!(snap.koreader_unread.len(), 1);
    let stats = lib.import(&snap, &device(), false).unwrap();
    assert_eq!(stats.annotations_removed, 0);
}

#[test]
fn without_koreader_nothing_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let snap = read(dir.path());
    assert!(!snap.koreader_read);
    assert!(snap.books.is_empty() && snap.words.is_empty());
}

#[test]
fn finds_books_in_any_folder_through_the_history() {
    let m = mount(BLUE);
    // A second book far deeper than the walk goes.
    let deep = "a/b/c/d/e/f/g/h/i/j/Deep - Author.epub";
    write(&m.path().join(deep), "");
    let sdr = m.path().join(deep.replace(".epub", ".sdr"));
    let settings = sidecar(BLUE)
        .replace(BOOK, deep)
        .replace("The Broken Sword", "Deep")
        .replace("4e1734f1", "0000aaaa");
    write(&sdr.join("metadata.epub.lua"), settings);
    write(
        &m.path().join(".adds/koreader/history.lua"),
        format!(
            "return {{\n    [1] = {{\n        [\"file\"] = \"/mnt/onboard/{deep}\",\n        [\"time\"] = 1790966983,\n    }},\n}}\n"
        ),
    );
    let snap = read(m.path());
    let titles: Vec<_> = snap.books.iter().map(|b| b.title.as_str()).collect();
    assert!(titles.contains(&"Deep"), "{titles:?}");
    assert_eq!(snap.bookmarks.len(), 2);
}

#[test]
fn a_book_only_opened_isnt_added() {
    // Settings for a book with no annotations and no words.
    let m = mount("");
    let conn = rusqlite::Connection::open(
        m.path()
            .join(".adds/koreader/settings/vocabulary_builder.sqlite3"),
    )
    .unwrap();
    conn.execute("DELETE FROM vocabulary", []).unwrap();
    drop(conn);
    let snap = read(m.path());
    assert!(snap.books.is_empty(), "{:?}", snap.books);
}
