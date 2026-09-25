//! Anki `.apkg` export (the legacy collection format every Anki version
//! imports). Deck, note type and note GUIDs are stable, so importing a newer
//! export updates existing cards and keeps their review history.

use std::io::Write;
use std::path::Path;

use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::json;

use super::{ExportBook, ExportOptions};
use crate::Result;
use crate::kobo::epub::word_span;
use crate::store::Library;

const VOCAB_DECK: i64 = 1_725_000_000_001;
const HIGHLIGHT_DECK: i64 = 1_725_000_000_002;
const VOCAB_MODEL: i64 = 1_725_000_000_101;
const HIGHLIGHT_MODEL: i64 = 1_725_000_000_102;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnkiStats {
    pub words: usize,
    pub highlights: usize,
    pub cards: usize,
}

const SCHEMA: &str = "
CREATE TABLE col (id integer primary key, crt integer not null, mod integer not null, scm integer not null,
    ver integer not null, dty integer not null, usn integer not null, ls integer not null, conf text not null,
    models text not null, decks text not null, dconf text not null, tags text not null);
CREATE TABLE notes (id integer primary key, guid text not null, mid integer not null, mod integer not null,
    usn integer not null, tags text not null, flds text not null, sfld integer not null, csum integer not null,
    flags integer not null, data text not null);
CREATE TABLE cards (id integer primary key, nid integer not null, did integer not null, ord integer not null,
    mod integer not null, usn integer not null, type integer not null, queue integer not null, due integer not null,
    ivl integer not null, factor integer not null, reps integer not null, lapses integer not null,
    left integer not null, odue integer not null, odid integer not null, flags integer not null, data text not null);
CREATE TABLE revlog (id integer primary key, cid integer not null, usn integer not null, ease integer not null,
    ivl integer not null, lastIvl integer not null, factor integer not null, time integer not null,
    type integer not null);
CREATE TABLE graves (usn integer not null, oid integer not null, type integer not null);
CREATE INDEX ix_notes_usn on notes (usn);
CREATE INDEX ix_cards_usn on cards (usn);
CREATE INDEX ix_revlog_usn on revlog (usn);
CREATE INDEX ix_cards_nid on cards (nid);
CREATE INDEX ix_cards_sched on cards (did, queue, due);
CREATE INDEX ix_revlog_cid on revlog (cid);
CREATE INDEX ix_notes_csum on notes (csum);
";

const CSS: &str = ".card { font-family: Georgia, serif; font-size: 20px; text-align: center; color: black; background-color: white; }
.word { font-size: 32px; font-weight: bold; }
.lemma { color: #777; }
.context { font-style: italic; margin-top: 1em; }
.definition { margin-top: 1em; text-align: left; display: inline-block; }
.quote { font-size: 22px; line-height: 1.4; text-align: left; }
.note { font-style: italic; margin-top: 1em; }
.source, .hint { color: #777; font-size: 15px; margin-top: 1em; }
.cloze { font-weight: bold; color: #1c71d8; }
.nightMode .cloze, .night_mode .cloze { color: #78aeed; }";

/// Anki's base91 alphabet (as used by genanki for GUIDs).
const BASE91: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&()*+,-./:;<=>?@[]^_`{|}~";

fn stable_u64(key: &str) -> u64 {
    u64::from_le_bytes(
        blake3::hash(key.as_bytes()).as_bytes()[..8]
            .try_into()
            .expect("8 bytes"),
    )
}

fn guid(key: &str) -> String {
    let mut n = stable_u64(key);
    let mut out = Vec::new();
    loop {
        out.push(BASE91[(n % 91) as usize]);
        n /= 91;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// Stable positive note ID derived from the GUID key.
fn note_id(key: &str) -> i64 {
    1_000_000_000_000 + (stable_u64(key) % 900_000_000_000) as i64
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\n', "<br>")
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Anki's duplicate-check checksum: first 8 hex digits of SHA-1 of the sort field.
fn checksum(sort_field: &str) -> i64 {
    let digest = sha1_smol::Sha1::from(strip_html(sort_field))
        .digest()
        .bytes();
    u32::from_be_bytes(digest[..4].try_into().expect("4 bytes")) as i64
}

fn anki_tag(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("_")
}

fn field(name: &str, ord: usize) -> serde_json::Value {
    json!({"name": name, "ord": ord, "font": "Arial", "media": [], "rtl": false, "size": 20, "sticky": false})
}

fn template(name: &str, ord: usize, qfmt: &str, afmt: &str) -> serde_json::Value {
    json!({"name": name, "ord": ord, "qfmt": qfmt, "afmt": afmt, "bqfmt": "", "bafmt": "", "did": null})
}

fn model(
    id: i64,
    name: &str,
    deck: i64,
    fields: &[&str],
    templates: Vec<serde_json::Value>,
    req: serde_json::Value,
    now: i64,
) -> serde_json::Value {
    json!({
        "id": id, "name": name, "type": 0, "mod": now, "usn": -1, "sortf": 0, "did": deck,
        "flds": fields.iter().enumerate().map(|(i, f)| field(f, i)).collect::<Vec<_>>(),
        "tmpls": templates, "req": req, "tags": [], "vers": [], "css": CSS,
        "latexPre": "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n",
        "latexPost": "\\end{document}",
    })
}

fn deck(id: i64, name: &str, now: i64) -> serde_json::Value {
    json!({
        "id": id, "name": name, "desc": "", "collapsed": false, "conf": 1, "dyn": 0, "extendNew": 10,
        "extendRev": 50, "mod": now, "usn": -1, "lrnToday": [0, 0], "newToday": [0, 0], "revToday": [0, 0],
        "timeToday": [0, 0],
    })
}

struct Writer {
    conn: Connection,
    now: i64,
    due: i64,
    cards: usize,
}

impl Writer {
    /// Adds a note and one card per template whose front isn't empty.
    fn note(
        &mut self,
        key: &str,
        model: i64,
        deck: i64,
        fields: &[String],
        tags: &[String],
        card_ords: &[usize],
    ) -> Result<()> {
        let id = note_id(key);
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!(" {} ", tags.join(" "))
        };
        self.conn.execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, -1, ?5, ?6, ?7, ?8, 0, '')",
            params![
                id,
                guid(key),
                model,
                self.now,
                tags,
                fields.join("\u{1f}"),
                strip_html(&fields[0]),
                checksum(&fields[0])
            ],
        )?;
        for &ord in card_ords {
            self.conn.execute(
                "INSERT INTO cards VALUES (?1, ?2, ?3, ?4, ?5, -1, 0, 0, ?6, 0, 0, 0, 0, 0, 0, 0, 0, '')",
                params![id * 10 + ord as i64, id, deck, ord as i64, self.now, self.due],
            )?;
            self.cards += 1;
        }
        self.due += 1;
        Ok(())
    }
}

/// Writes an Anki package with a vocabulary deck and, optionally, a deck of
/// highlights to review.
pub fn export_anki(
    lib: &Library,
    out: &Path,
    options: ExportOptions,
    include_highlights: bool,
) -> Result<AnkiStats> {
    let books: Vec<ExportBook> = lib.export_books(options)?;
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("collection.anki2");
    let now = Utc::now().timestamp();
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(SCHEMA)?;

    let vocab_model = model(
        VOCAB_MODEL,
        "Kollate Vocab",
        VOCAB_DECK,
        &[
            "Word",
            "Lemma",
            "Definition",
            "Context",
            "ContextBlank",
            "Book",
        ],
        vec![
            template(
                "Recognition",
                0,
                "<div class=word>{{Word}}</div>{{#Context}}<div class=context>{{Context}}</div>{{/Context}}",
                "{{FrontSide}}<hr id=answer>{{#Lemma}}<div class=lemma>{{Lemma}}</div>{{/Lemma}}<div class=definition>{{Definition}}</div><div class=source>{{Book}}</div>",
            ),
            template(
                "Fill In",
                1,
                "{{#ContextBlank}}<div class=context>{{ContextBlank}}</div>{{#Definition}}<div class=hint>{{Definition}}</div>{{/Definition}}{{/ContextBlank}}",
                "{{FrontSide}}<hr id=answer><div class=word>{{Word}}</div><div class=source>{{Book}}</div>",
            ),
        ],
        json!([[0, "any", [0, 3]], [1, "all", [4]]]),
        now,
    );
    let highlight_model = model(
        HIGHLIGHT_MODEL,
        "Kollate Highlight",
        HIGHLIGHT_DECK,
        &["Quote", "Note", "Book", "Author", "Chapter"],
        vec![template(
            "Review",
            0,
            "<div class=quote>{{Quote}}</div>",
            "{{FrontSide}}<hr id=answer>{{#Note}}<div class=note>{{Note}}</div>{{/Note}}<div class=source>{{Book}}{{#Author}} — {{Author}}{{/Author}}{{#Chapter}}<br>{{Chapter}}{{/Chapter}}</div>",
        )],
        json!([[0, "any", [0]]]),
        now,
    );
    let mut decks = json!({"1": deck(1, "Default", now), VOCAB_DECK.to_string(): deck(VOCAB_DECK, "Kollate::Vocabulary", now)});
    let mut models = json!({VOCAB_MODEL.to_string(): vocab_model});
    if include_highlights {
        decks[HIGHLIGHT_DECK.to_string()] = deck(HIGHLIGHT_DECK, "Kollate::Highlights", now);
        models[HIGHLIGHT_MODEL.to_string()] = highlight_model;
    }
    let conf = json!({"activeDecks": [1], "curDeck": 1, "newSpread": 0, "collapseTime": 1200, "timeLim": 0,
        "estTimes": true, "dueCounts": true, "curModel": null, "nextPos": 1, "sortType": "noteFld",
        "sortBackwards": false, "addToCur": true});
    let dconf = json!({"1": {"id": 1, "name": "Default", "mod": 0, "usn": 0, "maxTaken": 60, "autoplay": true,
        "timer": 0, "replayq": true, "dyn": false,
        "new": {"bury": true, "delays": [1, 10], "initialFactor": 2500, "ints": [1, 4, 7], "order": 1, "perDay": 20, "separate": true},
        "rev": {"bury": true, "ease4": 1.3, "fuzz": 0.05, "ivlFct": 1, "maxIvl": 36500, "minSpace": 1, "perDay": 100},
        "lapse": {"delays": [10], "leechAction": 0, "leechFails": 8, "minInt": 1, "mult": 0}}});
    conn.execute(
        "INSERT INTO col VALUES (1, ?1, ?2, ?2, 11, 0, 0, 0, ?3, ?4, ?5, ?6, '{}')",
        params![
            now - now % 86400,
            now * 1000,
            conf.to_string(),
            models.to_string(),
            decks.to_string(),
            dconf.to_string()
        ],
    )?;

    let mut w = Writer {
        conn,
        now,
        due: 1,
        cards: 0,
    };
    let mut stats = AnkiStats::default();
    let mut seen_words = std::collections::HashSet::new();
    for eb in &books {
        let book_tag = anki_tag(&crate::export::safe_file_name(&eb.book.title));
        for v in &eb.vocab {
            if !seen_words.insert(v.vocab.id) {
                continue;
            }
            let word = &v.vocab.word;
            let lemma = v
                .vocab
                .lemma
                .clone()
                .filter(|l| !l.eq_ignore_ascii_case(word))
                .unwrap_or_default();
            let sighting = v.sightings.iter().find(|s| s.context.is_some());
            let (context, blank) = match sighting.and_then(|s| Some((s, s.context.as_deref()?))) {
                Some((s, sentence)) => {
                    match [s.surface_form.as_str(), word.as_str()]
                        .iter()
                        .find_map(|w| word_span(sentence, w))
                    {
                        Some((a, b)) => (
                            format!(
                                "{}<b>{}</b>{}",
                                html(&sentence[..a]),
                                html(&sentence[a..b]),
                                html(&sentence[b..])
                            ),
                            format!(
                                "{}<span class=cloze>[…]</span>{}",
                                html(&sentence[..a]),
                                html(&sentence[b..])
                            ),
                        ),
                        None => (html(sentence), String::new()),
                    }
                }
                None => (String::new(), String::new()),
            };
            let fields = [
                html(word),
                html(&lemma),
                html(v.vocab.definition.as_deref().unwrap_or("")),
                context,
                blank.clone(),
                html(&eb.book.title),
            ];
            let ords: &[usize] = if blank.is_empty() { &[0] } else { &[0, 1] };
            let tags = vec!["kollate".to_owned(), "vocab".to_owned(), book_tag.clone()];
            w.note(
                &format!("kollate:vocab:{}", v.vocab.id),
                VOCAB_MODEL,
                VOCAB_DECK,
                &fields,
                &tags,
                ords,
            )?;
            stats.words += 1;
        }
        if include_highlights {
            for a in eb.annotations.iter().filter(|a| a.text().is_some()) {
                let fields = [
                    html(a.text().unwrap_or_default()),
                    html(a.note().unwrap_or("")),
                    html(&eb.book.title),
                    html(eb.book.author.as_deref().unwrap_or("")),
                    html(a.chapter_title.as_deref().unwrap_or("")),
                ];
                let mut tags = vec![
                    "kollate".to_owned(),
                    "highlight".to_owned(),
                    book_tag.clone(),
                ];
                tags.extend(a.tags.iter().map(|t| anki_tag(t)));
                w.note(
                    &format!("kollate:highlight:{}", a.id),
                    HIGHLIGHT_MODEL,
                    HIGHLIGHT_DECK,
                    &fields,
                    &tags,
                    &[0],
                )?;
                stats.highlights += 1;
            }
        }
    }
    stats.cards = w.cards;
    drop(w);

    // Package: collection + (empty) media map.
    let tmp = out.with_extension("apkg.part");
    let file = std::fs::File::create(&tmp)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let io = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
    zip.start_file("collection.anki2", opts).map_err(io)?;
    zip.write_all(&std::fs::read(&db_path)?)?;
    zip.start_file("media", opts).map_err(io)?;
    zip.write_all(b"{}")?;
    zip.finish().map_err(io)?;
    std::fs::rename(&tmp, out)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_and_ids_are_stable() {
        assert_eq!(guid("kollate:vocab:1"), guid("kollate:vocab:1"));
        assert_ne!(guid("kollate:vocab:1"), guid("kollate:vocab:2"));
        assert!(guid("x").bytes().all(|b| BASE91.contains(&b)));
        assert!(note_id("kollate:vocab:1") > 0);
    }

    #[test]
    fn checksum_matches_anki() {
        // sha1("hello") = aaf4c61d…
        assert_eq!(checksum("<b>hello</b>"), 0xaaf4c61d);
    }
}
