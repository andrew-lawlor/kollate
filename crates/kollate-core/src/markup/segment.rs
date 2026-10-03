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

    pub(crate) fn centre(&self) -> (f32, f32) {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    pub(crate) fn contains(&self, (x, y): (f32, f32)) -> bool {
        (self.left..=self.right).contains(&x) && (self.top..=self.bottom).contains(&y)
    }

    pub(crate) fn union(boxes: impl IntoIterator<Item = Bounds>) -> Option<Bounds> {
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
    /// The points of its outline (for telling shapes apart).
    pub points: Vec<(f32, f32)>,
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
        let points: Vec<(f32, f32)> = numbers
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[x, y]| (x, y))
            .collect();
        // A stroked path (KOReader's ink is pen centre lines) is as wide as
        // its pen; Nickel's ink is filled outlines, which already are.
        let half = xml
            .split(" stroke-width=\"")
            .nth(1)
            .and_then(|w| w.split('"').next())
            .and_then(|w| w.parse::<f32>().ok())
            .map_or(0.0, |w| w / 2.0);
        let boxes = points.iter().map(|&(x, y)| Bounds {
            left: x - half,
            top: y - half,
            right: x + half,
            bottom: y + half,
        });
        if let Some(bounds) = Bounds::union(boxes) {
            out.push(Stroke {
                xml: xml.to_owned(),
                bounds,
                points,
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

    // Join strokes that come within about a letter's height of each other
    // vertically (lines of one note), or twice that sideways (the space
    // between words, which some hands and fonts make wide).
    let mut heights: Vec<f32> = writing
        .iter()
        .filter(|(_, circle)| !circle)
        .map(|&(i, _)| strokes[i].bounds.height())
        .collect();
    heights.sort_by(f32::total_cmp);
    let letter = heights.get(heights.len() / 2).copied().unwrap_or(40.0);
    let across = group_writing(strokes, &writing, 1.6 * letter, 0.8 * letter);
    // Writing down the margin has its word gaps the other way, so across
    // the page it falls apart into pieces a word or two long, each read
    // sideways on its own. Grouped again with the reaches swapped, and
    // letters measured across (a sideways letter's height is its width on
    // the page), it holds together: a long, narrow note made only of pieces
    // that aren't lines across the page.
    let ink_bounds = |g: &[usize]| {
        Bounds::union(
            g.iter()
                .filter(|&&k| !writing[k].1)
                .map(|&k| strokes[writing[k].0].bounds),
        )
    };
    let upright = |g: &[usize]| ink_bounds(g).is_none_or(|b| b.height() < 0.5 * b.width());
    let mut widths: Vec<f32> = across
        .iter()
        .filter(|g| g.len() >= 3 && !upright(g))
        .flatten()
        .filter(|&&k| !writing[k].1)
        .map(|&k| strokes[writing[k].0].bounds.width())
        .collect();
    widths.sort_by(f32::total_cmp);
    let mut joined: Vec<usize> = (0..across.len()).collect();
    if let Some(&side) = widths.get(widths.len() / 2) {
        let piece_of: std::collections::HashMap<usize, usize> = across
            .iter()
            .enumerate()
            .flat_map(|(a, g)| g.iter().map(move |&k| (k, a)))
            .collect();
        for down in group_writing(strokes, &writing, 0.8 * side, 1.6 * side) {
            let Some(b) = ink_bounds(&down) else { continue };
            // At least a word or two: not a question mark and its dot.
            if down.len() < 6 || b.height() < 5.0 * side || b.height() <= 1.8 * b.width() {
                continue;
            }
            let mut parts: Vec<usize> = down.iter().map(|k| piece_of[k]).collect();
            parts.sort_unstable();
            parts.dedup();
            // A line across the page is wider than it's tall, and longer
            // than a couple of words side by side in two sideways lines.
            let across_the_page = |g: &[usize]| {
                ink_bounds(g).is_none_or(|p| p.height() < 0.5 * p.width() && p.width() > 3.0 * side)
            };
            if parts.len() < 2 || parts.iter().any(|&a| across_the_page(&across[a])) {
                continue;
            }
            for pair in parts.windows(2) {
                let (ra, rb) = (root(&mut joined, pair[0]), root(&mut joined, pair[1]));
                joined[ra] = rb;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot_of = std::collections::HashMap::new();
    for (a, g) in across.into_iter().enumerate() {
        let r = root(&mut joined, a);
        let slot = *slot_of.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].extend(g);
    }
    // Pieces of one line of writing with a wider word gap than the first
    // pass allows (a hand with room to spare, as on the Elipsa) join: each a
    // line or less, mostly level with the other, and within three letters
    // sideways. A lone word read on its own loses its context ("a" read as
    // "1"). A piece about a letter wide joins only with writing on both
    // sides: at the start or end of a line it's a mark (a star, a "?"), read
    // better on its own.
    let one_line = |b: &Bounds| b.height() < 4.0 * letter;
    let bounds: Vec<Option<Bounds>> = groups.iter().map(|g| ink_bounds(g)).collect();
    let beside = |a: usize, b: usize| {
        let (Some(ba), Some(bb)) = (bounds[a], bounds[b]) else {
            return false;
        };
        let (gx, gy) = ba.gap(&bb);
        let overlap = ba.bottom.min(bb.bottom) - ba.top.max(bb.top);
        one_line(&ba)
            && one_line(&bb)
            && gy == 0.0
            && overlap >= 0.5 * ba.height().min(bb.height())
            && gx < 3.0 * letter
    };
    let short = |a: usize| bounds[a].is_some_and(|b| b.width() <= 2.0 * letter);
    let flanked = |a: usize| {
        let Some(ba) = bounds[a] else { return false };
        let side = |left: bool| {
            (0..groups.len()).any(|b| {
                b != a
                    && !short(b)
                    && beside(a, b)
                    && bounds[b].is_some_and(|bb| (bb.centre().0 < ba.centre().0) == left)
            })
        };
        side(true) && side(false)
    };
    let mut level: Vec<usize> = (0..groups.len()).collect();
    for a in 0..groups.len() {
        for b in a + 1..groups.len() {
            if beside(a, b) && (!short(a) || flanked(a)) && (!short(b) || flanked(b)) {
                let (ra, rb) = (root(&mut level, a), root(&mut level, b));
                level[ra] = rb;
            }
        }
    }
    let mut lined: Vec<Vec<usize>> = Vec::new();
    let mut slot_of = std::collections::HashMap::new();
    for (a, g) in groups.into_iter().enumerate() {
        let r = root(&mut level, a);
        let slot = *slot_of.entry(r).or_insert_with(|| {
            lined.push(Vec::new());
            lined.len() - 1
        });
        lined[slot].extend(g);
    }
    let groups: Vec<Vec<(usize, bool)>> = lined
        .into_iter()
        .map(|g| {
            let mut g: Vec<(usize, bool)> = g.into_iter().map(|k| writing[k]).collect();
            g.sort_unstable();
            g
        })
        .collect();

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

/// The representative of `i`'s set in a union-find forest.
fn root(group: &mut [usize], mut i: usize) -> usize {
    while group[i] != i {
        group[i] = group[group[i]];
        i = group[i];
    }
    i
}

/// Groups `writing` (strokes, and whether each is a circle around writing)
/// into notes: strokes closer than `reach_x` sideways and `reach_y`
/// vertically join, as does writing with the circle around it. Returns
/// indexes into `writing`, each group in writing order, groups in the order
/// their first stroke was written.
fn group_writing(
    strokes: &[Stroke],
    writing: &[(usize, bool)],
    reach_x: f32,
    reach_y: f32,
) -> Vec<Vec<usize>> {
    let mut group: Vec<usize> = (0..writing.len()).collect();
    for a in 0..writing.len() {
        for b in a + 1..writing.len() {
            let (sa, sb) = (&strokes[writing[a].0].bounds, &strokes[writing[b].0].bounds);
            let (gx, gy) = sa.gap(sb);
            let near = gx < reach_x && gy < reach_y;
            let circled = (writing[a].1 && sa.contains(sb.centre()))
                || (writing[b].1 && sb.contains(sa.centre()));
            if near || circled {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra] = rb;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for k in 0..writing.len() {
        let r = root(&mut group, k);
        let slot = *index.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(k);
    }
    groups
}

/// A notebook page's writing as lines, in reading order, each split where a
/// gap wider than six letters separates two pieces (labels side by side).
/// Strokes much taller or longer than a letter are drawings, left out; so is
/// anything drawn apart from the writing, which comes back as its own short
/// piece (see [`read_page`](super::read_page)).
pub fn lines(strokes: &[Stroke]) -> Vec<Note> {
    let mut heights: Vec<f32> = strokes
        .iter()
        .map(|s| s.bounds.height())
        .filter(|&h| h > 8.0)
        .collect();
    heights.sort_by(f32::total_cmp);
    let h = heights.get(heights.len() / 2).copied().unwrap_or(40.0);
    let text: Vec<usize> = (0..strokes.len())
        .filter(|&i| {
            let b = strokes[i].bounds;
            b.height() <= 2.5 * h && b.width() <= 6.0 * h
        })
        .collect();

    // Lines, in two passes. First the letter-sized strokes, leaving out dots,
    // apostrophes and tall ascenders and descenders, which reach towards the
    // neighbouring lines: in order of height, a jump of most of a letter
    // between centres starts the next line. Then the other strokes join the
    // line nearest them, if it's within a letter.
    let centre = |i: usize| (strokes[i].bounds.top + strokes[i].bounds.bottom) / 2.0;
    let (mut core, rest): (Vec<usize>, Vec<usize>) = text
        .into_iter()
        .partition(|&i| (0.5 * h..=1.5 * h).contains(&strokes[i].bounds.height()));
    core.sort_by(|&a, &b| centre(a).total_cmp(&centre(b)));
    let mut rows: Vec<(Bounds, Vec<usize>)> = Vec::new();
    let mut last = f32::MIN;
    for i in core {
        let b = strokes[i].bounds;
        match rows.last_mut() {
            Some((r, v)) if centre(i) - last < 0.8 * h => {
                *r = Bounds::union([*r, b]).expect("two boxes");
                v.push(i);
            }
            _ => rows.push((b, vec![i])),
        }
        last = centre(i);
    }
    for i in rest {
        let (c, b) = (centre(i), strokes[i].bounds);
        let distance = |r: &Bounds| (r.top - c).max(c - r.bottom).max(0.0);
        match rows
            .iter_mut()
            .min_by(|x, y| distance(&x.0).total_cmp(&distance(&y.0)))
        {
            Some((r, v)) if distance(r) < h => {
                *r = Bounds::union([*r, b]).expect("two boxes");
                v.push(i);
            }
            _ => rows.push((b, vec![i])),
        }
    }

    let mut out = Vec::new();
    for (_, mut row) in rows {
        row.sort_by(|&a, &b| strokes[a].bounds.left.total_cmp(&strokes[b].bounds.left));
        let mut piece: Vec<usize> = Vec::new();
        let mut right = f32::MIN;
        for i in row {
            let b = strokes[i].bounds;
            if !piece.is_empty() && b.left - right > 6.0 * h {
                out.push(std::mem::take(&mut piece));
            }
            right = right.max(b.right);
            piece.push(i);
        }
        out.push(piece);
    }
    let mut notes: Vec<Note> = out
        .into_iter()
        .map(|mut v| {
            let bounds = Bounds::union(v.iter().map(|&i| strokes[i].bounds)).expect("not empty");
            v.sort_unstable();
            Note {
                strokes: v,
                bounds,
                rotation: Rotation::None,
            }
        })
        .collect();
    notes.sort_by(|a, b| {
        (a.bounds.top / h)
            .round()
            .total_cmp(&(b.bounds.top / h).round())
            .then(a.bounds.left.total_cmp(&b.bounds.left))
    });
    notes
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

    /// A word written down the page: `n` letters 40 wide and 20 tall
    /// (sideways), from (x, y) downwards.
    fn word_down(x: f32, y: f32, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| rect(x, y + i as f32 * 24.0, x + 40.0, y + i as f32 * 24.0 + 20.0))
            .collect()
    }

    #[test]
    fn keeps_a_note_down_the_margin_whole() {
        // Two lines written down the margin, words far apart along them, and
        // a question mark with its dot elsewhere on the page.
        let mut paths = Vec::new();
        for (x, top) in [(1180.0, 200.0), (1120.0, 200.0)] {
            let mut y = top;
            for n in [4, 2, 5] {
                paths.extend(word_down(x, y, n));
                y += n as f32 * 24.0 + 45.0;
            }
        }
        paths.push(path(&[(400.0, 520.0), (430.0, 525.0), (420.0, 560.0)]));
        paths.push(rect(418.0, 575.0, 424.0, 581.0));
        let ss = strokes(&svg(paths));
        let s = segment(&ss);
        let down: Vec<&Note> = s.notes.iter().filter(|n| n.bounds.left > 1000.0).collect();
        assert_eq!(down.len(), 1, "{:?}", s.notes);
        assert_eq!(down[0].strokes.len(), 22);
        assert_eq!(down[0].rotation, Rotation::Anticlockwise);
        assert!(
            s.notes
                .iter()
                .filter(|n| n.bounds.left < 1000.0)
                .all(|n| n.rotation == Rotation::None),
            "{:?}",
            s.notes
        );
    }

    #[test]
    fn splits_a_notebook_page_into_lines() {
        let mut paths = Vec::new();
        // Second line first: lines come back in reading order, not writing order.
        paths.extend(word(100.0, 300.0, 5));
        // First line: two words, with an apostrophe above and a tall "b"
        // reaching up from the line (neither may start a line of its own).
        paths.extend(word(100.0, 100.0, 4));
        paths.push(rect(260.0, 80.0, 266.0, 92.0));
        paths.extend(word(300.0, 100.0, 3));
        paths.push(rect(410.0, 60.0, 440.0, 140.0));
        // A label far to the right of the second line: a piece of its own.
        paths.extend(word(900.0, 300.0, 2));
        // A drawing: a big box and a long arrow.
        paths.push(rect(200.0, 600.0, 600.0, 900.0));
        paths.push(rect(100.0, 1000.0, 900.0, 1010.0));
        let s = strokes(&svg(paths));
        let lines = lines(&s);
        let sizes: Vec<_> = lines
            .iter()
            .map(|n| (n.bounds.top as i32, n.bounds.left as i32, n.strokes.len()))
            .collect();
        assert_eq!(sizes, [(60, 100, 9), (300, 100, 5), (300, 900, 2)]);
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
    fn keeps_widely_spaced_words_in_one_note() {
        // Two words 45 apart (more than a letter's height, as some hands
        // write), and a second line under them: one note. A word far off is
        // another note.
        let mut paths = word(100.0, 100.0, 4);
        paths.extend(word(100.0 + 4.0 * 34.0 + 45.0, 100.0, 3));
        paths.extend(word(110.0, 170.0, 5));
        paths.extend(word(700.0, 100.0, 3));
        let s = segment(&strokes(&svg(paths)));
        assert_eq!(s.notes.len(), 2);
        assert_eq!(s.notes[0].strokes.len(), 12);
        assert_eq!(s.notes[1].strokes.len(), 3);
    }

    #[test]
    fn keeps_a_line_with_very_wide_word_gaps_whole() {
        // "This is a much longer note": gaps of two letters around a lone
        // "a", as written on the Elipsa. A mark two letters before the line,
        // like a star, and one six letters after it stay apart.
        let mut paths = word(100.0, 100.0, 4);
        paths.extend(word(100.0 + 4.0 * 34.0 + 80.0, 100.0, 1));
        paths.extend(word(100.0 + 5.0 * 34.0 + 160.0, 100.0, 4));
        paths.extend(word(100.0 + 9.0 * 34.0 + 160.0 + 240.0, 100.0, 1));
        paths.extend(word(100.0 - 30.0 - 80.0, 100.0, 1));
        let s = segment(&strokes(&svg(paths)));
        let sizes: Vec<usize> = s.notes.iter().map(|n| n.strokes.len()).collect();
        assert_eq!(sizes, [9, 1, 1]);
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
