//! Search manifest schema for investigation-task close warnings (cas-49f1).
//!
//! # Motivation
//!
//! An investigation task (a "spike" — no code diff expected) closes on a
//! prose conclusion ("no defects found"), and nothing distinguishes "I
//! searched and found nothing" from "my search pattern was broken and
//! matched zero lines everywhere." cas-94a3 shipped exactly that: a single
//! `grep -o '"message":{"type":"human"...'` against JSONL that has no
//! `"type":"human"` key returned 0 hits across every input, and the worker
//! folded the silence into a clean-sweep conclusion.
//!
//! # Contract
//!
//! This is a voluntary, narrow surface — not a general "prove your
//! investigation" framework. A worker on a `Spike`-type task *may* attach a
//! `search_manifest` to `task.close`: a JSON array of the literal search
//! commands it ran and the hit count each one returned. When present, the
//! close gate (`cas-cli`'s `run_search_manifest_gate`) scans for any entry
//! reporting `hits == 0` and appends a loud, non-blocking warning note to
//! the task rather than letting the close proceed silently. Ordinary code
//! tasks are never touched — the field is optional and the gate only
//! consults it for `Spike` tasks.

use serde::{Deserialize, Serialize};

/// One executed search step and the hit count it returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchManifestEntry {
    /// The literal command run (e.g. a `grep`/`rg` invocation or a
    /// `codex exec` prompt describing the search performed).
    pub command: String,
    /// Total hit count the command returned across all inputs it was run
    /// against. `0` means the pattern matched nothing anywhere.
    pub hits: u64,
}

/// A worker-submitted search manifest: the ordered list of search steps
/// taken during an investigation task.
pub type SearchManifest = Vec<SearchManifestEntry>;

/// Parse a `search_manifest` JSON string into entries.
///
/// Returns `Err(message)` with a human-readable diagnostic on malformed
/// JSON so the close gate can surface it directly rather than a raw serde
/// error.
pub fn parse_search_manifest(raw: &str) -> Result<SearchManifest, String> {
    serde_json::from_str::<SearchManifest>(raw).map_err(|e| {
        format!(
            "search_manifest failed to parse as JSON: {e}\n\n{}",
            search_manifest_shape_hint()
        )
    })
}

/// Compact shape hint for `search_manifest` parse-failure / usage text.
pub fn search_manifest_shape_hint() -> String {
    "Expected shape: [{\"command\": string, \"hits\": number}, ...] — one entry \
     per search command actually run, with the total hit count it returned."
        .to_string()
}

/// Entries in `manifest` whose `hits == 0`.
///
/// A grep that matches nothing anywhere is far more often a broken pattern
/// than a clean corpus (cas-49f1) — this is the degenerate case the close
/// gate surfaces as a warning instead of folding into a silent "nothing
/// found" conclusion.
pub fn zero_hit_entries(manifest: &[SearchManifestEntry]) -> Vec<&SearchManifestEntry> {
    manifest.iter().filter(|e| e.hits == 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_manifest() {
        let raw = r#"[{"command": "grep -c foo file", "hits": 3}]"#;
        let parsed = parse_search_manifest(raw).expect("should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].hits, 3);
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_search_manifest("not json").unwrap_err();
        assert!(err.contains("failed to parse"));
        assert!(err.contains("Expected shape"));
    }

    #[test]
    fn finds_zero_hit_entries() {
        let manifest = vec![
            SearchManifestEntry {
                command: "grep -c '\"type\":\"human\"'".to_string(),
                hits: 0,
            },
            SearchManifestEntry {
                command: "grep -c '\"type\":\"user\"'".to_string(),
                hits: 207,
            },
        ];
        let zero = zero_hit_entries(&manifest);
        assert_eq!(zero.len(), 1);
        assert_eq!(zero[0].command, "grep -c '\"type\":\"human\"'");
    }

    #[test]
    fn no_zero_hit_entries_when_all_matched() {
        let manifest = vec![SearchManifestEntry {
            command: "grep -c foo file".to_string(),
            hits: 5,
        }];
        assert!(zero_hit_entries(&manifest).is_empty());
    }
}
