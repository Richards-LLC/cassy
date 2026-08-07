//! The zero-loss carrier: reserved `cas_legacy_*` frontmatter (spec §2).
//!
//! `knowledge_pages` has no column for importance, tier, feedback counts, tags
//! or any of the other 20-odd legacy fields (§1), so `migrate-to-page` is not
//! lossless on its own. `cas-cli/src/knowledge/merge.rs` owns exactly five
//! frontmatter keys (`OWNED_KEYS`, `merge.rs:92`) and carries every other key
//! through a distillation round trip verbatim — that passthrough is the
//! carrier, and the `cas_legacy_` prefix cannot collide with an owned key, so
//! the state is immune to re-distillation by construction (Rule C1).
//!
//! Three rules are load-bearing here and each has a test:
//!
//! - **C2** — the reserved key set is complete; M3 emits no other keys.
//! - **C3** — omit-when-default; absence decodes via [`DEFAULTS`], which the
//!   ledger records so the omission is readable without this file.
//! - **C4** — `parse_frontmatter` is a hand-rolled line reader, not a YAML
//!   engine (`merge.rs:117-120`). Every key is a flat scalar on one line;
//!   `cas_legacy_tags` is the single permitted list block. No nested maps, no
//!   multi-line strings, no anchors.

use super::DbLabel;
use super::source::LegacyRow;

/// Rule C2 — the complete reserved key set.
pub const RESERVED_KEYS: [&str; 27] = [
    "cas_legacy_id",
    "cas_legacy_db",
    "cas_legacy_scope",
    "cas_legacy_type",
    "cas_legacy_observation_type",
    "cas_legacy_belief_type",
    "cas_legacy_confidence",
    "cas_legacy_memory_tier",
    "cas_legacy_archived",
    "cas_legacy_importance",
    "cas_legacy_stability",
    "cas_legacy_access_count",
    "cas_legacy_last_accessed",
    "cas_legacy_helpful_count",
    "cas_legacy_harmful_count",
    "cas_legacy_created",
    "cas_legacy_updated_at",
    "cas_legacy_tags",
    "cas_legacy_team_id",
    "cas_legacy_valid_from",
    "cas_legacy_valid_until",
    "cas_legacy_review_after",
    "cas_legacy_last_reviewed",
    "cas_legacy_domain",
    "cas_legacy_branch",
    "cas_legacy_session_id",
    "cas_legacy_source_tool",
];

/// Rule C3 — the DDL defaults an omitted key decodes back to. Recorded in the
/// migration ledger so a reader does not need the spec to interpret absence.
pub const DEFAULTS: [(&str, &str); 12] = [
    ("type", "learning"),
    ("archived", "0"),
    ("stability", "0.5"),
    ("importance", "0.5"),
    ("access_count", "0"),
    ("memory_tier", "working"),
    ("helpful_count", "0"),
    ("harmful_count", "0"),
    ("belief_type", "fact"),
    ("confidence", "1.0"),
    ("pending_embedding", "1"),
    ("scope", "project"),
];

/// §5.12 — the `scope` column is known-false (all 450 GLOBAL rows claim
/// `project`), so scope is *derived*: from the `g-`/`p-` id prefix that
/// `merge_entries` strips (`hooks/context/mod.rs:528`), falling back to which
/// database file the row came from.
pub fn derive_scope(row: &LegacyRow, db: DbLabel) -> &'static str {
    if row.id.starts_with("g-") {
        "global"
    } else if row.id.starts_with("p-") {
        "project"
    } else {
        db.as_str()
    }
}

/// Render one scalar so it occupies exactly one line and cannot forge a key.
///
/// A control character in a value would otherwise close the frontmatter block
/// early or inject a fake key; JSON quoting is single-line, escapes losslessly,
/// and survives the passthrough branch untouched.
fn scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value.trim() == value
        && !value.chars().any(char::is_control)
        && !value.starts_with([
            '"', '\'', '[', '{', '-', '&', '*', '#', '!', '%', '@', '`', '>', '|', '?', ',',
        ]);
    if plain {
        value.to_string()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
    }
}

/// Format a float without a trailing `.0` cascade — `0.9`, not `0.9000000001`.
fn float(value: f64) -> String {
    let rendered = format!("{value}");
    if rendered.contains(['e', 'E']) {
        format!("{value:.6}")
    } else {
        rendered
    }
}

struct Emitter {
    lines: Vec<String>,
}

impl Emitter {
    fn scalar(&mut self, key: &str, value: &str) {
        debug_assert!(RESERVED_KEYS.contains(&key), "{key} is not a reserved key");
        self.lines.push(format!("{key}: {}", scalar(value)));
    }

    /// Emit only when the value differs from its DDL default (Rule C3).
    fn non_default(&mut self, key: &str, value: &str, default: &str) {
        if value != default {
            self.scalar(key, value);
        }
    }

    fn optional(&mut self, key: &str, value: Option<&String>) {
        if let Some(value) = value.map(String::as_str).filter(|v| !v.is_empty()) {
            self.scalar(key, value);
        }
    }
}

/// Build the `cas_legacy_*` passthrough lines for one row, in Rule C2 order.
pub fn legacy_lines(row: &LegacyRow, db: DbLabel) -> Vec<String> {
    let mut out = Emitter { lines: Vec::new() };

    // Identity and provenance — never defaulted away; these are what make the
    // migration reversible and what the ledger is keyed on.
    out.scalar("cas_legacy_id", &row.id);
    out.scalar("cas_legacy_db", db.as_str());
    out.scalar("cas_legacy_scope", derive_scope(row, db));

    out.non_default("cas_legacy_type", &row.entry_type, "learning");
    out.optional("cas_legacy_observation_type", row.observation_type.as_ref());
    out.non_default("cas_legacy_belief_type", &row.belief_type, "fact");
    out.non_default("cas_legacy_confidence", &float(row.confidence), "1");
    out.non_default("cas_legacy_memory_tier", &row.memory_tier, "working");
    if row.archived != 0 {
        out.scalar("cas_legacy_archived", "true");
    }
    out.non_default("cas_legacy_importance", &float(row.importance), "0.5");
    out.non_default("cas_legacy_stability", &float(row.stability), "0.5");
    out.non_default(
        "cas_legacy_access_count",
        &row.access_count.to_string(),
        "0",
    );
    out.optional("cas_legacy_last_accessed", row.last_accessed.as_ref());
    out.non_default(
        "cas_legacy_helpful_count",
        &row.helpful_count.to_string(),
        "0",
    );
    out.non_default(
        "cas_legacy_harmful_count",
        &row.harmful_count.to_string(),
        "0",
    );

    if !row.created.is_empty() {
        out.scalar("cas_legacy_created", &row.created);
    }
    out.optional("cas_legacy_updated_at", row.updated_at.as_ref());

    // The single list block C4 permits. A `- item` line has no colon, so it
    // falls to the passthrough branch (`merge.rs:139-144`) and round-trips.
    let mut lines = out.lines;
    if !row.tags.is_empty() {
        lines.push("cas_legacy_tags:".to_string());
        for tag in &row.tags {
            lines.push(format!("  - {}", scalar(tag)));
        }
    }

    let mut out = Emitter { lines };
    // §8.2 — team_id is authoritative; `share` is deliberately left and is not
    // even read from the database, so Rule S1 cannot be violated from here.
    out.optional("cas_legacy_team_id", row.team_id.as_ref());
    out.optional("cas_legacy_valid_from", row.valid_from.as_ref());
    out.optional("cas_legacy_valid_until", row.valid_until.as_ref());
    out.optional("cas_legacy_review_after", row.review_after.as_ref());
    out.optional("cas_legacy_last_reviewed", row.last_reviewed.as_ref());
    out.optional("cas_legacy_domain", row.domain.as_ref());
    out.optional("cas_legacy_branch", row.branch.as_ref());
    out.optional("cas_legacy_session_id", row.session_id.as_ref());
    out.optional("cas_legacy_source_tool", row.source_tool.as_ref());
    out.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_prefers_the_id_prefix_over_the_database_file() {
        let row = LegacyRow::for_test("g-1", "context", None, "x");
        assert_eq!(derive_scope(&row, DbLabel::Project), "global");
        let row = LegacyRow::for_test("p-1", "context", None, "x");
        assert_eq!(derive_scope(&row, DbLabel::Global), "project");
        let row = LegacyRow::for_test("abc", "context", None, "x");
        assert_eq!(derive_scope(&row, DbLabel::Global), "global");
    }

    #[test]
    fn a_control_character_cannot_close_the_block_or_forge_a_key() {
        assert_eq!(scalar("mcp\n---\nevil: true"), "\"mcp\\n---\\nevil: true\"");
        assert!(!scalar("a\rb").contains('\r'));
    }

    #[test]
    fn plain_scalars_stay_readable() {
        assert_eq!(scalar("2026-01-01T00:00:00Z"), "2026-01-01T00:00:00Z");
        assert_eq!(scalar("team-42"), "team-42");
        assert_eq!(scalar("p-abc123"), "p-abc123");
    }

    #[test]
    fn leading_yaml_indicators_are_quoted() {
        assert_eq!(scalar("- not a list"), "\"- not a list\"");
        assert_eq!(scalar("*anchor"), "\"*anchor\"");
        assert_eq!(scalar(" padded "), "\" padded \"");
    }

    #[test]
    fn every_line_is_single_line_for_any_input() {
        let mut row = LegacyRow::fully_populated_for_test();
        row.tags = vec!["a\nb: c".to_string()];
        row.domain = Some("x\ny".to_string());
        for line in legacy_lines(&row, DbLabel::Project) {
            assert!(!line.contains('\n'), "{line:?} spans lines");
        }
    }

    #[test]
    fn defaults_are_omitted_and_non_defaults_are_emitted() {
        let bare = legacy_lines(
            &LegacyRow::for_test("p-1", "learning", None, "x"),
            DbLabel::Project,
        );
        assert!(!bare.iter().any(|l| l.starts_with("cas_legacy_importance")));
        assert!(!bare.iter().any(|l| l.starts_with("cas_legacy_type")));

        let mut row = LegacyRow::for_test("p-1", "learning", None, "x");
        row.importance = 0.9;
        row.entry_type = "context".to_string();
        let rich = legacy_lines(&row, DbLabel::Project);
        assert!(rich.contains(&"cas_legacy_importance: 0.9".to_string()));
        assert!(rich.contains(&"cas_legacy_type: context".to_string()));
    }

    #[test]
    fn confidence_and_archived_use_their_ddl_defaults() {
        let mut row = LegacyRow::for_test("p-1", "learning", None, "x");
        assert!(
            !legacy_lines(&row, DbLabel::Project)
                .iter()
                .any(|l| l.starts_with("cas_legacy_confidence"))
        );
        row.confidence = 0.4;
        row.archived = 1;
        let lines = legacy_lines(&row, DbLabel::Project);
        assert!(lines.contains(&"cas_legacy_confidence: 0.4".to_string()));
        assert!(lines.contains(&"cas_legacy_archived: true".to_string()));
    }

    #[test]
    fn tags_render_as_the_one_permitted_list_block() {
        let mut row = LegacyRow::for_test("p-1", "learning", None, "x");
        row.tags = vec!["alpha".into(), "beta".into()];
        let joined = legacy_lines(&row, DbLabel::Project).join("\n");
        assert!(
            joined.contains("cas_legacy_tags:\n  - alpha\n  - beta"),
            "{joined}"
        );
    }

    #[test]
    fn the_reserved_key_set_has_no_duplicates() {
        let mut sorted = RESERVED_KEYS.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len());
    }
}
