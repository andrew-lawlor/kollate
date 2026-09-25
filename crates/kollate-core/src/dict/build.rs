//! Converters from dictionary sources into Kollate's dictionary format.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;
use rusqlite::{Connection, params};

use super::{INDEXES, SCHEMA, fold, insert_meta};
use crate::Result;

/// Writes a dictionary database. Rows are buffered in one transaction and
/// indexes are built at the end.
pub struct DictionaryBuilder {
    conn: Connection,
    ranks: HashMap<String, i64>,
}

impl DictionaryBuilder {
    /// Creates `out` (replacing any existing file).
    pub fn create(out: &Path, name: &str, language: Option<&str>, source: &str) -> Result<Self> {
        crate::kobo::ensure_not_on_kobo(out)?;
        if out.exists() {
            std::fs::remove_file(out)?;
        }
        let conn = Connection::open(out)?;
        conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;
        conn.execute_batch(SCHEMA)?;
        insert_meta(&conn, "name", name)?;
        insert_meta(&conn, "source", source)?;
        if let Some(lang) = language {
            insert_meta(&conn, "language", lang)?;
        }
        conn.execute_batch("BEGIN")?;
        Ok(Self {
            conn,
            ranks: HashMap::new(),
        })
    }

    pub fn meta(&self, key: &str, value: &str) -> Result<()> {
        insert_meta(&self.conn, key, value)
    }

    pub fn sense(
        &mut self,
        headword: &str,
        pos: Option<&str>,
        gloss: &str,
        example: Option<&str>,
    ) -> Result<()> {
        let gloss = gloss.trim();
        if headword.trim().is_empty() || gloss.is_empty() {
            return Ok(());
        }
        let key = fold(headword);
        let rank = self.ranks.entry(key.clone()).or_insert(0);
        self.conn.prepare_cached(
            "INSERT INTO entry (key, headword, pos, gloss, example, rank) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![key, headword.trim(), pos, gloss, example.map(str::trim), *rank])?;
        *rank += 1;
        Ok(())
    }

    pub fn form(&mut self, form: &str, headword: &str) -> Result<()> {
        let (form, key) = (fold(form), fold(headword));
        if !form.is_empty() && form != key {
            self.conn
                .prepare_cached("INSERT INTO form (form, key) VALUES (?1, ?2)")?
                .execute(params![form, key])?;
        }
        Ok(())
    }

    /// Commits, indexes and returns the number of senses written.
    pub fn finish(self) -> Result<usize> {
        self.conn.execute_batch("COMMIT")?;
        self.conn.execute_batch(INDEXES)?;
        self.conn.execute_batch("VACUUM")?;
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))?
            as usize)
    }
}

fn gz_or_plain(path: &Path) -> Result<Box<dyn Read>> {
    let file = std::fs::File::open(path)?;
    Ok(
        if path.extension().is_some_and(|e| e == "gz" || e == "dz") {
            Box::new(flate2::read::MultiGzDecoder::new(file))
        } else {
            Box::new(file)
        },
    )
}

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Builds from Open English WordNet in WN-LMF XML (`english-wordnet-*.xml[.gz]`).
pub fn build_from_wordnet_lmf(input: &Path, out: &Path) -> Result<usize> {
    struct Entry {
        lemma: String,
        pos: String,
        forms: Vec<String>,
        synsets: Vec<String>,
    }
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(gz_or_plain(input)?));
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut synsets: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut current_synset: Option<String> = None;
    let mut in_definition = false;
    let mut in_example = false;
    let mut text = String::new();
    let mut version = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "Lexicon" => version = attr(&e, "version").unwrap_or_default(),
                "LexicalEntry" => entries.push(Entry {
                    lemma: String::new(),
                    pos: String::new(),
                    forms: Vec::new(),
                    synsets: Vec::new(),
                }),
                "Lemma" => {
                    if let Some(entry) = entries.last_mut() {
                        entry.lemma = attr(&e, "writtenForm").unwrap_or_default();
                        entry.pos = attr(&e, "partOfSpeech").unwrap_or_default();
                    }
                }
                "Form" => {
                    if let (Some(entry), Some(form)) = (entries.last_mut(), attr(&e, "writtenForm"))
                    {
                        entry.forms.push(form);
                    }
                }
                "Sense" => {
                    if let (Some(entry), Some(synset)) = (entries.last_mut(), attr(&e, "synset")) {
                        entry.synsets.push(synset);
                    }
                }
                "Synset" => current_synset = attr(&e, "id"),
                "Definition" => {
                    in_definition = true;
                    text.clear();
                }
                "Example" => {
                    in_example = true;
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_definition || in_example => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_definition || in_example => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    text.push(c);
                } else {
                    text.push_str(match &*r {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "Definition" => {
                    in_definition = false;
                    if let Some(id) = &current_synset {
                        synsets.entry(id.clone()).or_default().0 = text.trim().to_owned();
                    }
                }
                "Example" => {
                    in_example = false;
                    if let Some(id) = &current_synset {
                        let slot = &mut synsets.entry(id.clone()).or_default().1;
                        if slot.is_none() {
                            *slot = Some(text.trim().trim_matches('"').to_owned());
                        }
                    }
                }
                "Synset" => current_synset = None,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(
                    std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()).into(),
                );
            }
            _ => {}
        }
        buf.clear();
    }

    let mut builder = DictionaryBuilder::create(
        out,
        format!("Open English WordNet {version}").trim(),
        Some("en"),
        "https://en-word.net (CC BY 4.0)",
    )?;
    builder.meta("license", "CC BY 4.0")?;
    for entry in &entries {
        for synset in &entry.synsets {
            if let Some((gloss, example)) = synsets.get(synset) {
                builder.sense(&entry.lemma, Some(&entry.pos), gloss, example.as_deref())?;
            }
        }
        for form in &entry.forms {
            builder.form(form, &entry.lemma)?;
        }
    }
    builder.finish()
}

/// Strips HTML tags and decodes a few entities (StarDict `h` entries).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Builds from a StarDict dictionary, given its `.ifo` file (the `.idx`
/// and `.dict`/`.dict.dz` must sit beside it).
pub fn build_from_stardict(ifo: &Path, out: &Path) -> Result<usize> {
    let info: HashMap<String, String> = std::fs::read_to_string(ifo)?
        .lines()
        .filter_map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    let base = ifo.with_extension("");
    let find = |exts: &[&str]| {
        exts.iter()
            .map(|e| base.with_extension(e))
            .find(|p| p.is_file())
    };
    let invalid = |msg: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_owned());
    let idx_path = find(&["idx", "idx.gz"]).ok_or_else(|| invalid("missing .idx file"))?;
    let dict_path = find(&["dict", "dict.dz"]).ok_or_else(|| invalid("missing .dict file"))?;
    let mut idx = Vec::new();
    gz_or_plain(&idx_path)?.read_to_end(&mut idx)?;
    let mut data = Vec::new();
    gz_or_plain(&dict_path)?.read_to_end(&mut data)?;
    let offset_bits: usize = info
        .get("idxoffsetbits")
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    let same_type = info.get("sametypesequence").cloned();

    let name = info
        .get("bookname")
        .cloned()
        .unwrap_or_else(|| "StarDict".into());
    let mut builder = DictionaryBuilder::create(out, &name, None, "StarDict")?;
    let mut pos = 0;
    while pos < idx.len() {
        let Some(nul) = idx[pos..].iter().position(|b| *b == 0) else {
            break;
        };
        let word = String::from_utf8_lossy(&idx[pos..pos + nul]).into_owned();
        pos += nul + 1;
        let (offset, size) = if offset_bits == 64 {
            let o = u64::from_be_bytes(
                idx.get(pos..pos + 8)
                    .ok_or_else(|| invalid("truncated .idx"))?
                    .try_into()
                    .unwrap(),
            );
            pos += 8;
            (o as usize, 0)
        } else {
            let o = u32::from_be_bytes(
                idx.get(pos..pos + 4)
                    .ok_or_else(|| invalid("truncated .idx"))?
                    .try_into()
                    .unwrap(),
            );
            pos += 4;
            (o as usize, 0)
        };
        let size = size
            + u32::from_be_bytes(
                idx.get(pos..pos + 4)
                    .ok_or_else(|| invalid("truncated .idx"))?
                    .try_into()
                    .unwrap(),
            ) as usize;
        pos += 4;
        let Some(raw) = data.get(offset..offset + size) else {
            continue;
        };
        // Without sametypesequence each field starts with its type letter.
        let body = match &same_type {
            Some(_) => raw,
            None => raw.get(1..).unwrap_or_default(),
        };
        let text = strip_html(&String::from_utf8_lossy(body).replace('\0', " "));
        builder.sense(&word, None, &text, None)?;
    }
    builder.finish()
}

/// Builds from a kaikki.org Wiktionary extract (JSON Lines, optionally
/// gzipped). Only entries in the first language seen are used.
pub fn build_from_kaikki(input: &Path, out: &Path, name: &str) -> Result<usize> {
    let reader = BufReader::new(gz_or_plain(input)?);
    let mut builder: Option<DictionaryBuilder> = None;
    let mut language: Option<String> = None;
    for line in reader.lines() {
        let line = line?;
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let lang = entry["lang_code"].as_str().unwrap_or_default().to_owned();
        if language.get_or_insert_with(|| lang.clone()) != &lang {
            continue;
        }
        let builder = match &mut builder {
            Some(b) => b,
            None => builder.insert(DictionaryBuilder::create(
                out,
                name,
                Some(&lang).filter(|l| !l.is_empty()).map(String::as_str),
                "Wiktionary via kaikki.org (CC BY-SA)",
            )?),
        };
        let Some(word) = entry["word"].as_str() else {
            continue;
        };
        let pos = entry["pos"].as_str();
        for sense in entry["senses"].as_array().into_iter().flatten() {
            if let Some(base) = sense["form_of"].get(0).and_then(|f| f["word"].as_str()) {
                builder.form(word, base)?;
                continue;
            }
            let Some(gloss) = sense["glosses"].get(0).and_then(|g| g.as_str()) else {
                continue;
            };
            let example = sense["examples"].get(0).and_then(|e| e["text"].as_str());
            builder.sense(word, pos, gloss, example)?;
        }
    }
    match builder {
        Some(b) => b.finish(),
        None => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no entries found").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dictionary;

    const LMF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LexicalResource><Lexicon id="oewn" language="en" version="2025">
  <LexicalEntry id="e1"><Lemma writtenForm="theophany" partOfSpeech="n"/>
    <Sense id="s1" synset="y1"/></LexicalEntry>
  <LexicalEntry id="e2"><Lemma writtenForm="mouse" partOfSpeech="n"/><Form writtenForm="mice"/>
    <Sense id="s2" synset="y2"/></LexicalEntry>
  <LexicalEntry id="e3"><Lemma writtenForm="seance" partOfSpeech="n"/><Sense id="s3" synset="y3"/></LexicalEntry>
  <Synset id="y1" partOfSpeech="n"><Definition>a visible manifestation of a deity</Definition></Synset>
  <Synset id="y2" partOfSpeech="n"><Definition>small rodent</Definition>
    <Example>&quot;the mice ran&quot;</Example></Synset>
  <Synset id="y3" partOfSpeech="n"><Definition>a meeting of spiritualists &amp; mediums</Definition></Synset>
</Lexicon></LexicalResource>"#;

    #[test]
    fn builds_and_looks_up_wordnet() {
        let dir = tempfile::tempdir().unwrap();
        let xml = dir.path().join("wn.xml");
        std::fs::write(&xml, LMF).unwrap();
        let db = dir.path().join("wn.db");
        assert_eq!(build_from_wordnet_lmf(&xml, &db).unwrap(), 3);

        let dict = Dictionary::open(&db).unwrap();
        assert_eq!(dict.name, "Open English WordNet 2025");
        let d = dict.lookup("Theophanies").unwrap().unwrap();
        assert_eq!(d.headword, "theophany");
        assert_eq!(d.to_text(3), "noun: a visible manifestation of a deity");
        assert_eq!(
            dict.lookup("mice").unwrap().unwrap().senses[0]
                .example
                .as_deref(),
            Some("the mice ran")
        );
        assert_eq!(
            dict.lookup("séances").unwrap().unwrap().senses[0].gloss,
            "a meeting of spiritualists & mediums"
        );
        assert!(dict.lookup("hierophant").unwrap().is_none());
    }

    #[test]
    fn builds_from_stardict() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"<b>hierophant</b> an interpreter of sacred mysteries";
        let mut idx = b"hierophant\0".to_vec();
        idx.extend_from_slice(&0u32.to_be_bytes());
        idx.extend_from_slice(&(data.len() as u32).to_be_bytes());
        std::fs::write(dir.path().join("d.idx"), idx).unwrap();
        std::fs::write(dir.path().join("d.dict"), data).unwrap();
        std::fs::write(
            dir.path().join("d.ifo"),
            "StarDict's dict ifo file\nbookname=Test Dict\nsametypesequence=h\n",
        )
        .unwrap();
        let db = dir.path().join("sd.db");
        assert_eq!(
            build_from_stardict(&dir.path().join("d.ifo"), &db).unwrap(),
            1
        );
        let dict = Dictionary::open(&db).unwrap();
        assert_eq!(dict.name, "Test Dict");
        assert_eq!(
            dict.lookup("hierophants").unwrap().unwrap().senses[0].gloss,
            "hierophant an interpreter of sacred mysteries"
        );
    }

    #[test]
    fn builds_from_kaikki() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("k.jsonl");
        std::fs::write(
            &jsonl,
            [
                r#"{"word":"hierophant","pos":"noun","lang_code":"en","senses":[{"glosses":["A priest who interprets sacred mysteries."],"examples":[{"text":"The hierophant spoke."}]}]}"#,
                r#"{"word":"hierophants","pos":"noun","lang_code":"en","senses":[{"glosses":["plural of hierophant"],"form_of":[{"word":"hierophant"}]}]}"#,
                r#"{"word":"Hund","pos":"noun","lang_code":"de","senses":[{"glosses":["dog"]}]}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let db = dir.path().join("k.db");
        assert_eq!(build_from_kaikki(&jsonl, &db, "Wiktionary").unwrap(), 1);
        let dict = Dictionary::open(&db).unwrap();
        assert_eq!(dict.language.as_deref(), Some("en"));
        let d = dict.lookup("hierophants").unwrap().unwrap();
        assert_eq!(d.headword, "hierophant");
        assert_eq!(
            d.senses[0].example.as_deref(),
            Some("The hierophant spoke.")
        );
    }
}
