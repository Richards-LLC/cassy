//! Stored-memory hygiene checks.

use cas_types::Entry;

/// High-importance preferences are standing instructions, not compact hints.
pub const HIGH_IMPORTANCE_PREFERENCE_THRESHOLD: f32 = 0.9;

/// Tool-call artifacts worth reporting when found in stored memory content.
pub const TOOL_CALL_ARTIFACT_PATTERNS: [&str; 8] = [
    "<invoke",
    "</invoke>",
    "<tool_call",
    "</tool_call>",
    "<tool_result",
    "</tool_result>",
    "<parameter",
    "</parameter>",
];

/// One stored entry containing one or more tool-call/XML artifact patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContaminatedEntry {
    pub id: String,
    pub patterns: Vec<&'static str>,
}

/// Return artifact patterns found in a string.
pub fn contamination_patterns(_content: &str) -> Vec<&'static str> {
    Vec::new()
}

/// Find entries whose title or content contains a tool-call/XML artifact.
pub fn find_contaminated_entries(_entries: &[Entry]) -> Vec<ContaminatedEntry> {
    Vec::new()
}
