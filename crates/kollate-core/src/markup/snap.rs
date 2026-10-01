//! Replaces a model's reading of printed text with the book's own words.

fn norm(s: &str) -> String {
    s.to_lowercase()
        .replace(['’', '‘'], "'")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 1 for identical strings, falling towards 0 with each edit.
pub(crate) fn similarity(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    1.0 - row[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// The run of `words` (the book's text near the markup) that best matches
/// `reading`, or `None` when nothing is close. Clipped words at the edge of
/// a crop and typography the model flattened (’ → ') both come out right.
pub fn snap(reading: &str, words: &[String]) -> Option<String> {
    let target = norm(reading);
    let n = target.split(' ').count();
    if target.is_empty() || words.is_empty() {
        return None;
    }
    let mut best: Option<(f32, &[String])> = None;
    for size in n.saturating_sub(3).max(1)..=n + 3 {
        for window in words.windows(size.min(words.len())) {
            let score = similarity(&target, &norm(&window.join(" ")));
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, window));
            }
        }
    }
    let (score, window) = best?;
    (score >= 0.7).then(|| tidy(&window.join(" ")))
}

/// Corrects misread names in handwriting against the names printed nearby:
/// "Who is Pokmarchus?" beside a page that mentions Polemarchus becomes "Who
/// is Polemarchus?". Only words `known` doesn't recognise (not in any
/// dictionary) are corrected, only to names, both of five letters or more and
/// close: "Athena" stays "Athena" even beside "Athens". A name is a word the
/// book capitalises mid-sentence, or anywhere if it's not a dictionary word
/// (so "Polemarchus" opening a paragraph counts, and "Moderns" doesn't).
pub fn correct_names(note: &str, words: &[String], known: &dyn Fn(&str) -> bool) -> String {
    let mut names: Vec<&str> = Vec::new();
    for pair in words.windows(2) {
        let (before, word) = (
            &pair[0],
            pair[1].trim_matches(|c: char| !c.is_alphanumeric()),
        );
        let mid_sentence = !before.ends_with(['.', '!', '?', ':', '“', '"']);
        if (mid_sentence || !known(&word.to_lowercase()))
            && word.chars().count() >= 5
            && word.chars().next().is_some_and(char::is_uppercase)
            && word.chars().all(char::is_alphabetic)
            && !names.contains(&word)
        {
            names.push(word);
        }
    }
    note.split(' ')
        .map(|token| {
            let start = token.find(char::is_alphanumeric).unwrap_or(token.len());
            let end = token
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_alphanumeric())
                .map_or(start, |(i, c)| i + c.len_utf8());
            let word = &token[start..end];
            if word.chars().count() < 5 || names.contains(&word) || known(&word.to_lowercase()) {
                return token.to_owned();
            }
            let best = names
                .iter()
                .map(|n| (similarity(&word.to_lowercase(), &n.to_lowercase()), n))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            match best {
                Some((score, name)) if score >= 0.75 => {
                    format!("{}{name}{}", &token[..start], &token[end..])
                }
                _ => token.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drops the comma or dash a mark stops short of.
pub fn tidy(text: &str) -> String {
    text.trim()
        .trim_end_matches([',', ';', ':', '—', '–', '-'])
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn snaps_to_the_books_words() {
        let book = words(
            "at his feet gave mute testimony that the fiend could act with terrible material \
             effect. Kane’s thoughts were grim.",
        );
        assert_eq!(
            snap(
                "e testimony that the fiend could act with terrible material effect.",
                &book
            )
            .as_deref(),
            Some("testimony that the fiend could act with terrible material effect.")
        );
        assert_eq!(
            snap("Kane's thoughts", &book).as_deref(),
            Some("Kane’s thoughts")
        );
        assert_eq!(snap("something else entirely", &book), None);
        assert_eq!(snap("", &book), None);
    }

    #[test]
    fn corrects_misread_names_only() {
        let book = words(
            "procession. Polemarchus said: It looks to me, Socrates, as if you two are starting \
             off for Athens. Moderns would say otherwise. Glaucon agreed.",
        );
        let dictionary = |w: &str| ["athena", "moderns", "socrates", "again"].contains(&w);
        let fix = |note| correct_names(note, &book, &dictionary);
        assert_eq!(fix("who is Pokmarchus?"), "who is Polemarchus?");
        assert_eq!(fix("Socrates, again"), "Socrates, again");
        // A real word stays, even beside a similar name.
        assert_eq!(fix("Athena, not Athenz"), "Athena, not Athens");
        // "Moderns" starts a sentence in the book, so it isn't a name.
        assert_eq!(fix("Jarring for Modernz"), "Jarring for Modernz");
        assert_eq!(correct_names("Pokmarchus", &[], &dictionary), "Pokmarchus");
        // Letters beyond ASCII, at either end of a word.
        assert_eq!(fix("¡Qué café, Pokmarchus!"), "¡Qué café, Polemarchus!");
        assert_eq!(fix("Pokmarchö"), "Pokmarchö");
        assert_eq!(fix("米 ñandú"), "米 ñandú");
    }

    #[test]
    fn tidies_trailing_punctuation() {
        assert_eq!(
            tidy("pale blood over the moor, "),
            "pale blood over the moor"
        );
        assert_eq!(tidy("the end."), "the end.");
    }

    #[test]
    fn measures_similarity() {
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert!((similarity("kitten", "sitting") - (1.0 - 3.0 / 7.0)).abs() < 1e-6);
    }
}
