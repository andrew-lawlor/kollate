//! Pen marks (SPEC §8c): triage written in the margin. A star (`*`) or `NB`
//! stars, a `?` tags a question, and `#word` tags. `*`, `NB` and `?` count
//! only as a line of their own, so "Is this true?" stays a note; `#word`
//! counts anywhere.

/// What a note asks for.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PenMarks {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub star: bool,
    /// Lower-cased tag names, in the order written (a `?` is `question`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl PenMarks {
    pub fn is_empty(&self) -> bool {
        !self.star && self.tags.is_empty()
    }

    fn tag(&mut self, name: String) {
        if !self.tags.contains(&name) {
            self.tags.push(name);
        }
    }
}

/// The tag a `?` gives.
pub const QUESTION: &str = "question";

/// Splits the marks out of a note: the note without them (`None` if
/// nothing else was written), and the marks.
pub fn take_marks(note: &str) -> (Option<String>, PenMarks) {
    let mut marks = PenMarks::default();
    let mut kept = Vec::new();
    for line in note.lines() {
        let squeezed: String = line
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '.')
            .collect::<String>()
            .to_lowercase();
        match squeezed.as_str() {
            "*" | "★" | "☆" | "✱" | "nb" => {
                marks.star = true;
                continue;
            }
            "?" => {
                marks.tag(QUESTION.to_owned());
                continue;
            }
            _ => {}
        }
        let mut words = Vec::new();
        for word in line.split_whitespace() {
            let tag = word
                .strip_prefix('#')
                .map(|t| t.trim_end_matches(|c: char| !c.is_alphanumeric()))
                // A tag starts with a letter: "see #1" isn't one.
                .filter(|t| {
                    t.starts_with(char::is_alphabetic)
                        && t.chars()
                            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                });
            match tag {
                Some(t) => marks.tag(t.to_lowercase()),
                None => words.push(word),
            }
        }
        if !words.is_empty() {
            kept.push(words.join(" "));
        }
    }
    ((!kept.is_empty()).then(|| kept.join("\n")), marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(star: bool, tags: &[&str]) -> PenMarks {
        PenMarks {
            star,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn takes_marks_out_of_notes() {
        assert_eq!(take_marks("*"), (None, marks(true, &[])));
        assert_eq!(take_marks("N B"), (None, marks(true, &[])));
        assert_eq!(take_marks("N.B."), (None, marks(true, &[])));
        assert_eq!(take_marks("?"), (None, marks(false, &["question"])));
        assert_eq!(take_marks("#justice"), (None, marks(false, &["justice"])));
        assert_eq!(
            take_marks("Every migration, ever. #Leadership\n*"),
            (
                Some("Every migration, ever.".into()),
                marks(true, &["leadership"])
            )
        );
        // A question in a note is a note; a lone # or a hashtag of symbols isn't a tag.
        assert_eq!(take_marks("why?"), (Some("why?".into()), marks(false, &[])));
        assert_eq!(
            take_marks("page # 4"),
            (Some("page # 4".into()), marks(false, &[]))
        );
        assert_eq!(
            take_marks("#based, #based"),
            (None, marks(false, &["based"]))
        );
        assert_eq!(
            take_marks("see #1"),
            (Some("see #1".into()), marks(false, &[]))
        );
        assert_eq!(take_marks("#c3po"), (None, marks(false, &["c3po"])));
    }
}
