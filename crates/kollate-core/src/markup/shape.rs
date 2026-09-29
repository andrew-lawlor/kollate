//! Shapes in the ink that mean something without being read: a loop drawn
//! around a word, a drawn star and a question mark. Tuned on samples from a
//! Libra Colour (SPEC §8c); the model misreads all three (a small loop as
//! "123456789", a star as "5", a question mark as "3").

use super::image::render_note;
use super::segment::{Bounds, Note, Rotation, Stroke};
use crate::Result;

/// The share of a stroke's box that it encloses: about 0.5–0.8 for a loop,
/// near 0 for an open stroke. Gaps of a few pixels, where the pen didn't
/// quite meet its start, are closed first.
pub fn enclosed(svg: &str, strokes: &[Stroke], i: usize) -> Result<f32> {
    const GAP: i32 = 4;
    let note = Note {
        strokes: vec![i],
        bounds: strokes[i].bounds,
        rotation: Rotation::None,
    };
    let img = render_note(svg, strokes, &note, GAP as u32 + 2)?;
    let (w, h) = (img.width as i32, img.height as i32);
    let ink: Vec<bool> = img.data.chunks(3).map(|p| p[0] < 128).collect();
    // Thicken the ink, so a small gap doesn't let the outside in.
    let mut thick = ink.clone();
    for y in 0..h {
        for x in 0..w {
            if ink[(y * w + x) as usize] {
                for dy in -GAP..=GAP {
                    for dx in -GAP..=GAP {
                        let (nx, ny) = (x + dx, y + dy);
                        if (0..w).contains(&nx)
                            && (0..h).contains(&ny)
                            && dx * dx + dy * dy <= GAP * GAP
                        {
                            thick[(ny * w + nx) as usize] = true;
                        }
                    }
                }
            }
        }
    }
    // Fill the outside from the edges; what's left unfilled and not ink is enclosed.
    let mut outside = vec![false; thick.len()];
    let mut stack: Vec<(i32, i32)> = Vec::new();
    for x in 0..w {
        stack.extend([(x, 0), (x, h - 1)]);
    }
    for y in 0..h {
        stack.extend([(0, y), (w - 1, y)]);
    }
    while let Some((x, y)) = stack.pop() {
        if !(0..w).contains(&x) || !(0..h).contains(&y) {
            continue;
        }
        let k = (y * w + x) as usize;
        if outside[k] || thick[k] {
            continue;
        }
        outside[k] = true;
        stack.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
    }
    let inside = (0..thick.len())
        .filter(|&k| !outside[k] && !thick[k])
        .count();
    let b = strokes[i].bounds;
    Ok(inside as f32 / (b.width() * b.height()).max(1.0))
}

/// Strokes of a note that aren't specks (the pen touching down by accident).
fn solid(strokes: &[Stroke], note: &Note) -> Vec<usize> {
    note.strokes
        .iter()
        .copied()
        .filter(|&i| strokes[i].bounds.width().max(strokes[i].bounds.height()) >= 8.0)
        .collect()
}

/// A loop drawn around a word or two: one stroke (and perhaps a speck),
/// clearly wider than tall, that encloses space. (Loops around longer
/// passages are circles by size already; see [`segment`](super::segment::segment).)
pub fn is_word_loop(svg: &str, strokes: &[Stroke], note: &Note) -> Result<bool> {
    let [i] = solid(strokes, note)[..] else {
        return Ok(false);
    };
    let b = strokes[i].bounds;
    if b.width() < 30.0 || b.width() < 1.5 * b.height() {
        return Ok(false);
    }
    Ok(enclosed(svg, strokes, i)? > 0.3)
}

/// A star drawn in one to three strokes: its outline, seen from its middle,
/// reaches out five times.
pub fn is_star(strokes: &[Stroke], note: &Note) -> bool {
    let ids = solid(strokes, note);
    if ids.is_empty() || ids.len() > 3 {
        return false;
    }
    let Some(b) = Bounds::union(ids.iter().map(|&i| strokes[i].bounds)) else {
        return false;
    };
    let points = ids.iter().flat_map(|&i| strokes[i].points.iter().copied());
    let aspect = b.width() / b.height().max(1.0);
    if !(0.6..=1.7).contains(&aspect) || b.width().max(b.height()) < 20.0 {
        return false;
    }
    let (cx, cy) = b.centre();
    // The farthest point in each 5° direction, then the runs of far ones.
    const BINS: usize = 72;
    let mut reach = [0.0f32; BINS];
    for (x, y) in points {
        let (dx, dy) = ((x - cx) / b.width(), (y - cy) / b.height());
        let angle = dy.atan2(dx).rem_euclid(std::f32::consts::TAU);
        let bin = ((angle / std::f32::consts::TAU) * BINS as f32) as usize % BINS;
        reach[bin] = reach[bin].max(dx.hypot(dy));
    }
    // Directions with no points (a gap in the outline) take their neighbours'.
    for k in 0..BINS {
        if reach[k] == 0.0 {
            reach[k] = reach[(k + BINS - 1) % BINS].max(reach[(k + 1) % BINS]);
        }
    }
    let max = reach.iter().copied().fold(0.0, f32::max);
    if max == 0.0 {
        return false;
    }
    let far: Vec<bool> = reach.iter().map(|r| r / max > 0.78).collect();
    let near = reach.iter().filter(|r| **r / max < 0.6).count();
    let points = (0..BINS)
        .filter(|&k| far[k] && !far[(k + BINS - 1) % BINS])
        .count();
    points == 5 && near >= 10
}

/// A question mark: a hook, with a dot below it.
pub fn is_question(strokes: &[Stroke], note: &Note) -> bool {
    let [a, b] = note.strokes[..] else {
        return false;
    };
    let size = |r: &Bounds| r.width().max(r.height());
    let (hook, dot) = if size(&strokes[a].bounds) >= size(&strokes[b].bounds) {
        (strokes[a].bounds, strokes[b].bounds)
    } else {
        (strokes[b].bounds, strokes[a].bounds)
    };
    let (dx, _) = dot.centre();
    hook.height() >= 20.0
        && (0.25..=1.3).contains(&(hook.width() / hook.height()))
        && size(&dot) < 0.55 * hook.height()
        && dot.top >= hook.bottom - 0.15 * hook.height()
        && dot.top <= hook.bottom + 0.6 * hook.height()
        && dx >= hook.left - 0.3 * hook.width()
        && dx <= hook.right + 0.3 * hook.width()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::segment::strokes;

    /// An outline around a polyline, as the Kobo draws a stroke.
    fn pen(points: &[(f32, f32)]) -> String {
        let half = 2.0;
        let mut left = Vec::new();
        let mut right = Vec::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            let a = points[i.saturating_sub(1)];
            let b = points[(i + 1).min(points.len() - 1)];
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = dx.hypot(dy).max(0.001);
            left.push((x - dy / l * half, y + dx / l * half));
            right.push((x + dy / l * half, y - dx / l * half));
        }
        let ring: Vec<String> = left
            .into_iter()
            .chain(right.into_iter().rev())
            .map(|(x, y)| format!("{x},{y}"))
            .collect();
        format!("<path d=\"M{} Z\"/>", ring.join(" L"))
    }

    fn svg(paths: &[String]) -> String {
        format!(
            "<svg width=\"400\" height=\"400\" viewBox=\"0 0 400 400\"><g>{}</g></svg>",
            paths.concat()
        )
    }

    fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, turn: f32) -> Vec<(f32, f32)> {
        (0..=90)
            .map(|k| {
                let t = turn * std::f32::consts::TAU * k as f32 / 90.0;
                (cx + rx * t.cos(), cy + ry * t.sin())
            })
            .collect()
    }

    fn star(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
        // An outlined star in one go: out to a point, in, out to the next.
        let corner = |k: usize| {
            let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
            let r = if k.is_multiple_of(2) { r } else { 0.4 * r };
            (cx + r * a.cos(), cy + r * a.sin())
        };
        (0..10)
            .flat_map(|k| {
                let (a, b) = (corner(k), corner(k + 1));
                (0..8).map(move |t| {
                    (
                        a.0 + (b.0 - a.0) * t as f32 / 8.0,
                        a.1 + (b.1 - a.1) * t as f32 / 8.0,
                    )
                })
            })
            .chain([corner(0)])
            .collect()
    }

    fn note(strokes: &[Stroke], ids: &[usize]) -> Note {
        Note {
            strokes: ids.to_vec(),
            bounds: Bounds::union(ids.iter().map(|&i| strokes[i].bounds)).unwrap(),
            rotation: Rotation::None,
        }
    }

    #[test]
    fn tells_a_loop_around_a_word_from_other_strokes() {
        // A closed loop, one nearly closed, a hook, and a round "o".
        let paths = [
            pen(&ellipse(100.0, 50.0, 45.0, 14.0, 1.0)),
            pen(&ellipse(100.0, 150.0, 45.0, 14.0, 0.95)),
            pen(&[(40.0, 250.0), (60.0, 230.0), (80.0, 250.0), (60.0, 280.0)]),
            pen(&ellipse(300.0, 50.0, 12.0, 12.0, 1.0)),
        ];
        let doc = svg(&paths);
        let s = strokes(&doc);
        let loops: Vec<bool> = (0..4)
            .map(|i| is_word_loop(&doc, &s, &note(&s, &[i])).unwrap())
            .collect();
        assert_eq!(loops, [true, true, false, false]);
    }

    #[test]
    fn knows_a_star_and_a_question_mark() {
        let hook: Vec<(f32, f32)> = ellipse(200.0, 60.0, 15.0, 15.0, 0.6)
            .into_iter()
            .chain([(200.0, 90.0), (200.0, 100.0)])
            .collect();
        let paths = [
            pen(&star(100.0, 100.0, 30.0)),
            pen(&ellipse(100.0, 250.0, 30.0, 30.0, 1.0)),
            pen(&hook),
            pen(&[(200.0, 112.0), (201.0, 113.0)]),
        ];
        let s = strokes(&svg(&paths));
        assert!(is_star(&s, &note(&s, &[0])));
        assert!(!is_star(&s, &note(&s, &[1])));
        assert!(is_question(&s, &note(&s, &[2, 3])));
        assert!(!is_question(&s, &note(&s, &[0, 3])));
    }
}

#[cfg(test)]
mod real {
    use super::*;
    use crate::markup::segment::{segment, strokes};

    /// Classifies the notes in real markup SVGs from `$KOLLATE_MARKUPS_DIR`.
    #[test]
    fn classifies_real_markups() {
        let Some(dir) = std::env::var_os("KOLLATE_MARKUPS_DIR") else {
            return;
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "svg"))
            .collect();
        files.sort();
        for f in files {
            let doc = std::fs::read_to_string(&f).unwrap();
            let s = strokes(&doc);
            let seg = segment(&s);
            let kinds: Vec<String> = seg
                .notes
                .iter()
                .map(|n| {
                    if is_word_loop(&doc, &s, n).unwrap() {
                        "loop".into()
                    } else if is_star(&s, n) {
                        "star".into()
                    } else if is_question(&s, n) {
                        "question".into()
                    } else {
                        format!("text({})", n.strokes.len())
                    }
                })
                .collect();
            eprintln!(
                "{}: marks {:?} notes {:?}",
                f.file_name().unwrap().to_string_lossy(),
                seg.marks.iter().map(|m| m.kind).collect::<Vec<_>>(),
                kinds
            );
        }
    }
}
