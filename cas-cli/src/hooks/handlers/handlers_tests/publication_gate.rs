//! cas-f6ad (GH #1057): no deliverable share before its epic's verification
//! passes. Deny, allow and the logged operator override through the real
//! PreToolUse handler.
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};
use cas_types::{Dependency, DependencyType, Task, TaskStatus, TaskType};
use serde_json::{Value, json};

fn input(tool: &str, args: Value, cwd: &std::path::Path) -> HookInput {
    HookInput {
        session_id: "publication-gate-session".into(),
        cwd: cwd.display().to_string(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(args),
        ..HookInput::default()
    }
}

fn denied(output: HookOutput) -> Option<String> {
    let value = serde_json::to_value(output).unwrap();
    (value
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
        == Some("deny"))
    .then(|| {
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    })
}

/// An epic with one open verification task and one ordinary closed child.
fn epic_store(root: &std::path::Path) -> std::sync::Arc<dyn cas_store::TaskStore> {
    let cas = root.join(".cas");
    std::fs::create_dir_all(&cas).unwrap();
    let store = crate::store::open_task_store_local(&cas).unwrap();
    store.init().unwrap();
    let mut epic = Task::new("cas-pub-epic".into(), "client report epic".into());
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;
    store.add(&epic).unwrap();
    let mut verify = Task::new("cas-pub-verify".into(), "Verify the client report".into());
    verify.status = TaskStatus::InProgress;
    verify.labels = vec!["verification".into()];
    store.add(&verify).unwrap();
    let mut work = Task::new("cas-pub-work".into(), "Write the report".into());
    work.status = TaskStatus::Closed;
    store.add(&work).unwrap();
    for child in ["cas-pub-verify", "cas-pub-work"] {
        store
            .add_dependency(&Dependency::new(child.into(), epic.id.clone(), DependencyType::ParentChild))
            .unwrap();
    }
    store
}

#[test]
fn deliverable_post_is_denied_while_epic_verification_is_open_cas_f6ad() {
    let _env = TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let _store = epic_store(dir.path());
    let cas = dir.path().join(".cas");
    let file = json!({"channel":"client-internal","kind":"file","epic_id":"cas-pub-epic",
        "file":{"filename":"report.pdf","content":"JVBERi0=","content_encoding":"base64"}});
    for (tool, args) in [
        ("mcp__violet__violet_post", file.clone()),
        ("violet.violet_post", file.clone()),
        ("mcp__cas__mcp_execute", json!({"code":{"server":"violet","tool":"violet_post","arguments":file.clone()}})),
        ("mcp__cas__mcp_execute", json!({"code":format!("violet.violet_post({file})")})),
        (
            "mcp__violet__violet_post",
            json!({"channel":"client-internal","kind":"message","text":"Report attached","deliverable":true,"task_id":"cas-pub-work"}),
        ),
        // cas-1206 (GH #1154): the 2026-10-09 contract's other file routes.
        (
            "mcp__violet__violet_post",
            json!({"channel":"client-internal","kind":"file_external","step":"complete","epic_id":"cas-pub-epic",
                "file_id":"F1","filename":"report.pdf","size_bytes":2_000_000,"sha256":"a".repeat(64)}),
        ),
        (
            "mcp__violet__violet_post",
            json!({"channel":"client-internal","kind":"thread","epic_id":"cas-pub-epic","text":"Report","idempotency_key":"k",
                "replies":[{"text":"Summary"},{"text":"PDF","files":[{"filename":"report.pdf","content":"JVBERi0=","content_encoding":"base64"}]}]}),
        ),
    ] {
        let reason = denied(handle_pre_tool_use(&input(tool, args.clone(), dir.path()), Some(&cas)).unwrap())
            .unwrap_or_else(|| panic!("{tool} {args} must be denied"));
        assert!(reason.contains("verification_pending"), "{reason}");
        assert!(reason.contains("cas-pub-verify"), "the denial lists the open task: {reason}");
        assert!(!reason.contains("cas-pub-work ("), "a closed child is not listed: {reason}");
        assert!(reason.contains("caveat is not enough"), "{reason}");
        assert!(reason.contains("PUBLICATION OVERRIDE:"), "names the override: {reason}");
    }
}

#[test]
fn ordinary_messages_and_verified_epics_are_allowed_cas_f6ad() {
    let _env = TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let store = epic_store(dir.path());
    let cas = dir.path().join(".cas");
    // A status message that shares no deliverable is not gated.
    let message = json!({"channel":"client-internal","kind":"message","text":"Working on it","epic_id":"cas-pub-epic"});
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", message, dir.path()), Some(&cas)).unwrap()).is_none());
    // A text-only ordered thread (cas-1206) shares no file either.
    let thread = json!({"channel":"client-internal","kind":"thread","text":"Status","idempotency_key":"k",
        "epic_id":"cas-pub-epic","replies":[{"text":"Was → Now"}]});
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", thread, dir.path()), Some(&cas)).unwrap()).is_none());
    // A file post with no epic context has nothing to wait on.
    let unbound = json!({"channel":"client-internal","kind":"file","file":{"filename":"x.txt","content":"x"}});
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", unbound, dir.path()), Some(&cas)).unwrap()).is_none());
    // Reads are never gated.
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_read", json!({"channel":"x","kind":"file"}), dir.path()), Some(&cas)).unwrap()).is_none());
    // Once verification closes, the deliverable posts.
    let mut verify = store.get("cas-pub-verify").unwrap();
    verify.status = TaskStatus::Closed;
    store.update(&verify).unwrap();
    let file = json!({"channel":"client-internal","kind":"file","epic_id":"cas-pub-epic","file":{"filename":"report.pdf","content":"x"}});
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", file, dir.path()), Some(&cas)).unwrap()).is_none());
}

#[test]
fn recorded_operator_override_allows_and_is_logged_cas_f6ad() {
    let _env = TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let store = epic_store(dir.path());
    let cas = dir.path().join(".cas");
    let file = json!({"channel":"client-internal","kind":"file","epic_id":"cas-pub-epic","file":{"filename":"report.pdf","content":"x"}});
    // A stale override does not authorize a post.
    store
        .append_note("cas-pub-epic", "[2020-01-01 00:00] DECISION PUBLICATION OVERRIDE: Daniel: post the draft now")
        .unwrap();
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", file.clone(), dir.path()), Some(&cas)).unwrap()).is_some());
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    store
        .append_note("cas-pub-epic", &format!("[{now}] ✅ DECISION PUBLICATION OVERRIDE: Daniel: share it now, the client is waiting"))
        .unwrap();
    assert!(denied(handle_pre_tool_use(&input("mcp__violet__violet_post", file, dir.path()), Some(&cas)).unwrap()).is_none());
    let notes = store.get("cas-pub-epic").unwrap().notes;
    assert!(
        notes.contains("publication gate overridden") && notes.contains("the client is waiting") && notes.contains("cas-pub-verify"),
        "each overridden post is logged on the epic: {notes}"
    );
}
