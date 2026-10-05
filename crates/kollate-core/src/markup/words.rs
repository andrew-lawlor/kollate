//! What a mark covers, from where the words on the page are: KOReader's
//! Pencil export lists every word on the page with its box (SPEC §8f), so
//! an underline or circle is resolved to the book's own words by geometry,
//! with no model reading print and nothing to snap.

use serde::{Deserialize, Serialize};

use super::segment::{Mark, MarkKind, Stroke};

/// A word on the page: its text, and one box per line it's on (a word
/// hyphenated across lines has two), as `[x0, y0, x1, y1]` in page pixels.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PageWord {
    pub text: String,
    pub boxes: Vec<[f32; 4]>,
    /// What's printed between this word and the next: a space, ", ", "-",
    /// "’". The plugin splits words at hyphens and apostrophes ("Nestor",
    /// "s"), so passages are joined with this. Exports before Pencil 0.6.6
    /// have none: those are joined with spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// The passages a mark covers, in reading order: for each underline stroke,
/// the words on the line just above it across its width; for a circle, the
/// words inside it. Words that follow one another in the book are one
/// passage, so an underline carried onto the next line reads as one, and
/// underlines of separate passages drawn one after another as several.
pub fn marked(mark: &Mark, strokes: &[Stroke], words: &[PageWord]) -> Vec<String> {
    let mut chosen = vec![false; words.len()];
    for &i in &mark.strokes {
        let b = strokes[i].bounds;
        let mid = (b.top + b.bottom) / 2.0;
        // How much of a box lies within left..right.
        let across = |bx: &[f32; 4], left: f32, right: f32| {
            let w = (bx[2] - bx[0]).max(1.0);
            (bx[2].min(right) - bx[0].max(left)).max(0.0) / w
        };
        match mark.kind {
            MarkKind::Underline => {
                // The line whose bottom is nearest the underline, among
                // those that start above it (as on Nickel's page, §8a).
                let line = words
                    .iter()
                    .flat_map(|w| &w.boxes)
                    .filter(|bx| bx[1] < mid)
                    .min_by(|a, c| (a[3] - mid).abs().total_cmp(&(c[3] - mid).abs()));
                let Some(line) = line.copied() else { continue };
                let centre = (line[1] + line[3]) / 2.0;
                for (k, w) in words.iter().enumerate() {
                    if w.boxes.iter().any(|bx| {
                        (bx[1]..=bx[3]).contains(&centre)
                            && across(bx, b.left - 12.0, b.right + 12.0) > 0.5
                    }) {
                        chosen[k] = true;
                    }
                }
            }
            MarkKind::Circle => {
                for (k, w) in words.iter().enumerate() {
                    if w.boxes.iter().any(|bx| {
                        let cy = (bx[1] + bx[3]) / 2.0;
                        (b.top..=b.bottom).contains(&cy) && across(bx, b.left, b.right) > 0.6
                    }) {
                        chosen[k] = true;
                    }
                }
            }
        }
    }
    let mut passages: Vec<Vec<&PageWord>> = Vec::new();
    let mut last = None;
    for (k, w) in words.iter().enumerate().filter(|&(k, _)| chosen[k]) {
        match passages.last_mut() {
            Some(p) if last == Some(k - 1) => p.push(w),
            _ => passages.push(vec![w]),
        }
        last = Some(k);
    }
    passages.into_iter().map(|p| join(&p)).collect()
}

/// A passage's words as printed: each followed by what came after it, with
/// any run of whitespace (a line break between lines of verse) as one space.
fn join(words: &[&PageWord]) -> String {
    let mut text = String::new();
    for (i, w) in words.iter().enumerate() {
        text.push_str(&w.text);
        if i + 1 == words.len() {
            break;
        }
        let mut in_space = false;
        for c in w.after.as_deref().unwrap_or(" ").chars() {
            if c.is_whitespace() {
                if !in_space {
                    text.push(' ');
                }
                in_space = true;
            } else {
                text.push(c);
                in_space = false;
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::segment::strokes;

    fn word(text: &str, x0: f32, y0: f32, x1: f32) -> PageWord {
        PageWord {
            text: text.into(),
            boxes: vec![[x0, y0, x1, y0 + 40.0]],
            after: None,
        }
    }

    /// Two lines of four words each.
    fn page() -> Vec<PageWord> {
        vec![
            word("Laertes,", 20.0, 100.0, 180.0),
            word("when", 200.0, 100.0, 300.0),
            word("his", 320.0, 100.0, 380.0),
            word("fatal", 400.0, 100.0, 500.0),
            word("With", 20.0, 160.0, 110.0),
            word("death’s", 130.0, 160.0, 280.0),
            word("long", 300.0, 160.0, 380.0),
            word("sleep.", 400.0, 160.0, 520.0),
        ]
    }

    fn mark(kind: MarkKind, d: &str) -> (Mark, Vec<Stroke>) {
        let svg = format!("<svg><path d=\"{d}\"/></svg>");
        (
            Mark {
                kind,
                strokes: vec![0],
            },
            strokes(&svg),
        )
    }

    #[test]
    fn an_underline_marks_the_words_on_the_line_above_it() {
        // Under "when his" on the first line, a little short at both ends.
        let (m, s) = mark(MarkKind::Underline, "M210,146 L300,147 L370,146");
        assert_eq!(marked(&m, &s, &page()), ["when his"]);
    }

    #[test]
    fn a_circle_marks_the_words_inside_it_and_not_its_neighbours() {
        // Around "death’s", grazing the ends of "With" and "long".
        let (m, s) = mark(
            MarkKind::Circle,
            "M100,150 L290,150 L310,175 L290,210 L100,210 L95,180 L100,150",
        );
        assert_eq!(marked(&m, &s, &page()), ["death’s"]);
    }

    #[test]
    fn a_word_split_across_lines_is_found_by_either_part() {
        let mut words = page();
        words.push(PageWord {
            text: "Achaian".into(),
            boxes: vec![[540.0, 100.0, 620.0, 140.0], [20.0, 220.0, 90.0, 260.0]],
            after: None,
        });
        let (m, s) = mark(MarkKind::Underline, "M15,266 L95,267");
        assert_eq!(marked(&m, &s, &words), ["Achaian"]);
    }

    #[test]
    fn a_passage_keeps_its_punctuation_and_split_words() {
        let w = |text: &str, after: &str, x: f32| PageWord {
            text: text.into(),
            boxes: vec![[x, 100.0, x + 60.0, 140.0]],
            after: Some(after.into()),
        };
        // "Nestor’s son, ocean-side" as the plugin's word walk splits it,
        // with a line break before the last word.
        let words = vec![
            w("Nestor", "’", 20.0),
            w("s", " ", 90.0),
            w("son", ",\n", 160.0),
            w("ocean", "-", 230.0),
            w("side", ";", 300.0),
        ];
        let (m, s) = mark(MarkKind::Underline, "M15,146 L365,147");
        assert_eq!(marked(&m, &s, &words), ["Nestor’s son, ocean-side"]);
    }

    #[test]
    fn underlines_carried_over_lines_are_one_passage_and_apart_are_two() {
        let svg = "<svg><path d=\"M390,146 L505,146\"/><path d=\"M15,206 L120,206\"/>\
                   <path d=\"M290,206 L390,206\"/></svg>";
        let s = strokes(svg);
        let m = Mark {
            kind: MarkKind::Underline,
            strokes: vec![0, 1, 2],
        };
        // "fatal" at the end of the first line runs on to "With"; "long" is
        // apart.
        assert_eq!(marked(&m, &s, &page()), ["fatal With", "long"]);
    }
}
