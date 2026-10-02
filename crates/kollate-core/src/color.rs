//! Highlight colours, by name (SPEC §8e): the Kobo's four, then the others
//! KOReader offers.

/// Every highlight colour Kollate knows, in a fixed order: Kobo's four
/// first. Anything else a reader records is kept as it is and shown grey.
pub const HIGHLIGHT_COLORS: [&str; 10] = [
    "yellow", "pink", "blue", "green", "red", "orange", "olive", "cyan", "purple", "gray",
];

/// The colour every highlight has unless the reader says otherwise, and
/// that exports leave unsaid.
pub const DEFAULT_COLOR: &str = "yellow";

/// A Kobo `Bookmark.Color` index as a name (verified on a Libra Colour).
pub fn kobo_color(index: i64) -> &'static str {
    match index {
        1 => "pink",
        2 => "blue",
        3 => "green",
        _ => DEFAULT_COLOR,
    }
}

/// Where `name` sits in [`HIGHLIGHT_COLORS`], if it's one Kollate knows.
pub fn color_index(name: &str) -> Option<usize> {
    HIGHLIGHT_COLORS.iter().position(|c| *c == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_kobo_colours() {
        let names: Vec<_> = (0..4).map(kobo_color).collect();
        assert_eq!(names, ["yellow", "pink", "blue", "green"]);
        assert_eq!(kobo_color(9), "yellow");
        assert_eq!(color_index("olive"), Some(6));
        assert_eq!(color_index("teal"), None);
    }
}
