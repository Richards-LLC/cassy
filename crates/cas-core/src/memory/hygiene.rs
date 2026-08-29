//! Stored-memory hygiene checks.

use cas_types::Entry;

/// High-importance preferences are standing instructions, not compact hints.
pub const HIGH_IMPORTANCE_PREFERENCE_THRESHOLD: f32 = 0.9;

/// Maximum number of characters retained from an important preference's first
/// line when it is rendered in the SessionStart summary.
pub const HIGH_IMPORTANCE_PREFERENCE_MAX_CHARS: usize = 300;

/// Tool-call artifacts worth reporting when found in stored memory content.
pub const TOOL_CALL_ARTIFACT_PATTERNS: [&str; 11] = [
    "<invoke",
    "</invoke>",
    "<tool_call",
    "</tool_call>",
    "<tool_result",
    "</tool_result>",
    "<tool_use",
    "</tool_use>",
    "<parameter",
    "</parameter>",
    "</content>",
];

/// One stored entry containing one or more tool-call/XML artifact patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContaminatedEntry {
    pub id: String,
    pub patterns: Vec<&'static str>,
}

/// Whether an entry is a standing preference important enough to preserve in
/// full-line form during compact SessionStart rendering.
pub fn is_high_importance_preference(entry: &Entry) -> bool {
    entry.entry_type == cas_types::EntryType::Preference
        && entry.importance >= HIGH_IMPORTANCE_PREFERENCE_THRESHOLD
}

/// Render the SessionStart summary for an entry.
///
/// Ordinary memories retain the compact preview. High-importance preferences
/// use their operative first line, capped by characters so a malformed or
/// unusually large memory cannot consume the entire hook budget.
pub fn session_memory_preview(entry: &Entry) -> String {
    if !is_high_importance_preference(entry) {
        return entry.preview(60);
    }

    let first_line = entry
        .content
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    let mut chars = first_line.chars();
    let preview: String = chars
        .by_ref()
        .take(HIGH_IMPORTANCE_PREFERENCE_MAX_CHARS)
        .collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

/// Return artifact patterns found in a string.
pub fn contamination_patterns(content: &str) -> Vec<&'static str> {
    let lower = content.to_ascii_lowercase();
    TOOL_CALL_ARTIFACT_PATTERNS
        .iter()
        .copied()
        .filter(|pattern| lower.contains(&pattern.to_ascii_lowercase()))
        .collect()
}

/// Find entries whose title or content contains a tool-call/XML artifact.
pub fn find_contaminated_entries(entries: &[Entry]) -> Vec<ContaminatedEntry> {
    entries
        .iter()
        .filter_map(|entry| {
            let mut patterns = contamination_patterns(entry.title.as_deref().unwrap_or_default());
            for content in [
                &entry.content,
                entry.raw_content.as_deref().unwrap_or_default(),
            ] {
                for pattern in contamination_patterns(content) {
                    if !patterns.contains(&pattern) {
                        patterns.push(pattern);
                    }
                }
            }
            (!patterns.is_empty()).then(|| ContaminatedEntry {
                id: entry.id.clone(),
                patterns,
            })
        })
        .collect()
}
