#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OverlongWordPolicy {
    KeepWhole,
    HardWrap { spare_columns: usize },
    ShortenMiddle,
}

pub(crate) struct WrappedText {
    pub(crate) lines: Vec<String>,
    pub(crate) shortened: bool,
}

/// Pack whitespace-separated words to a measured content width.
///
/// `HardWrap` uses `spare_columns` only when splitting a word wider than the
/// full width. `KeepWhole` may return lines wider than the budget; callers
/// choose it when preserving a value is more useful than fitting the row.
pub(crate) fn wrap_words(
    text: &str,
    width: usize,
    measure: impl Fn(&str) -> usize,
    policy: OverlongWordPolicy,
) -> WrappedText {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut shortened = false;

    for word in text.split_whitespace() {
        let word_width = measure(word);
        if word_width > width {
            match policy {
                OverlongWordPolicy::KeepWhole => {}
                OverlongWordPolicy::HardWrap { spare_columns } => {
                    if !current.is_empty() {
                        lines.push(std::mem::take(&mut current));
                    }
                    let hard_width = width.saturating_sub(spare_columns).max(1);
                    let mut chunks = split_word(word, hard_width, &measure).into_iter();
                    let count = chunks.len();
                    for _ in 1..count {
                        lines.push(chunks.next().expect("split word has enough chunks"));
                    }
                    current = chunks.next().expect("overlong word creates chunks");
                    continue;
                }
                OverlongWordPolicy::ShortenMiddle => {
                    shortened = true;
                    let word = shorten_middle(word, width);
                    append_word(&mut lines, &mut current, &word, width, &measure);
                    continue;
                }
            }
        }
        append_word(&mut lines, &mut current, word, width, &measure);
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    WrappedText { lines, shortened }
}

fn append_word(
    lines: &mut Vec<String>,
    current: &mut String,
    word: &str,
    width: usize,
    measure: &impl Fn(&str) -> usize,
) {
    if current.is_empty() {
        current.push_str(word);
    } else if measure(current) + 1 + measure(word) <= width {
        current.push(' ');
        current.push_str(word);
    } else {
        lines.push(std::mem::take(current));
        current.push_str(word);
    }
}

fn split_word(word: &str, width: usize, measure: &impl Fn(&str) -> usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for (index, ch) in word.char_indices() {
        let next_end = index + ch.len_utf8();
        if index > start && measure(&word[start..next_end]) > width {
            chunks.push(word[start..index].to_string());
            start = index;
        }
        end = next_end;
    }
    if start < end {
        chunks.push(word[start..end].to_string());
    }
    chunks
}

fn shorten_middle(word: &str, max: usize) -> String {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() <= max || max < 5 {
        return chars.into_iter().take(max).collect();
    }
    let keep = max - 3;
    let head = keep / 3;
    let tail = keep - head;
    let mut shortened: String = chars[..head].iter().collect();
    shortened.push_str("...");
    shortened.extend(&chars[chars.len() - tail..]);
    shortened
}

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
