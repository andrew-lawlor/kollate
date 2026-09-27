//! Handwriting transcription for stylus markups (SPEC §8a).
//!
//! The ink is split into notes and marks by geometry ([`segment`]); marks are
//! resolved to the book's own words ([`snap`]); only the handwriting needs a
//! model, which the caller supplies as a [`Reader`] (see `kollate-transcribe`).

pub mod image;
pub mod segment;
mod snap;

pub use image::{Page, RgbImage};
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
    let segments = segment::segment(&strokes);

    let mut notes = Vec::new();
    for note in &segments.notes {
        let img = image::render_note(svg, &strokes, note, 16)?;
        // A note's own line breaks are just where the margin ran out.
        let text = reader
            .handwriting(&img)?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let text = match (context.book_words, context.known_word) {
            (Some(words), Some(known)) => snap::correct_names(&text, words, known),
            _ => text,
        };
        if !text.is_empty() {
            notes.push(text);
        }
    }

    let mut marked = Vec::new();
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
            if !text.is_empty() {
                marked.push(text);
            }
        }
    }

    let join = |v: Vec<String>| (!v.is_empty()).then(|| v.join("\n"));
    Ok(Transcription {
        text: join(marked),
        note: join(notes),
    })
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
                text: None,
                note: Some("Woah, this works well!".into())
            }
        );
    }
}
