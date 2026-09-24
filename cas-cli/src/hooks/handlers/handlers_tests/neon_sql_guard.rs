//! cas-d8fc (GH #907): a factory worker's Neon SQL write is refused unless
//! branchId names a non-production branch; reads are unaffected.
use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::{HookInput, HookOutput};

fn input(
    tool: &str,
    tool_input: serde_json::Value,
    role: &str,
    cwd: &std::path::Path,
) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: cwd.display().to_string(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(tool_input),
        agent_role: Some(role.into()),
        ..HookInput::default()
    }
}

fn deny_reason(out: &HookOutput) -> Option<String> {
    let value = serde_json::to_value(out.hook_specific_output.as_ref()?).ok()?;
    if value.get("permissionDecision")?.as_str()? != "deny" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|reason| reason.as_str())
        .map(str::to_string)
}

fn repo_with_skill() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".claude/skills/neon-database");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "<!-- keep neon-ids -->\n| | Value |\n|--|--|\n| **org_id** | `org-1` |\n| **projectId** | `proj-1` |\n| **databaseName** | `neondb` |\n| **production branchId** | `br-prod` (name: `main`) |\n<!-- /keep neon-ids -->\n",
    )
    .unwrap();
    temp
}

#[test]
fn worker_write_without_branch_or_on_production_is_denied_naming_the_branch() {
    let repo = repo_with_skill();
    let no_branch = input(
        "mcp__neon__run_sql",
        serde_json::json!({"projectId": "proj-1", "sql": "INSERT INTO fixtures VALUES (1)"}),
        "worker",
        repo.path(),
    );
    let reason = deny_reason(&handle_pre_tool_use(&no_branch, None).unwrap()).expect("denied");
    assert!(
        reason.contains("NEON PRODUCTION WRITE")
            && reason.contains("no branchId")
            && reason.contains("production"),
        "{reason}"
    );

    let prod = input(
        "mcp__neon__run_sql_transaction",
        serde_json::json!({"projectId": "proj-1", "branchId": "br-prod", "sqlStatements": ["DELETE FROM fixtures"]}),
        "worker",
        repo.path(),
    );
    let reason = deny_reason(&handle_pre_tool_use(&prod, None).unwrap()).expect("denied");
    assert!(reason.contains("`br-prod`"), "{reason}");
}

#[test]
fn worker_reads_and_non_production_writes_pass_and_other_roles_are_untouched() {
    let repo = repo_with_skill();
    for (tool_input, role) in [
        (
            serde_json::json!({"projectId": "proj-1", "sql": "SELECT * FROM fixtures"}),
            "worker",
        ),
        (
            serde_json::json!({"projectId": "proj-1", "branchId": "br-qa-1700", "sql": "INSERT INTO fixtures VALUES (1)"}),
            "worker",
        ),
        (
            serde_json::json!({"projectId": "proj-1", "sql": "INSERT INTO fixtures VALUES (1)"}),
            "supervisor",
        ),
    ] {
        let out = handle_pre_tool_use(
            &input("mcp__neon__run_sql", tool_input.clone(), role, repo.path()),
            None,
        )
        .unwrap();
        assert!(
            deny_reason(&out).is_none_or(|reason| !reason.contains("NEON PRODUCTION WRITE")),
            "{role} {tool_input}"
        );
    }
}
