//! Splits a markup's ink into notes (handwriting) and marks (underlines and
//! circles on the printed text), from stroke geometry alone.
//!
//! Each `<path>` in a Kobo markup SVG is one pen stroke, in the order it was
//! written (verified on a Libra Colour, firmware 4.45).

/// A box in page pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    fn centre(&self) -> (f32, f32) {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    fn contains(&self, (x, y): (f32, f32)) -> bool {
        (self.left..=self.right).contains(&x) && (self.top..=self.bottom).contains(&y)
    }

    fn union(boxes: impl IntoIterator<Item = Bounds>) -> Option<Bounds> {
        boxes.into_iter().reduce(|a, b| Bounds {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })
    }

    /// The distance between two boxes along each axis (0 where they overlap).
    fn gap(&self, other: &Bounds) -> (f32, f32) {
        (
            (self.left.max(other.left) - self.right.min(other.right)).max(0.0),
            (self.top.max(other.top) - self.bottom.min(other.bottom)).max(0.0),
        )
    }
}

/// One pen stroke: its `<path …/>` element and bounding box.
#[derive(Debug, Clone)]
pub struct Stroke {
    pub xml: String,
    pub bounds: Bounds,
}

/// The strokes of a Kobo markup SVG, in writing order.
pub fn strokes(svg: &str) -> Vec<Stroke> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<path") {
        let Some(len) = rest[start..].find("/>") else {
            break;
        };
        let xml = &rest[start..start + len + 2];
        rest = &rest[start + len + 2..];
        let Some(d) = xml.split(" d=\"").nth(1).and_then(|d| d.split('"').next()) else {
            continue;
        };
        // Path data is absolute M/L/C commands; every pair of numbers is a point.
        let numbers: Vec<f32> = d
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|n| n.parse().ok())
            .collect();
        let points = numbers.as_chunks::<2>().0.iter().map(|&[x, y]| Bounds {
            left: x,
            top: y,
            right: x,
            bottom: y,
        });
        if let Some(bounds) = Bounds::union(points) {
            out.push(Stroke {
                xml: xml.to_owned(),
                bounds,
            });
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Underline,
    Circle,
}

/// An underline or circle on the printed text.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    /// Indexes into the strokes. A multi-line underline has one per line.
    pub strokes: Vec<usize>,
}

/// How a note must be turned to read upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    /// Written down the page: turn a quarter anticlockwise.
    Anticlockwise,
    /// Written up the page: turn a quarter clockwise.
    Clockwise,
}

/// A piece of handwriting.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Indexes into the strokes, in writing order, excluding any circle
    /// drawn around the writing.
    pub strokes: Vec<usize>,
    pub bounds: Bounds,
    pub rotation: Rotation,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Segments {
    pub notes: Vec<Note>,
    pub marks: Vec<Mark>,
}

/// Classifies the strokes. Tuned on real markups: an underline is one long,
/// flat stroke; a circle is one large stroke, which belongs to a note when it
/// encloses other strokes (circled handwriting) and is a mark otherwise.
pub fn segment(strokes: &[Stroke]) -> Segments {
    let mut marks: Vec<Mark> = Vec::new();
    let mut writing = Vec::new(); // (stroke, is a circle around writing)
    for (i, s) in strokes.iter().enumerate() {
        let b = s.bounds;
        if b.width() >= 150.0 && b.height() <= 0.12 * b.width() {
            // One underline carried over consecutive lines is one mark.
            match marks.last_mut() {
                Some(m) if m.kind == MarkKind::Underline && m.strokes.last() == Some(&(i - 1)) => {
                    m.strokes.push(i)
                }
                _ => marks.push(Mark {
                    kind: MarkKind::Underline,
                    strokes: vec![i],
                }),
            }
        } else if b.width() >= 100.0 && b.height() >= 60.0 {
            let encloses = strokes
                .iter()
                .enumerate()
                .any(|(j, t)| j != i && b.contains(t.bounds.centre()));
            if encloses {
                writing.push((i, true));
            } else {
                marks.push(Mark {
                    kind: MarkKind::Circle,
                    strokes: vec![i],
                });
            }
        } else {
            writing.push((i, false));
        }
    }

    // Join strokes that come within about a letter's height of each other.
    let mut heights: Vec<f32> = writing
        .iter()
        .filter(|(_, circle)| !circle)
        .map(|&(i, _)| strokes[i].bounds.height())
        .collect();
    heights.sort_by(f32::total_cmp);
    let letter = heights.get(heights.len() / 2).copied().unwrap_or(40.0);
    let reach = 0.8 * letter;
    let mut group: Vec<usize> = (0..writing.len()).collect();
    fn root(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    for a in 0..writing.len() {
        for b in a + 1..writing.len() {
            let (sa, sb) = (&strokes[writing[a].0].bounds, &strokes[writing[b].0].bounds);
            let (gx, gy) = sa.gap(sb);
            let near = gx < reach && gy < reach;
            let circled = (writing[a].1 && sa.contains(sb.centre()))
                || (writing[b].1 && sb.contains(sa.centre()));
            if near || circled {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra] = rb;
            }
        }
    }
    let mut groups: Vec<Vec<(usize, bool)>> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for (k, &w) in writing.iter().enumerate() {
        let r = root(&mut group, k);
        let slot = *index.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(w);
    }

    let mut notes = Vec::new();
    for g in groups {
        let ink: Vec<usize> = g.iter().filter(|(_, c)| !c).map(|&(i, _)| i).collect();
        let Some(bounds) = Bounds::union(ink.iter().map(|&i| strokes[i].bounds)) else {
            continue; // an empty circle
        };
        if bounds.width().max(bounds.height()) < 12.0 {
            continue; // a stray speck
        }
        let sideways = bounds.height() > 1.8 * bounds.width() && ink.len() >= 3;
        let rotation = if !sideways {
            Rotation::None
        } else if strokes[*ink.last().unwrap()].bounds.centre().1
            > strokes[ink[0]].bounds.centre().1
        {
            // Writing runs the way the pen moved.
            Rotation::Anticlockwise
        } else {
            Rotation::Clockwise
        };
        notes.push(Note {
            strokes: ink,
            bounds,
            rotation,
        });
    }
    Segments { notes, marks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stroke as Qt writes it: a filled outline around the pen's path.
    fn path(points: &[(f32, f32)]) -> String {
        let d: Vec<String> = points
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("{}{x},{y}", if i == 0 { "M" } else { " L" }))
            .collect();
        format!(
            r#"<path vector-effect="none" fill-rule="nonzero" d="{}"/>"#,
            d.join("")
        )
    }

    fn rect(l: f32, t: f32, r: f32, b: f32) -> String {
        path(&[(l, t), (r, t), (r, b), (l, b)])
    }

    /// A word of `n` letter strokes, 30 wide and 40 tall, starting at (x, y).
    fn word(x: f32, y: f32, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| rect(x + i as f32 * 34.0, y, x + i as f32 * 34.0 + 30.0, y + 40.0))
            .collect()
    }

    fn svg(paths: Vec<String>) -> String {
        format!(
            "<svg width=\"1264\" height=\"1680\" viewBox=\"0 0 1264 1680\">\n<g fill=\"#000000\">\n{}\n</g></svg>",
            paths.join("\n")
        )
    }

    #[test]
    fn reads_strokes_in_order() {
        let s = strokes(&svg(vec![
            r#"<path d="M103,1094.5 L101.939,1094.06 C99,1 90,1100 95,1090 "/>"#.into(),
            rect(10.0, 20.0, 30.0, 40.0),
        ]));
        assert_eq!(s.len(), 2);
        assert_eq!(
            s[0].bounds,
            Bounds {
                left: 90.0,
                top: 1.0,
                right: 103.0,
                bottom: 1100.0
            }
        );
        assert!(s[1].xml.starts_with("<path"));
    }

    #[test]
    fn separates_notes_underlines_and_circles() {
        let mut paths = Vec::new();
        // An underline over two lines, then a note beside it.
        paths.push(rect(500.0, 444.0, 1215.0, 470.0));
        paths.push(rect(95.0, 521.0, 644.0, 537.0));
        paths.extend(word(680.0, 480.0, 5));
        // A circle around printed text (nothing inside it).
        paths.push(rect(58.0, 1066.0, 600.0, 1160.0));
        // Circled handwriting, far away.
        paths.push(rect(766.0, 703.0, 997.0, 924.0));
        paths.extend(word(800.0, 790.0, 3));
        // A stray speck.
        paths.push(rect(684.0, 1358.0, 688.0, 1362.0));
        let s = segment(&strokes(&svg(paths)));

        assert_eq!(s.marks.len(), 2);
        assert_eq!(s.marks[0].kind, MarkKind::Underline);
        assert_eq!(s.marks[0].strokes, vec![0, 1]);
        assert_eq!(s.marks[1].kind, MarkKind::Circle);
        assert_eq!(s.notes.len(), 2);
        assert_eq!(s.notes[0].strokes, vec![2, 3, 4, 5, 6]);
        // The circle around the writing isn't part of the note's ink.
        assert_eq!(s.notes[1].strokes, vec![9, 10, 11]);
        assert!(s.notes.iter().all(|n| n.rotation == Rotation::None));
    }

    #[test]
    fn turns_sideways_writing_the_way_it_was_written() {
        // Letters stacked down the page, written top to bottom.
        let down: Vec<String> = (0..5)
            .map(|i| {
                rect(
                    300.0,
                    700.0 + i as f32 * 45.0,
                    340.0,
                    735.0 + i as f32 * 45.0,
                )
            })
            .collect();
        let s = segment(&strokes(&svg(down.clone())));
        assert_eq!(s.notes.len(), 1);
        assert_eq!(s.notes[0].rotation, Rotation::Anticlockwise);

        let up: Vec<String> = down.into_iter().rev().collect();
        assert_eq!(
            segment(&strokes(&svg(up))).notes[0].rotation,
            Rotation::Clockwise
        );
    }
}
