//! Dictionaries to get: the free Wiktionary dictionaries reader.dict
//! compiles for each language (CC BY-SA 4.0), in the DictFile format Kollate
//! builds from. Like handwriting models, they're downloaded in the browser
//! and added with +; Kollate itself never goes online. reader.dict updates
//! the files in place, so they're listed by language and address, and the
//! sizes are approximate.

/// A language reader.dict publishes a dictionary for, and its size in MB
/// (compressed; checked September 2026).
pub const READER_DICT: &[(&str, f32)] = &[
    ("ca", 3.0),
    ("da", 0.5),
    ("de", 13.4),
    ("el", 6.9),
    ("en", 44.1),
    ("eo", 1.7),
    ("es", 8.2),
    ("fr", 28.3),
    ("it", 5.7),
    ("ja", 4.8),
    ("lt", 2.1),
    ("no", 0.7),
    ("pt", 5.2),
    ("ro", 4.8),
    ("ru", 29.7),
    ("sv", 4.8),
    ("zh", 2.6),
];

/// reader.dict's site, and where it asks for donations (linked from its
/// download pages).
pub const READER_DICT_URL: &str = "https://www.reader-dict.com/";
pub const READER_DICT_DONATE_URL: &str = "https://donate.stripe.com/9B600j2cheU905LdaE2cg01";

/// Where to download the dictionary for `language` (words defined in the
/// same language).
pub fn download_url(language: &str) -> String {
    format!("https://www.reader-dict.com/file/{language}/dict-{language}-{language}.df.bz2")
}

/// The approximate size of `language`'s dictionary, if reader.dict has one.
pub fn size_mb(language: &str) -> Option<f32> {
    READER_DICT
        .iter()
        .find(|(l, _)| *l == language)
        .map(|(_, mb)| *mb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_languages_with_addresses() {
        assert_eq!(
            download_url("es"),
            "https://www.reader-dict.com/file/es/dict-es-es.df.bz2"
        );
        assert_eq!(size_mb("es"), Some(8.2));
        assert_eq!(size_mb("xx"), None);
        assert!(READER_DICT.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
