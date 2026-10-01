//! Handwriting transcription for stylus markups (SPEC §8a).
//!
//! The ink is split into notes and marks by geometry ([`segment`]); marks are
//! resolved to the book's own words ([`snap`]); only the handwriting needs a
//! model, which the caller supplies as a [`Reader`] (see `kollate-transcribe`).

pub mod image;
pub mod marks;
pub mod segment;
pub mod shape;
mod snap;

pub use image::{Page, RgbImage};
pub(crate) use snap::similarity as snap_similarity;
pub use snap::snap;

use crate::Result;

/// Reads text from an image. Implemented by a local vision model.
pub trait Reader {
    /// Handwriting: a note rendered alone, black on white.
    fn handwriting(&mut self, image: &RgbImage) -> Result<String>;
    /// Printed text: a line of the book under a mark.
    fn print(&mut self, image: &RgbImage) -> Result<String>;
}

/// A markup as text: what it marks in the book, and what was written.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Transcription {
    /// The printed words underlined or circled, one mark per line.
    pub text: Option<String>,
    /// The handwriting, one note per line.
    pub note: Option<String>,
    /// Single words circled on the page, as the book spells them (glosses).
    pub circled: Vec<String>,
}

/// What helps read a markup besides its ink.
#[derive(Default, Clone, Copy)]
pub struct Context<'a> {
    /// The Kobo's page image, needed to find what marks cover.
    pub page_jpeg: Option<&'a [u8]>,
    /// The book's words around the markup: marked text is snapped to them,
    /// and misread names in notes corrected against them.
    pub book_words: Option<&'a [String]>,
    /// Whether a (lowercase) word is in a dictionary. Name correction only
    /// touches words it doesn't know; without it, notes are left as read.
    pub known_word: Option<&'a dyn Fn(&str) -> bool>,
}

/// Transcribes one markup.
pub fn transcribe(
    svg: &str,
    context: Context<'_>,
    reader: &mut dyn Reader,
) -> Result<Transcription> {
    let strokes = segment::strokes(svg);
    let mut segments = segment::segment(&strokes);
    // A loop around a word or two is too small to be a circle by size alone,
    // and would otherwise be "read" as handwriting.
    let mut writing = Vec::new();
    for note in std::mem::take(&mut segments.notes) {
        if shape::is_word_loop(svg, &strokes, &note)? {
            segments.marks.push(segment::Mark {
                kind: segment::MarkKind::Circle,
                strokes: note.strokes,
            });
        } else {
            writing.push(note);
        }
    }
    // In page order, top to bottom, whatever order they were drawn in.
    let top = |m: &segment::Mark| {
        m.strokes
            .iter()
            .map(|&i| strokes[i].bounds.top)
            .fold(f32::MAX, f32::min)
    };
    segments.marks.sort_by(|a, b| top(a).total_cmp(&top(b)));

    let mut notes = Vec::new();
    for note in &writing {
        // A drawn star and a question mark are known by their shape; the
        // model tends to read them as "5" and "3".
        let text = if shape::is_star(&strokes, note) {
            "*".to_owned()
        } else if shape::is_question(&strokes, note) {
            "?".to_owned()
        } else {
            let img = image::render_note(svg, &strokes, note, 16)?;
            // A note's own line breaks are just where the margin ran out.
            let text = reader.handwriting(&img)?;
            without_runaway(&text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let text = match (context.book_words, context.known_word) {
            (Some(words), Some(known)) => snap::correct_names(&text, words, known),
            _ => text,
        };
        if !text.is_empty() {
            notes.push(text);
        }
    }

    let mut marked = Vec::new();
    let mut circled = Vec::new();
    if let (Some(jpeg), false) = (context.page_jpeg, segments.marks.is_empty()) {
        let page = Page::from_jpeg(jpeg)?;
        for mark in &segments.marks {
            let mut reading = Vec::new();
            for crop in page.marked(mark, &strokes) {
                reading.push(
                    reader
                        .print(&crop)?
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            let reading = reading.join(" ");
            let text = context
                .book_words
                .and_then(|words| snap(&reading, words))
                .unwrap_or_else(|| snap::tidy(&reading));
            let text = trim_overshoot(&text);
            if mark.kind == segment::MarkKind::Circle
                && let Some(word) = single_word(&text)
            {
                circled.push(word.to_owned());
            }
            if !text.is_empty() {
                marked.push(text);
            }
        }
    }

    let join = |v: Vec<String>| (!v.is_empty()).then(|| v.join("\n"));
    Ok(Transcription {
        text: join(marked),
        note: join(notes),
        circled,
    })
}

/// Drops the word or two a pen runs on past a full stop ("…humanity. At"),
/// or starts before one ("man.” Over the centuries…"): nobody means to mark
/// the first word of the next sentence, or the last of the one before.
fn trim_overshoot(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    // A word ending a sentence (not a paragraph number like "8.").
    let ends = |w: &str| {
        w.chars().any(char::is_alphabetic)
            && w.trim_end_matches(['"', '\'', '”', '’', ')', ']'])
                .trim_end_matches(|c: char| c.is_ascii_digit())
                .ends_with(['.', '!', '?', '…'])
    };
    let n = words.len();
    let mut from = 0;
    let mut to = n;
    // A sentence ending within the last three words, with at most two after it.
    if let Some(i) = (n.saturating_sub(3)..n.saturating_sub(1))
        .rev()
        .find(|&i| ends(words[i]))
        && i >= 2
    {
        to = i + 1;
    }
    // One ending within the first two words, with more than two after it.
    if let Some(i) = (0..2.min(n)).find(|&i| ends(words[i]))
        && to - i - 1 > 2
    {
        from = i + 1;
    }
    let text = words[from..to].join(" ");
    // A bracket whose partner lies outside the mark ("novarum)").
    match (text.contains('('), text.contains(')')) {
        (false, true) => text.trim_end_matches(')').to_owned(),
        (true, false) => text.trim_start_matches('(').to_owned(),
        _ => text,
    }
}

/// Where a model reading starts repeating itself ("+ 1 + 1 + 1…", "2222…"),
/// as a byte index: a run of at least five copies of a piece up to eight
/// characters long, at least 16 characters in all. Small models do this on
/// ink they can't read, until they run out of tokens.
pub fn runaway_start(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    // Whether `len` characters from `i` repeat enough to be a loop.
    let looping = |i: usize, len: usize| {
        let Some(unit) = chars.get(i..i + len) else {
            return false;
        };
        if unit.iter().all(|c| c.is_whitespace()) {
            return false;
        }
        let mut copies = 1;
        while chars.get(i + copies * len..i + (copies + 1) * len) == Some(unit) {
            copies += 1;
        }
        copies >= 5 && copies * len >= 16
    };
    for i in 0..chars.len() {
        for len in 1..=8 {
            if looping(i, len) {
                // "line the the the…" loops from "e the" as much as from
                // "the ": start at a word if the loop still holds there.
                let at = (i..i + len)
                    .find(|&j| (j == 0 || chars[j - 1].is_whitespace()) && looping(j, len))
                    .unwrap_or(i);
                return Some(starts[at]);
            }
        }
    }
    None
}

/// A reading without the loop a model fell into, or nothing if that leaves
/// less than a word (what came before a loop is rarely real).
fn without_runaway(text: &str) -> &str {
    let Some(at) = runaway_start(text) else {
        return text;
    };
    let kept = text[..at].trim_end();
    if kept.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        ""
    } else {
        kept
    }
}

/// The word, if `text` is one word (letters, with a hyphen or apostrophe
/// inside), without punctuation around it.
fn single_word(text: &str) -> Option<&str> {
    let word = text.trim_matches(|c: char| !c.is_alphanumeric());
    let inner = |c: char| c.is_alphabetic() || matches!(c, '-' | '\'' | '’');
    (word.chars().count() >= 2
        && word.chars().all(inner)
        && word.starts_with(char::is_alphabetic)
        && word.ends_with(char::is_alphabetic))
    .then_some(word)
}

/// Reads a notebook page: its writing line by line, one line per line of
/// text. Drawings are left out: a piece of one that reaches the model (an
/// eye, an arrowhead, a box) reads as a stray character or a runaway repeat,
/// and is dropped; an arrowhead touching a label is trimmed off it.
pub fn read_page(svg: &str, reader: &mut dyn Reader) -> Result<Option<String>> {
    let strokes = segment::strokes(svg);
    let mut lines = Vec::new();
    for line in segment::lines(&strokes) {
        // A star or question mark on a line of its own is a pen mark (§8c).
        if shape::is_star(&strokes, &line) {
            lines.push("*".to_owned());
            continue;
        }
        if shape::is_question(&strokes, &line) {
            lines.push("?".to_owned());
            continue;
        }
        let img = image::render_note(svg, &strokes, &line, 16)?;
        let text = reader.handwriting(&img)?;
        let mut words: Vec<String> = without_runaway(&text)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let arrow = |w: &String| w.chars().all(|c| "<>-–—→←=".contains(c));
        while words.first().is_some_and(arrow) {
            words.remove(0);
        }
        while words.last().is_some_and(arrow) {
            words.pop();
        }
        let text = words.join(" ");
        let letters = text.chars().filter(|c| c.is_alphabetic()).count();
        let distinct: std::collections::HashSet<char> = text.chars().collect();
        let runaway = text.chars().count() > 12 && distinct.len() < 4;
        let mark = matches!(text.as_str(), "*" | "?");
        if line.strokes.len() <= 2 && (letters < 2 || runaway) && !mark {
            continue;
        }
        if !text.is_empty() {
            lines.push(text);
        }
    }
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers from a script, and records what it was shown.
    struct Scripted {
        handwriting: Vec<&'static str>,
        print: Vec<&'static str>,
        seen: Vec<(u32, u32)>,
    }

    impl Reader for Scripted {
        fn handwriting(&mut self, image: &RgbImage) -> Result<String> {
            self.seen.push((image.width, image.height));
            Ok(self.handwriting.remove(0).to_owned())
        }
        fn print(&mut self, _: &RgbImage) -> Result<String> {
            Ok(self.print.remove(0).to_owned())
        }
    }

    #[test]
    fn drops_a_reading_that_runs_away() {
        assert_eq!(runaway_start("Need 114 K Y"), None);
        assert_eq!(runaway_start("Wait... what?!! Really???"), None);
        assert_eq!(runaway_start("hahaha, yes"), None);
        assert_eq!(runaway_start("+ + 1 + 1 + 1 + 1 + 1 + 1 + 1"), Some(2));
        assert_eq!(without_runaway("2222222222222222222222"), "");
        assert_eq!(without_runaway("+ + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1"), "");
        assert_eq!(
            without_runaway("Lovely line the the the the the the the"),
            "Lovely line"
        );
        assert_eq!(without_runaway("What a line!"), "What a line!");
    }

    #[test]
    fn trims_what_the_pen_ran_on_to() {
        assert_eq!(
            trim_overshoot(
                "Over the centuries, development has improved the living conditions of humanity. At"
            ),
            "Over the centuries, development has improved the living conditions of humanity."
        );
        assert_eq!(
            trim_overshoot("man.” Over the centuries, technological development has improved"),
            "Over the centuries, technological development has improved"
        );
        assert_eq!(trim_overshoot("novarum)"), "novarum");
        assert_eq!(trim_overshoot("(rerum novarum)"), "(rerum novarum)");
        // A marked sentence, or two, is left whole.
        assert_eq!(trim_overshoot("Call me Ishmael."), "Call me Ishmael.");
        assert_eq!(
            trim_overshoot("It was cold. It was dark."),
            "It was cold. It was dark."
        );
        assert_eq!(
            trim_overshoot("8. The Book of Nehemiah, in turn"),
            "8. The Book of Nehemiah, in turn"
        );
    }

    #[test]
    fn single_words_only() {
        assert_eq!(single_word("Nehemiah,"), Some("Nehemiah"));
        assert_eq!(single_word("“knight-errant”"), Some("knight-errant"));
        assert_eq!(single_word("o’er"), Some("o’er"));
        assert_eq!(single_word("two words"), None);
        assert_eq!(single_word("1984"), None);
        assert_eq!(single_word("a"), None);
    }

    #[test]
    fn reads_a_page_line_by_line_without_drawings() {
        let rect = |l: f32, t: f32, r: f32, b: f32| {
            format!("<path d=\"M{l},{t} L{r},{t} L{r},{b} L{l},{b}\"/>")
        };
        let mut paths = String::new();
        for (y, n) in [(100.0, 6), (300.0, 4)] {
            for i in 0..n {
                let x = 100.0 + i as f32 * 34.0;
                paths += &rect(x, y, x + 30.0, y + 40.0);
            }
        }
        // An eye of a drawn face: one small stroke on its own.
        paths += &rect(700.0, 700.0, 730.0, 730.0);
        let svg = format!(
            "<svg width=\"1264\" height=\"1680\" viewBox=\"0 0 1264 1680\"><g>{paths}</g></svg>"
        );
        let mut reader = Scripted {
            handwriting: vec!["This is  my", "-> page <", "0"],
            print: vec![],
            seen: vec![],
        };
        assert_eq!(
            read_page(&svg, &mut reader).unwrap().as_deref(),
            Some("This is my\npage")
        );
        assert_eq!(reader.seen.len(), 3);
    }

    #[test]
    fn transcribes_notes_without_a_page() {
        let svg = "<svg width=\"1264\" height=\"1680\" viewBox=\"0 0 1264 1680\"><g>\
            <path d=\"M100,100 L130,100 L130,140 L100,140\"/>\
            <path d=\"M134,100 L164,100 L164,140 L134,140\"/>\
            <path d=\"M600,900 L630,900 L630,940 L600,940\"/>\
            <path d=\"M634,900 L664,900 L664,940 L634,940\"/></g></svg>";
        let mut reader = Scripted {
            handwriting: vec!["Woah, this works\nwell!", "  "],
            print: vec![],
            seen: vec![],
        };
        let t = transcribe(svg, Context::default(), &mut reader).unwrap();
        // Two notes; the second read as nothing and is dropped.
        assert_eq!(reader.seen, vec![(96, 72), (96, 72)]);
        assert_eq!(
            t,
            Transcription {
                note: Some("Woah, this works well!".into()),
                ..Default::default()
            }
        );
    }
}
