//! A small rule-based English lemmatizer. It only proposes candidates; the
//! dictionary decides which one exists.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lookup key: lowercase with accents removed (`Séances` → `seances`).
pub fn fold(word: &str) -> String {
    word.trim()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

const RULES: &[(&str, &str)] = &[
    ("ies", "y"),
    ("ves", "f"),
    ("ves", "fe"),
    ("ses", "s"),
    ("xes", "x"),
    ("zes", "z"),
    ("ches", "ch"),
    ("shes", "sh"),
    ("men", "man"),
    ("es", "e"),
    ("s", ""),
    ("ied", "y"),
    ("ed", ""),
    ("ed", "e"),
    ("ying", "ie"),
    ("ing", ""),
    ("ing", "e"),
    ("ier", "y"),
    ("iest", "y"),
    ("er", ""),
    ("est", ""),
    ("ly", ""),
];

/// Candidate headwords for `word`, most likely first: the word itself,
/// then suffix-stripped forms (including undoubled consonants: `stopped` →
/// `stop`).
pub fn candidates(word: &str) -> Vec<String> {
    let w = fold(word);
    let mut out = vec![w.clone()];
    let mut push = |c: String| {
        if c.chars().count() >= 2 && !out.contains(&c) {
            out.push(c);
        }
    };
    for (suffix, replacement) in RULES {
        if let Some(stem) = w.strip_suffix(suffix) {
            push(format!("{stem}{replacement}"));
            // stopped → stopp → stop
            if replacement.is_empty() && ["ed", "ing", "er", "est"].contains(suffix) {
                let mut chars = stem.chars().rev();
                if let (Some(a), Some(b)) = (chars.next(), chars.next())
                    && a == b
                    && !"aeiou".contains(a)
                {
                    push(stem[..stem.len() - a.len_utf8()].to_owned());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_accents() {
        assert_eq!(fold(" Séances "), "seances");
        assert_eq!(fold("Demiurge"), "demiurge");
    }

    #[test]
    fn proposes_lemmas() {
        let has = |w: &str, lemma: &str| {
            assert!(
                candidates(w).contains(&lemma.to_owned()),
                "{w} → {lemma}: {:?}",
                candidates(w)
            )
        };
        has("theophanies", "theophany");
        has("daimons", "daimon");
        has("séances", "seance");
        has("debouched", "debouch");
        has("stopped", "stop");
        has("glasses", "glass");
        has("wolves", "wolf");
        has("making", "make");
        assert_eq!(candidates("Mammon")[0], "mammon");
    }
}
