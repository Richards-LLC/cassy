//! UTF-8 safe preview truncation.
//!
//! `&s[..N]` panics if `N` lands inside a multi-byte character. This module
//! provides a single `truncate_preview` helper used by every `*::preview()`
//! method on `Entry`, `Task`, `Rule`, `Skill`, `Spec`, `Prompt`, and
//! `CommitLink`. The pre-2026-04-16 implementations open-coded byte slicing
//! and crashed the MCP server when any search result surfaced an entry
//! containing `→`, em-dash, emoji, or any other non-ASCII character near the
//! preview length.

/// Truncate `s` to at most `max_len` bytes, appending `"..."` when truncated,
/// always respecting UTF-8 character boundaries so the result is a valid
/// string that can safely be formatted or displayed.
///
/// Semantics:
/// - If `s.len() <= max_len`, returns the original string unchanged.
/// - Otherwise, returns `{prefix}...` where `prefix` is the largest
///   char-boundary-aligned prefix of `s` with byte length
///   `<= max_len.saturating_sub(3)`.
/// - A `max_len` smaller than 3 produces just `"..."` (no room for content).
pub fn truncate_preview(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let target = max_len.saturating_sub(3);
    let mut end = target.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_shorter_than_max_is_unchanged() {
        assert_eq!(truncate_preview("hello", 10), "hello");
    }

    #[test]
    fn regression_cas_serve_search_crash_2026_04_16() {
        // Exact prefix that crashed `cas serve` when returned by `search`.
        // Byte index 57 was inside `→` (bytes 55..58) with max_len = 60.
        let s = "CAS factory daemon boot order: `build_configs_for_mux` → `FactoryApp::new` (spawns PTYs)";
        let _ = truncate_preview(s, 60); // must not panic
    }

    #[test]
    fn emoji_prefix_does_not_panic() {
        // 4-byte emoji.
        let s = "hello 🎉 world";
        let _ = truncate_preview(s, 8);
        let _ = truncate_preview(s, 9);
        let _ = truncate_preview(s, 10);
    }

}
