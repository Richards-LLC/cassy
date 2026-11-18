use std::fs;

use cas_core::sync::{AgentsMdSyncMode, GENERATED_HEADER, sync_agents_md, transform_agents_md};
use tempfile::tempdir;

#[test]
fn identity_adds_only_the_generated_header() {
    assert_eq!(
        transform_agents_md("# Project\nUse local tools.\n"),
        format!("{GENERATED_HEADER}# Project\nUse local tools.\n")
    );
}

#[test]
fn transform_swaps_prefix_and_applies_gate_markers() {
    let generated = transform_agents_md(
        "before\nmcp__cas__task\n<!-- claude-only:start -->\nclaude guidance\n<!-- claude-only:end -->\n<!-- codex-only:start -->\ncodex guidance\n<!-- codex-only:end -->\nafter\n",
    );

    assert_eq!(generated, format!("{GENERATED_HEADER}before\nmcp__cs__task\ncodex guidance\nafter\n"));
}

#[test]
fn write_is_idempotent_and_check_detects_staleness() {
    let project = tempdir().unwrap();
    let source = project.path().join("CLAUDE.md");
    fs::write(&source, "mcp__cas__task\n").unwrap();

    let first = sync_agents_md(project.path(), AgentsMdSyncMode::Write).unwrap();
    assert_eq!(first.stale_count(), 1);
    let first_output = fs::read_to_string(project.path().join("AGENTS.md")).unwrap();

    let second = sync_agents_md(project.path(), AgentsMdSyncMode::Write).unwrap();
    assert_eq!(second.stale_count(), 0);
    assert_eq!(
        fs::read_to_string(project.path().join("AGENTS.md")).unwrap(),
        first_output
    );

    fs::write(&source, "mcp__cas__task updated\n").unwrap();
    let check = sync_agents_md(project.path(), AgentsMdSyncMode::Check).unwrap();
    assert_eq!(check.stale_count(), 1);
}
