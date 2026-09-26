//! Converter for DictFile (`.df`, optionally `.bz2`/`.gz`), the plain-text
//! dictionary format used by reader.dict's Wiktionary extracts:
//!
//! ```text
//! @ hierophant
//! : /ˈhaɪəɹəˌfænt/
//! & hierophants
//! <html><p><b>Noun</b></p><ol><li>An interpreter of sacred mysteries…</li></ol><p>From Ancient Greek…</p>
//! ```
//!
//! `@` starts an entry, `:` is a pronunciation, `&` an inflected form, and
//! the HTML holds one `<p><b>Part of speech</b></p><ol>` block per part of
//! speech, followed by etymology paragraphs.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;

use super::build::DictionaryBuilder;
use crate::Result;

/// One sense: part of speech (`n`, `v`, `a`, `r` or a lowercase label) and gloss.
pub(crate) type Sense = (Option<String>, String);

fn pos_code(heading: &str) -> Option<String> {
    let h = heading.trim().to_lowercase();
    // Sections that aren't parts of speech (as used in reader.dict's extracts).
    const NOT_POS: &[&str] = &[
        "synonym",
        "antonym",
        "related term",
        "note",
        "etymolog",
        "see also",
        "pronunciation",
        "pronounciation",
        "conjugation",
        "declension",
        "inflection",
        "syllable",
        "metadata",
        "tools",
        "available files",
    ];
    if h.chars().count() < 2 || NOT_POS.iter().any(|n| h.contains(n)) {
        return None;
    }
    Some(
        match h.as_str() {
            "noun" => "n",
            "verb" => "v",
            "adjective" => "a",
            "adverb" => "r",
            other => other,
        }
        .to_owned(),
    )
}

fn entity(name: &str) -> &'static str {
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        _ => "",
    }
}

/// Nested list items that aren't sub-senses: synonym lists and quotations.
fn is_aside(text: &str) -> bool {
    const LABELS: &[&str] = &[
        "Synonym",
        "Antonym",
        "Hypernym",
        "Hyponym",
        "Meronym",
        "Holonym",
        "Coordinate term",
        "See also",
    ];
    let t = text.trim_start();
    if LABELS
        .iter()
        .any(|l| t.starts_with(l) && t[l.len()..].trim_start().starts_with(['s', ':']))
    {
        return true;
    }
    // Quotations start with a year: "2011, Richard …", "c. 1921, Michael …".
    let t = t.strip_prefix("c.").map(str::trim_start).unwrap_or(t);
    t.chars().take_while(char::is_ascii_digit).count() >= 3
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Senses from one entry's HTML, in order.
pub(crate) fn senses(html: &str) -> Vec<Sense> {
    let mut reader = quick_xml::Reader::from_str(html);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;

    let mut out: Vec<Sense> = Vec::new();
    let mut pos: Option<String> = None;
    let mut skip_section = false;
    let mut in_p = false;
    let mut heading: Option<String> = None;
    let mut in_b = false;
    let mut ol_depth = 0usize;
    // Open list items, innermost last: (depth, text, sub-senses).
    let mut items: Vec<(usize, String, Vec<String>)> = Vec::new();
    // The last closed top-level sense. reader.dict often puts a sense's
    // sub-list *after* its </li>, so it stays open for sub-senses until the
    // next sense starts.
    let mut pending: Option<(String, Vec<String>)> = None;

    fn emit(
        out: &mut Vec<Sense>,
        pos: &Option<String>,
        pending: &mut Option<(String, Vec<String>)>,
    ) {
        let Some((text, children)) = pending.take() else {
            return;
        };
        // A sense that ends mid-sentence ("Synonym of demon, particularly
        // as") introduces its sub-senses.
        let introduces = !text.is_empty() && !text.ends_with(['.', '!', '?', ')', '"', '”']);
        if children.is_empty() || !introduces {
            if !text.is_empty() {
                out.push((pos.clone(), text));
            }
            out.extend(children.into_iter().map(|c| (pos.clone(), c)));
        } else {
            let lead = text.trim_end_matches(':').to_owned();
            out.extend(
                children
                    .into_iter()
                    .map(|c| (pos.clone(), format!("{lead}: {c}"))),
            );
        }
    }

    let push_text = |items: &mut Vec<(usize, String, Vec<String>)>,
                     heading: &mut Option<String>,
                     in_b: bool,
                     s: &str| {
        if let Some((_, text, _)) = items.last_mut() {
            text.push_str(s);
        } else if in_b && let Some(h) = heading {
            h.push_str(s);
        }
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                "p" if ol_depth == 0 => {
                    if !skip_section {
                        emit(&mut out, &pos, &mut pending);
                    }
                    pending = None;
                    in_p = true;
                    heading = Some(String::new());
                }
                "b" if in_p => in_b = true,
                "ol" => ol_depth += 1,
                "li" => {
                    if ol_depth <= 1 && !skip_section {
                        emit(&mut out, &pos, &mut pending);
                    }
                    items.push((ol_depth, String::new(), Vec::new()));
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "b" => in_b = false,
                "p" if in_p => {
                    in_p = false;
                    // A <p> with a bold heading opens a part of speech; other
                    // paragraphs (etymology) are ignored.
                    if let Some(h) = heading.take().filter(|h| !h.trim().is_empty()) {
                        pos = pos_code(&h);
                        skip_section = pos.is_none();
                    }
                }
                "ol" => ol_depth = ol_depth.saturating_sub(1),
                "li" => {
                    let Some((depth, text, children)) = items.pop() else {
                        continue;
                    };
                    let text = clean(&text);
                    if depth >= 2 {
                        if text.is_empty() || is_aside(&text) {
                            continue;
                        }
                        match items.last_mut() {
                            Some((_, _, siblings)) => siblings.push(text),
                            None => {
                                if let Some((_, siblings)) = pending.as_mut() {
                                    siblings.push(text);
                                }
                            }
                        }
                        continue;
                    }
                    if !skip_section {
                        pending = Some((text, children));
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) if e.local_name().as_ref() == "br" => {
                push_text(&mut items, &mut heading, in_b, " ");
            }
            Ok(Event::Text(t)) => push_text(&mut items, &mut heading, in_b, &t.xml10_content()),
            Ok(Event::GeneralRef(r)) => {
                let s = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => entity(&r).to_owned(),
                };
                push_text(&mut items, &mut heading, in_b, &s);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    if !skip_section {
        emit(&mut out, &pos, &mut pending);
    }
    out
}

fn open(path: &Path) -> Result<Box<dyn Read>> {
    let file = std::fs::File::open(path)?;
    let name = path.to_string_lossy();
    Ok(if name.ends_with(".bz2") {
        Box::new(bzip2_rs::DecoderReader::new(file))
    } else if name.ends_with(".gz") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    })
}

/// Builds a dictionary from a DictFile. Returns the number of senses written.
pub fn build_from_dictfile(
    input: &Path,
    out: &Path,
    name: &str,
    language: Option<&str>,
    source: &str,
) -> Result<usize> {
    let mut builder = DictionaryBuilder::create(out, name, language, source)?;
    let reader = BufReader::with_capacity(1 << 20, open(input)?);
    let mut headword: Option<String> = None;
    let mut forms: Vec<String> = Vec::new();
    let mut body = String::new();

    let flush = |builder: &mut DictionaryBuilder,
                 headword: &mut Option<String>,
                 forms: &mut Vec<String>,
                 body: &mut String|
     -> Result<()> {
        if let Some(h) = headword.take() {
            let senses = senses(body);
            if !senses.is_empty() {
                for (pos, gloss) in &senses {
                    builder.sense(&h, pos.as_deref(), gloss, None)?;
                }
                for f in forms.iter() {
                    builder.form(f, &h)?;
                }
            }
        }
        forms.clear();
        body.clear();
        Ok(())
    };

    for line in reader.lines() {
        let line = line?;
        if let Some(h) = line.strip_prefix("@ ") {
            flush(&mut builder, &mut headword, &mut forms, &mut body)?;
            headword = Some(h.trim().to_owned());
        } else if let Some(f) = line.strip_prefix("& ") {
            forms.push(f.trim().to_owned());
        } else if line.starts_with(": ") || line.trim().is_empty() {
            // Pronunciation, or the blank line between entries.
        } else if headword.is_some() {
            body.push_str(&line);
        }
    }
    flush(&mut builder, &mut headword, &mut forms, &mut body)?;
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_parts_of_speech_and_skips_etymology() {
        let html = r#"<html><p><b>Noun</b></p><ol><li>(<i>Ancient Greece</i>) An ancient Greek priest.</li><li>An interpreter of sacred mysteries.</li></ol><p>From Ancient Greek <i>ἱεροφάντης</i>.</p><br/>"#;
        assert_eq!(
            senses(html),
            [
                (
                    Some("n".into()),
                    "(Ancient Greece) An ancient Greek priest.".into()
                ),
                (
                    Some("n".into()),
                    "An interpreter of sacred mysteries.".into()
                ),
            ]
        );
    }

    #[test]
    fn nested_sub_senses_are_kept_with_their_lead_in() {
        let html = r#"<html><p><b>Noun</b></p><ol><li>Synonym of <i>demon</i>, <i>particularly</i> as</li><ol style="list-style-type:lower-alpha"><li>(<i>Greek mythology</i>) A tutelary deity or spirit that watches over a person or place.</li></ol></ol>"#;
        assert_eq!(
            senses(html),
            [(
                Some("n".into()),
                "Synonym of demon, particularly as: (Greek mythology) A tutelary deity or spirit that watches over a person or place."
                    .into()
            )]
        );
    }

    #[test]
    fn synonym_lists_quotations_and_usage_notes_are_skipped() {
        let html = r#"<html><p><b>Verb</b></p><ol><li>(<i>transitive</i>) To form an opinion on; to appraise.</li><ol style="list-style-type:lower-alpha"><li>Synonyms: evaluate, rate, appraise</li><li>c. 1921, Michael Collins, after the Anglo-Irish Treaty:</li></ol><li>(transitive,&#32;obsolete) To award judicially.</li></ol><p><b>Usage Note</b></p><ol><li>For information about the usage, see referee.</li></ol><p><b>Synonym</b></p><ol><li>money</li></ol><p><b>Interjection</b></p><ol><li>Used to encourage someone.</li><ol><li><b>2011</b>, Richard Bigwood, <i>We Were Reos</i></li></ol></ol>"#;
        assert_eq!(
            senses(html),
            [
                (
                    Some("v".into()),
                    "(transitive) To form an opinion on; to appraise.".into()
                ),
                (
                    Some("v".into()),
                    "(transitive, obsolete) To award judicially.".into()
                ),
                (
                    Some("interjection".into()),
                    "Used to encourage someone.".into()
                ),
            ]
        );
    }

    #[test]
    fn builds_and_looks_up_a_dictfile() {
        let dir = tempfile::tempdir().unwrap();
        let df = dir.path().join("en.df");
        std::fs::write(
            &df,
            "@ debouche\n& debouched\n<html><p><b>Verb</b></p><ol><li>(military) To enter into battle.</li></ol>\n\n\
             @ debouch\n: /dɪˈbaʊt͡ʃ/\n& debouched\n& debouches\n<html><p><b>Verb</b></p><ol><li>To march out into open ground.</li></ol>\n\n\
             @ hierophant\n& hierophants\n<html><p><b>Noun</b></p><ol><li>An interpreter of sacred mysteries.</li></ol>\n\n\
             @ empty\n<html><p>Only etymology.</p>\n",
        )
        .unwrap();
        let db = dir.path().join("en.db");
        assert_eq!(
            build_from_dictfile(&df, &db, "Test Wiktionary", Some("en"), "test").unwrap(),
            3
        );
        let dict = crate::dict::Dictionary::open(&db).unwrap();
        assert_eq!(
            dict.lookup("Hierophants").unwrap().unwrap().headword,
            "hierophant"
        );
        // An inflection listed under two headwords resolves to the one the
        // inflection rules point to (debouched → debouch, not debouche).
        let d = dict.lookup("debouched").unwrap().unwrap();
        assert_eq!(
            (d.headword.as_str(), d.senses[0].gloss.as_str()),
            ("debouch", "To march out into open ground.")
        );
        assert!(
            dict.lookup("empty").unwrap().is_none(),
            "entries without senses are dropped"
        );
    }
}
