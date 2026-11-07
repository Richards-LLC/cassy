//! Project hold probes through the actual context, query and factory paths.
use crate::mcp::{CasCore, CasService};
use rmcp::handler::server::wrapper::Parameters;

fn held_project() -> (tempfile::TempDir, std::path::PathBuf) {
    let project = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(project.path()).unwrap();
    let tasks = crate::store::open_task_store(&root).unwrap();
    tasks.init().unwrap();
    let ready = cas_types::Task::new("cas-hold-ready".into(), "ready hold probe".into());
    tasks.add(&ready).unwrap();
    let mut parked = cas_types::Task::new("cas-hold-merge".into(), "merge hold probe".into());
    parked.status = cas_types::TaskStatus::AwaitingMerge;
    parked.created_at = chrono::Utc::now() - chrono::Duration::days(30);
    parked.updated_at = parked.created_at;
    tasks.add(&parked).unwrap();
    std::fs::create_dir_all(root.join("operator")).unwrap();
    std::fs::write(root.join("operator/project-hold.json"), serde_json::json!({
        "active": {"reason": "Review the plan", "operator": "operator-fixture", "set_at": "2026-11-03T14:00:00Z"},
        "history": []
    }).to_string()).unwrap();
    (project, root)
}

fn text(result: rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| item.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn project_hold_is_first_and_hides_ready_tasks_at_session_start() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.remove("CAS_CLONE_PATH");
    env.remove("CAS_AGENT_NAME");
    let (project, root) = held_project();
    let input = crate::hooks::HookInput {
        session_id: "hold-test".into(),
        cwd: project.path().to_string_lossy().into(),
        hook_event_name: "SessionStart".into(),
        ..Default::default()
    };
    let context =
        serde_json::to_value(crate::hooks::handle_session_start(&input, Some(&root)).unwrap())
            .unwrap();
    let context = context
        .pointer("/hookSpecificOutput/additionalContext")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(context.starts_with("## Project hold"), "{context}");
    assert!(
        context.contains("Review the plan") && context.contains("operator-fixture"),
        "{context}"
    );
    assert!(
        !context.contains("ready hold probe") && !context.contains("merge hold probe") && !context.contains("## Ready Tasks"),
        "{context}"
    );
    std::fs::remove_file(root.join("operator/project-hold.json")).unwrap();
    let restored = crate::hooks::build_context(&input, 5, &root).unwrap();
    assert!(restored.contains("ready hold probe"), "{restored}");
}

#[tokio::test]
async fn project_hold_suppresses_ready_and_awaiting_merge_queries_until_lifted() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_project, root) = held_project();
    let core = CasCore::with_daemon(root.clone(), None, None);
    let ready = || serde_json::from_value(serde_json::json!({})).unwrap();
    let merges =
        || serde_json::from_value(serde_json::json!({"status": "awaiting_merge"})).unwrap();
    let result = text(core.cas_task_ready(Parameters(ready())).await.unwrap());
    assert!(
        result.contains("Review the plan") && !result.contains("cas-hold-ready"),
        "{result}"
    );
    let result = text(core.cas_task_list(Parameters(merges())).await.unwrap());
    assert!(
        result.contains("Review the plan") && !result.contains("cas-hold-merge"),
        "{result}"
    );
    // The unfiltered query is also an operator surface; internal store lists
    // retain these rows, but the MCP response must never invite a held merge.
    for query in [serde_json::json!({}), serde_json::json!({"status": "merge"})] {
        let unfiltered = query.get("status").is_none();
        let result = text(core.cas_task_list(Parameters(serde_json::from_value(query).unwrap())).await.unwrap());
        assert!(!result.contains("cas-hold-merge"), "{result}");
        assert!(result.contains("Review the plan"), "{result}");
        assert_eq!(result.contains("cas-hold-ready"), unfiltered);
    }
    std::fs::remove_file(root.join("operator/project-hold.json")).unwrap();
    assert!(
        text(core.cas_task_ready(Parameters(ready())).await.unwrap()).contains("cas-hold-ready")
    );
    assert!(
        text(core.cas_task_list(Parameters(merges())).await.unwrap()).contains("cas-hold-merge")
    );
}

#[tokio::test]
async fn project_hold_refuses_merge_even_with_overrides_and_leaves_tasks_untouched() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_project, root) = held_project();
    let core = CasCore::with_daemon(root.clone(), None, None);
    let error = core
        .worktree_merge(
            "missing-worker",
            true,
            Some("cas-hold-merge"),
            true,
            Some(true),
            true,
            Some("override"),
        )
        .await
        .unwrap_err()
        .to_string();

    assert_eq!(
        crate::store::open_task_store(&root)
            .unwrap()
            .get("cas-hold-merge")
            .unwrap()
            .status,
        cas_types::TaskStatus::AwaitingMerge
    );
    std::fs::remove_file(root.join("operator/project-hold.json")).unwrap();
    let error = core
        .worktree_merge(
            "missing-worker",
            true,
            Some("cas-hold-merge"),
            true,
            Some(true),
            true,
            Some("override"),
        )
        .await
        .unwrap_err()
        .to_string();
}

#[tokio::test]
async fn project_hold_refuses_spawn_before_it_can_queue_a_worker() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_project, root) = held_project();
    let core = CasCore::with_daemon(root.clone(), None, None);
    #[cfg(feature = "mcp-proxy")]
    let service = CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = CasService::new(core);
    let req = serde_json::from_value(
        serde_json::json!({"action": "spawn_workers", "count": 1, "force": true}),
    )
    .unwrap();
    let error = service
        .factory_spawn_workers_in(
            &crate::ops::fleet::FleetContext::operator("hold-session"),
            req,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(
        crate::store::open_spawn_queue_store(&root)
            .unwrap()
            .peek(10)
            .unwrap()
            .is_empty()
    );
}
