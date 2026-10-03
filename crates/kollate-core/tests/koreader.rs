//! KOReader's highlights, notes and words on a Kobo (SPEC §8e), from a fake
//! mount laid out like the Libra Colour sample.

use std::path::Path;

use kollate_core::Library;
use kollate_core::kobo::assets::copy_assets;
use kollate_core::kobo::{AnnotationKind, DeviceInfo, KoboSnapshot};
use kollate_core::koreader::add_koreader;
use kollate_core::markup::{Reader, RgbImage};

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

/// Stands in for the model: reads every note as `note`, and fails if asked
/// to read print (a Pencil markup's marks come from its page's words).
struct Notes(&'static str);

impl Reader for Notes {
    fn handwriting(&mut self, _: &RgbImage) -> kollate_core::Result<String> {
        Ok(self.0.to_owned())
    }
    fn print(&mut self, _: &RgbImage) -> kollate_core::Result<String> {
        unreachable!("marks are found from the page's words")
    }
}

/// A Pencil (fork) markup on the book: a line underlined under "the
/// glorious city" and a two-letter note in the margin, as the plugin writes
/// it, plus a folder still being written.
fn add_pencil_markup(mount: &Path) {
    let dir = mount
        .join(BOOK.replace(".epub", ".sdr"))
        .join("pencil/markups");
    let m = dir.join("m_20261003045726_288_351");
    write(
        &m.join("markup.json"),
        r#"{"format": 1, "id": "m_20261003045726_288_351", "created": 1791003446,
            "modified": 1791003500, "start": "/body/DocFragment[6]/body/div/p[1]/text().0",
            "end": "/body/DocFragment[6]/body/div/p[3]/text().20", "chapter": "Preface",
            "screen": {"width": 600, "height": 400, "rotation": 0}, "has_page_image": true,
            "plugin_version": "0.6.3"}"#,
    );
    write(
        &m.join("ink.json"),
        r#"{"format": 1, "strokes": [
            {"points": [[95, 146], [250, 147], [395, 146]], "width": 3, "color": "Black", "tool": "pen"},
            {"points": [[480, 90], [490, 120], [500, 90]], "width": 3, "color": "Black", "tool": "pen"},
            {"points": [[510, 90], [510, 120], [530, 120]], "width": 3, "color": "Black", "tool": "pen"}
        ]}"#,
    );
    write(
        &m.join("words.json"),
        r#"{"format": 1, "words": [
            {"text": "The", "boxes": [[100, 100, 150, 140]], "pos0": "a", "pos1": "b"},
            {"text": "glorious", "boxes": [[160, 100, 290, 140]], "pos0": "c", "pos1": "d"},
            {"text": "city", "boxes": [[300, 100, 380, 140]], "pos0": "e", "pos1": "f"},
            {"text": "of", "boxes": [[100, 160, 130, 200]], "pos0": "g", "pos1": "h"}
        ]}"#,
    );
    let mut png = std::io::Cursor::new(Vec::new());
    image::GrayImage::from_pixel(600, 400, image::Luma([255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    write(&m.join("page.png"), png.into_inner());
    // Being written: no markup.json yet.
    write(&dir.join("m_20261003050000_1_1/ink.json"), "{");
}

#[test]
fn imports_pencil_markups_and_reads_marks_from_the_page_words() {
    let m = mount(BLUE);
    add_pencil_markup(m.path());
    let snap = read(m.path());
    assert!(
        snap.koreader_unread.is_empty(),
        "{:?}",
        snap.koreader_unread
    );
    let markup = snap
        .bookmarks
        .iter()
        .find(|b| b.kind == AnnotationKind::Markup)
        .expect("the markup");
    assert_eq!(markup.bookmark_id, "koreader:ink:m_20261003045726_288_351");
    assert_eq!(markup.chapter_title.as_deref(), Some("Preface"));
    assert_eq!(markup.spine_index, Some(5));
    assert_eq!(
        snap.bookmarks
            .iter()
            .filter(|b| b.kind == AnnotationKind::Markup)
            .count(),
        1,
        "the folder being written is skipped"
    );

    let device = device();
    let mut lib = Library::open_in_memory().unwrap();
    lib.import(&snap, &device, false).unwrap();
    let assets = tempfile::tempdir().unwrap();
    let copied = copy_assets(m.path(), &snap, assets.path()).unwrap();
    let ink = copied
        .markups
        .iter()
        .find(|c| c.bookmark_id == markup.bookmark_id)
        .unwrap();
    let jpg = std::fs::read(ink.jpg.as_ref().unwrap()).unwrap();
    assert_eq!(&jpg[..2], [0xff, 0xd8], "the page, as a JPEG");
    assert!(
        std::fs::read_to_string(ink.svg.as_ref().unwrap())
            .unwrap()
            .contains("stroke-width=\"3\"")
    );
    lib.attach_assets(&device, &copied).unwrap();

    let jobs = lib.pending_transcriptions("model").unwrap();
    assert_eq!(jobs.len(), 1);
    assert!(jobs[0].page_words.is_some());
    let t = jobs[0].run(&mut Notes("ok"), None).unwrap();
    assert_eq!(t.text.as_deref(), Some("The glorious city"));
    assert_eq!(t.note.as_deref(), Some("ok"));
}
