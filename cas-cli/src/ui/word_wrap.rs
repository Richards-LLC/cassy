#[cfg(test)]
mod tests {
    use super::{OverlongWordPolicy, wrap_words};

    fn char_width(text: &str) -> usize {
        text.chars().count()
    }

    #[test]
    fn overlong_word_policies_keep_split_and_shorten() {
        let kept = wrap_words("longword x", 4, char_width, OverlongWordPolicy::KeepWhole);
        assert_eq!(kept.lines, ["longword", "x"]);
        assert!(!kept.shortened);

        let split = wrap_words(
            "longword",
            4,
            char_width,
            OverlongWordPolicy::HardWrap { spare_columns: 0 },
        );
        assert_eq!(split.lines, ["long", "word"]);

        let split_with_spare = wrap_words(
            "longword",
            5,
            char_width,
            OverlongWordPolicy::HardWrap { spare_columns: 1 },
        );
        assert_eq!(split_with_spare.lines, ["long", "word"]);

        let shortened = wrap_words(
            "abcdefgh/path",
            10,
            char_width,
            OverlongWordPolicy::ShortenMiddle,
        );
        assert_eq!(shortened.lines, ["ab.../path"]);
        assert!(shortened.shortened);
    }
}
