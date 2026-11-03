//! Intra-line diff computation for highlighting changes within a line.

use similar::{ChangeTag, TextDiff};

use crate::LineDiffType;

/// A span of text within a line, indicating whether it should be highlighted
/// as a change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InlineSpan {
    /// The text content of this span.
    pub text: String,
    /// Whether this span represents changed text (should be highlighted).
    pub highlighted: bool,
}

/// Compute inline (intra-line) diffs between an old and new line.
///
/// Returns `(deletion_spans, addition_spans)` — the old line split into
/// spans showing what was removed, and the new line split into spans
/// showing what was added.
///
/// # Modes
/// - [`LineDiffType::None`]: Returns empty vectors (no inline diffing).
/// - [`LineDiffType::Char`]: Character-level granularity.
/// - [`LineDiffType::Word`]: Word-level granularity.
/// - [`LineDiffType::WordAlt`]: Word-level with span joining — single-character
///   neutral gaps between highlighted spans are merged into the highlighted span
///   to reduce visual noise.
pub fn compute_inline_diff(
    old_line: &str,
    new_line: &str,
    mode: LineDiffType,
) -> (Vec<InlineSpan>, Vec<InlineSpan>) {
    if mode == LineDiffType::None {
        return (vec![], vec![]);
    }

    // Strip trailing newline, matching TS cleanLastNewline behavior.
    let old_clean = old_line.strip_suffix('\n').unwrap_or(old_line);
    let new_clean = new_line.strip_suffix('\n').unwrap_or(new_line);

    let diff = match mode {
        LineDiffType::Char => TextDiff::from_chars(old_clean, new_clean),
        LineDiffType::Word | LineDiffType::WordAlt => TextDiff::from_words(old_clean, new_clean),
        LineDiffType::None => unreachable!(),
    };

    let enable_join = mode == LineDiffType::WordAlt;
    let mut deletion_spans: Vec<InlineSpan> = Vec::new();
    let mut addition_spans: Vec<InlineSpan> = Vec::new();

    let changes: Vec<_> = diff.iter_all_changes().collect();
    let last_idx = changes.len().saturating_sub(1);

    for (i, change) in changes.iter().enumerate() {
        let is_last = i == last_idx;
        let value = change.value();

        match change.tag() {
            ChangeTag::Equal => {
                push_or_join_span(&mut deletion_spans, value, false, enable_join, is_last);
                push_or_join_span(&mut addition_spans, value, false, enable_join, is_last);
            }
            ChangeTag::Delete => {
                push_or_join_span(&mut deletion_spans, value, true, enable_join, is_last);
            }
            ChangeTag::Insert => {
                push_or_join_span(&mut addition_spans, value, true, enable_join, is_last);
            }
        }
    }

    (deletion_spans, addition_spans)
}

/// Port of `pushOrJoinSpan` from parseDiffDecorations.ts.
///
/// Consecutive same-type spans are always merged. Additionally, when
/// `enable_join` is true (WordAlt mode), a single-character neutral span
/// following a highlighted span is absorbed into the highlighted span,
/// preventing distracting single-space gaps in word diffs. This cross-type
/// join is skipped for the last item to avoid extending highlights to the
/// end of the line.
fn push_or_join_span(
    spans: &mut Vec<InlineSpan>,
    value: &str,
    highlighted: bool,
    enable_join: bool,
    is_last: bool,
) {
    if let Some(last) = spans.last_mut() {
        // Always merge consecutive same-type spans.
        if highlighted == last.highlighted {
            last.text.push_str(value);
            return;
        }
        // WordAlt: absorb single-char neutral gap into preceding highlighted span.
        // Skip on last item to avoid extending highlights to end of line.
        if enable_join && !is_last && !highlighted && value.len() == 1 && last.highlighted {
            last.text.push_str(value);
            return;
        }
    }

    spans.push(InlineSpan {
        text: value.to_string(),
        highlighted,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, highlighted: bool) -> InlineSpan {
        InlineSpan {
            text: text.to_string(),
            highlighted,
        }
    }

    #[test]
    fn none_mode_returns_empty() {
        let (del, add) = compute_inline_diff("hello", "world", LineDiffType::None);
        assert!(del.is_empty());
        assert!(add.is_empty());
    }

    #[test]
    fn word_mode_highlights_insertion() {
        let (del, add) =
            compute_inline_diff("hello world", "hello brave world", LineDiffType::Word);
        // Deletion side: entire line is unchanged
        assert_eq!(del, vec![span("hello world", false)]);
        // Addition side: "brave " is the new part
        assert_eq!(
            add,
            vec![
                span("hello ", false),
                span("brave ", true),
                span("world", false),
            ]
        );
    }

    #[test]
    fn char_mode_highlights_individual_chars() {
        let (del, add) = compute_inline_diff("cat", "car", LineDiffType::Char);
        // Deletion: "ca" unchanged, "t" removed
        assert_eq!(del, vec![span("ca", false), span("t", true)]);
        // Addition: "ca" unchanged, "r" added
        assert_eq!(add, vec![span("ca", false), span("r", true)]);
    }

    #[test]
    fn char_vs_word_different_granularity() {
        let old = "foo bar";
        let new = "foo baz";

        let (del_char, add_char) = compute_inline_diff(old, new, LineDiffType::Char);
        let (del_word, add_word) = compute_inline_diff(old, new, LineDiffType::Word);

        // Char mode: "foo ba" equal, "r" vs "z" differ
        assert_eq!(del_char, vec![span("foo ba", false), span("r", true)]);
        assert_eq!(add_char, vec![span("foo ba", false), span("z", true)]);

        // Word mode: "foo " equal, "bar" vs "baz" differ (whole words)
        assert_eq!(del_word, vec![span("foo ", false), span("bar", true)]);
        assert_eq!(add_word, vec![span("foo ", false), span("baz", true)]);
    }

    #[test]
    fn word_alt_joins_single_char_gaps() {
        // When two highlighted regions are separated by a single space,
        // WordAlt merges them into one highlighted region.
        // Compare Word vs WordAlt on "x y" vs "a b":
        let (del_word, _) = compute_inline_diff("x y", "a b", LineDiffType::Word);
        let (del_alt, _) = compute_inline_diff("x y", "a b", LineDiffType::WordAlt);

        // Word mode keeps the space as a separate neutral span
        assert_eq!(
            del_word,
            vec![span("x", true), span(" ", false), span("y", true)]
        );
        // WordAlt joins the single-space gap into the highlighted span
        assert_eq!(del_alt, vec![span("x y", true)]);
    }

    #[test]
    fn word_alt_preserves_longer_neutral_gaps() {
        // Neutral gaps longer than 1 character should NOT be joined.
        // "abc  xyz" vs "ABC  XYZ" — double-space gap between changed words.
        let (del, _) = compute_inline_diff("abc  xyz", "ABC  XYZ", LineDiffType::WordAlt);
        // The "  " (2 chars) neutral gap should remain separate
        assert_eq!(
            del,
            vec![span("abc", true), span("  ", false), span("xyz", true)]
        );
    }

    #[test]
    fn strips_trailing_newline() {
        let (del, add) = compute_inline_diff("hello\n", "world\n", LineDiffType::Word);
        // Should diff "hello" vs "world", not include the newline
        assert_eq!(del, vec![span("hello", true)]);
        assert_eq!(add, vec![span("world", true)]);
    }

    #[test]
    fn identical_lines() {
        let (del, add) = compute_inline_diff("same text", "same text", LineDiffType::Word);
        assert_eq!(del, vec![span("same text", false)]);
        assert_eq!(add, vec![span("same text", false)]);
    }

    #[test]
    fn empty_lines() {
        let (del, add) = compute_inline_diff("", "", LineDiffType::Char);
        assert!(del.is_empty());
        assert!(add.is_empty());
    }

    #[test]
    fn one_empty_one_not() {
        let (del, add) = compute_inline_diff("", "new text", LineDiffType::Word);
        assert!(del.is_empty());
        assert_eq!(add, vec![span("new text", true)]);
    }


}
