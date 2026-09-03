use crate::support::*;
use cas::cloud::CloudConfig;
use cas::mcp::tools::*;
use cas::mcp::{CasCore, CasService};
use cas::store::{SqliteTaskStore, TaskStore, open_agent_store, open_event_store, open_task_store};
use cas::types::{EventType, Task};
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MOVE_TEAM: &str = "move-team-uuid";

fn promote_default_test_agent(cas_dir: &std::path::Path) {
    let agent_store = open_agent_store(cas_dir).expect("agent store");
    let id = format!("test-session-{}", std::process::id());
    let mut agent = agent_store.get(&id).expect("default test agent");
    agent.role = cas::types::AgentRole::Supervisor;
    agent.heartbeat();
    agent_store.update(&agent).expect("promote test agent");
}

#[tokio::test]
async fn origin_project_move_refuses_unregistered_destination_before_local_write() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_default_test_agent(&cas_dir);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{MOVE_TEAM}/projects")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut cloud_config = CloudConfig::default();
    cloud_config.endpoint = server.uri();
    cloud_config.token = Some("test-token".to_string());
    cloud_config.set_team(MOVE_TEAM, "move-team");
    cloud_config.save_to_cas_dir(&cas_dir).unwrap();

    let local_store = cas::store::open_task_store_local(&cas_dir).unwrap();
    let mut task = cas::types::Task::new("cas-move-refuse".into(), "move refusal".into());
    task.origin_project = Some("project-a".into());
    local_store.add(&task).unwrap();

    let request: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": task.id,
        "origin_project": "project-b"
    }))
    .unwrap();
    let error = core
        .cas_task_update(Parameters(request))
        .await
        .expect_err("an unregistered destination must be rejected");
    assert!(
        error.message.contains("not registered"),
        "{}",
        error.message
    );
    assert_eq!(
        local_store.get(&task.id).unwrap().origin_project.as_deref(),
        Some("project-a")
    );
}

#[tokio::test]
async fn origin_project_move_updates_local_row_audit_and_team_queue() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_default_test_agent(&cas_dir);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{MOVE_TEAM}/projects")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{
                "id": "project-b-uuid",
                "canonical_id": "project-b",
                "name": "Project B",
                "contributor_count": 1,
                "memory_count": 0
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut cloud_config = CloudConfig::default();
    cloud_config.endpoint = server.uri();
    cloud_config.token = Some("test-token".to_string());
    cloud_config.set_team(MOVE_TEAM, "move-team");
    cloud_config.save_to_cas_dir(&cas_dir).unwrap();

    let local_store = cas::store::open_task_store_local(&cas_dir).unwrap();
    let mut task = cas::types::Task::new("cas-move-local".into(), "move local".into());
    task.origin_project = Some("project-a".into());
    local_store.add(&task).unwrap();

    let request: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": task.id,
        "origin_project": "project-b"
    }))
    .unwrap();
    core.cas_task_update(Parameters(request))
        .await
        .expect("registered destination move should succeed");

    let updated = local_store.get(&task.id).unwrap();
    assert_eq!(updated.origin_project.as_deref(), Some("project-b"));
    assert!(
        updated
            .notes
            .contains("DECISION: moved from project-a to project-b by test-agent"),
        "audit note missing: {}",
        updated.notes
    );
    let queue = cas::cloud::SyncQueue::open(&cas_dir).unwrap();
    let pending = queue.pending_for_team(MOVE_TEAM, 10, 5).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].operation, cas::cloud::SyncOperation::Delete);
    assert_eq!(pending[0].project_id.as_deref(), Some("project-a"));
    assert_eq!(pending[1].operation, cas::cloud::SyncOperation::Upsert);
}

/// cas-0447 (GH #187): a context-poor worker needs a bounded start response
/// that preserves its own task notes without inheriting an epic's potentially
/// enormous sibling-note payload.
#[tokio::test]
async fn test_0447_brief_task_start_returns_only_own_notes_and_is_size_bounded() {
    let (temp, service) = setup_cas();

    let epic = service
        .cas_task_create(Parameters(TaskCreateRequest {
            task_type: "epic".to_string(),
            ..basic_create("Brief-start epic", None)
        }))
        .await
        .expect("create epic");
    let epic_id = extract_task_id(&extract_text(epic))
        .expect("epic id")
        .to_string();

    let sibling_marker = "SIBLING-NOTES-MUST-NOT-LEAK";
    service
        .cas_task_create(Parameters(TaskCreateRequest {
            notes: Some(format!("{sibling_marker}{}", "x".repeat(128_000))),
            epic: Some(epic_id.clone()),
            ..basic_create("Large-note sibling", None)
        }))
        .await
        .expect("create sibling");

    let own_marker = "OWN-TASK-NOTES-MUST-SURVIVE";
    let target = service
        .cas_task_create(Parameters(TaskCreateRequest {
            notes: Some(own_marker.to_string()),
            epic: Some(epic_id.clone()),
            ..basic_create("Brief-start target", None)
        }))
        .await
        .expect("create target");
    let target_id = extract_task_id(&extract_text(target))
        .expect("target id")
        .to_string();

    open_task_store(&temp.path().join(".cas"))
        .expect("task store")
        .patch_execution_state(
            &target_id,
            &serde_json::json!({"phase": "resume", "next_step": "continue implementation"}),
        )
        .expect("persist structured execution state");

    // Exercise the public unified MCP boundary, including JSON bool
    // deserialization and TaskRequest -> TaskStartRequest forwarding.
    let service = CasService::new(service, None);
    let start: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "start",
        "id": target_id,
        "brief": true,
    }))
    .expect("deserialize brief start request");
    let response = service.task(Parameters(start)).await.expect("brief start");
    let text = extract_text(response);

    assert!(text.contains(own_marker), "own notes missing: {text}");
    assert!(
        text.contains("Structured execution state:") && text.contains("continue implementation"),
        "brief start missing structured execution state: {text}"
    );
    assert!(
        !text.contains(sibling_marker),
        "brief start leaked sibling notes: {} bytes",
        text.len()
    );
    assert!(
        !text.contains("SIBLING TASK NOTES") && !text.contains("EPIC OWNERSHIP"),
        "brief start leaked epic context: {text}"
    );
    assert!(
        text.len() < 8_192,
        "brief start response must stay context-affordable; got {} bytes",
        text.len()
    );
    assert_eq!(
        open_task_store(&temp.path().join(".cas"))
            .expect("task store")
            .get(&epic_id)
            .expect("epic")
            .status,
        cas::types::TaskStatus::InProgress,
        "brief output must not change normal start lifecycle effects"
    );
}

/// GH #515: close must publish the committed epic outcome before a huge
/// stranded-branch audit payload is written. The fixture has 31 child lanes
/// and a 200 KB supervisor narrative, the incident shape that previously
/// exhausted the MCP response budget.
#[tokio::test]
async fn epic_override_close_returns_compact_receipt_before_large_note_gh_515() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).expect("task store");
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let session_id = format!("test-session-{}", std::process::id());
    let mut supervisor = agent_store.get(&session_id).expect("registered caller");
    supervisor.role = cas::types::AgentRole::Supervisor;
    agent_store
        .update(&supervisor)
        .expect("make caller supervisor");

    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").expect("write seed");
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);

    let epic = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: Some("light".to_string()),
            title: "large override epic".to_string(),
            description: None,
            priority: 2,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create epic");
    let epic_id = extract_task_id(&extract_text(epic))
        .expect("epic id")
        .to_string();
    let mut stored_epic = task_store.get(&epic_id).expect("stored epic");
    stored_epic.branch = Some("main".to_string());
    task_store.update(&stored_epic).expect("set epic branch");

    for child in 0..31 {
        let worker = format!("worker-{child:02}");
        git(&["checkout", "-q", "-b", &format!("factory/{worker}"), "main"]);
        let file = format!("lane-{child:02}.txt");
        std::fs::write(temp.path().join(&file), format!("lane {child}\n")).expect("write lane");
        git(&["add", &file]);
        git(&["commit", "-q", "-m", &format!("lane {child}")]);
        git(&["checkout", "-q", "main"]);

        let child_task = service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..basic_create(&format!("child {child}"), None)
            }))
            .await
            .expect("create child");
        let child_id = extract_task_id(&extract_text(child_task))
            .expect("child id")
            .to_string();
        let mut stored_child = task_store.get(&child_id).expect("stored child");
        stored_child.status = cas::types::TaskStatus::Closed;
        stored_child.assignee = Some(worker);
        task_store
            .update(&stored_child)
            .expect("close child fixture");
    }

    let narrative = format!(
        "GH515-NARRATIVE-START: I diffed each of the 31 child branches against main and recorded the measured result. {} GH515-NARRATIVE-END",
        "evidence retained for each branch; ".repeat(7_000)
    );
    assert!(narrative.len() > 200_000, "large-narrative precondition");
    let started = std::time::Instant::now();
    let close = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: Some(narrative.clone()),
            id: epic_id.clone(),
            reason: Some("override incident-shaped epic close".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("close epic");
    let elapsed = started.elapsed();
    let receipt = extract_text(close);
    assert!(
        receipt.contains("Closed task:") && receipt.contains("epic close committed"),
        "close must return the compact committed receipt: {receipt}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "close waited for the large note instead of returning its committed receipt: {elapsed:?}"
    );
    assert_eq!(
        task_store.get(&epic_id).expect("closed epic").status,
        cas::types::TaskStatus::Closed,
        "the terminal mutation must commit before the compact receipt"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let stored = task_store.get(&epic_id).expect("read closed epic");
        if stored.notes.contains("GH515-NARRATIVE-START")
            && stored.notes.contains("GH515-NARRATIVE-END")
            && stored.notes.len() >= narrative.len()
        {
            break;
        }
        let current = task_store.get(&epic_id).expect("read current epic");
        assert!(
            std::time::Instant::now() < deadline,
            "the full override narrative must land durably after the compact close receipt; task notes contain {} bytes",
            current.notes.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn test_task_show() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Show task".to_string(),
        description: Some("Detailed description".to_string()),
        priority: 1,
        task_type: "bug".to_string(),
        labels: Some("urgent".to_string()),
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Show task
    let show_req = TaskShowRequest {
        id: id.to_string(),
        with_deps: true,
    };
    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task_show should succeed");

    let text = extract_text(result);
    assert!(text.contains("Show task"));
    assert!(text.contains("Detailed description") || text.contains("bug"));
}

#[tokio::test]
async fn task_show_renders_work_target_and_explicit_trunk_fallback_cas_0094() {
    let (temp, core) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");

    let mut targeted =
        cas::types::Task::new("cas-targeted-show".to_string(), "Targeted task".to_string());
    targeted.deliverables.work_target = Some(cas::types::WorkTarget {
        repo_selector: "project:gabber-studio".to_string(),
        target_branch: "staging".to_string(),
    });
    task_store.add(&targeted).expect("add targeted task");

    let fallback =
        cas::types::Task::new("cas-fallback-show".to_string(), "Fallback task".to_string());
    task_store.add(&fallback).expect("add fallback task");

    let service = CasService::new(core, None);
    for (task_id, expected) in [
        (targeted.id, "Target: project:gabber-studio @ staging"),
        (fallback.id, "Target: (none — trunk fallback)"),
    ] {
        let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
            "action": "show",
            "id": task_id,
        }))
        .expect("deserialize task show request");
        let text = extract_text(
            service
                .task(Parameters(request))
                .await
                .expect("public task show"),
        );
        assert!(
            text.lines().any(|line| line == expected),
            "task show must render {expected:?}:\n{text}"
        );
    }
}

#[tokio::test]
async fn task_update_persists_structured_state_and_rejects_invalid_patch() {
    let (_temp, core) = setup_cas();
    let created = core
        .cas_task_create(Parameters(TaskCreateRequest {
            ..basic_create("Structured state target", None)
        }))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    let service = CasService::new(core, None);

    let update: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "state_patch": {
            "phase": "implement",
            "files_touched": ["src/lib.rs"],
            "receipts": [{"command": "cargo check", "exit_status": 0}],
            "next_step": "run focused tests"
        }
    }))
    .expect("deserialize state patch");
    service
        .task(Parameters(update))
        .await
        .expect("state update");

    let show: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "show",
        "id": task_id,
    }))
    .expect("deserialize show");
    let shown = extract_text(service.task(Parameters(show)).await.expect("show task"));
    assert!(shown.contains("Structured execution state:"), "{shown}");
    assert!(shown.contains("run focused tests"), "{shown}");

    let invalid: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "state_patch": {"files_touched": "not-an-array"}
    }))
    .expect("deserialize invalid state patch");
    assert!(
        service.task(Parameters(invalid)).await.is_err(),
        "malformed state patch must be rejected"
    );

    let show_after_rejection: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "show",
        "id": task_id,
    }))
    .expect("deserialize second show");
    let shown_after = extract_text(
        service
            .task(Parameters(show_after_rejection))
            .await
            .expect("show task after rejection"),
    );
    assert!(shown_after.contains("run focused tests"), "{shown_after}");
}

// =============================================================================
// cas-7fc1: execution_note field end-to-end coverage
// =============================================================================

fn basic_create(title: &str, execution_note: Option<String>) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: None,
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note,
        epic: None,
    }
}

/// Happy path: create a task with an accepted execution_note value and
/// verify it is persisted + surfaced by `action=show`.
#[tokio::test]
async fn test_execution_note_create_and_show_happy_path() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Task with execution note",
            Some("test-first".to_string()),
        )))
        .await
        .expect("create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show should succeed");
    let text = extract_text(shown);
    assert!(
        text.contains("Execution Note: test-first"),
        "show output must include execution_note line when set, got: {text}"
    );
}

/// Null path: create a task WITHOUT execution_note and verify `action=show`
/// omits the line entirely.
#[tokio::test]
async fn test_execution_note_null_omitted_from_show() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Task without execution note",
            None,
        )))
        .await
        .expect("create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show should succeed");
    let text = extract_text(shown);
    assert!(
        !text.contains("Execution Note"),
        "show output must omit execution_note line when unset, got: {text}"
    );
}

/// Invalid enum: reject unknown values at the MCP tool layer with a clear
/// error that lists the allowed values.
#[tokio::test]
async fn test_execution_note_invalid_enum_rejected() {
    let (_temp, service) = setup_cas();

    let err = service
        .cas_task_create(Parameters(basic_create(
            "Task with garbage execution note",
            Some("garbage".to_string()),
        )))
        .await
        .expect_err("invalid enum must be rejected at MCP layer");
    let msg = err.message.to_string();
    assert!(
        msg.contains("Invalid execution_note"),
        "error must name the bad field, got: {msg}"
    );
    assert!(
        msg.contains("test-first")
            && msg.contains("characterization-first")
            && msg.contains("additive-only")
            && msg.contains("value-only"),
        "error must list allowed values, got: {msg}"
    );
}

/// cas-8ad8: value-only is an accepted declaration for copy/i18n work that
/// modifies existing values without claiming a new-file-only posture.
#[tokio::test]
async fn test_execution_note_value_only_create_and_show() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Copy-value correction",
            Some("value-only".to_string()),
        )))
        .await
        .expect("value-only must be accepted");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show should succeed");
    assert!(extract_text(shown).contains("Execution Note: value-only"));
}

/// Update path: create without execution_note, then set it via update.
#[tokio::test]
async fn test_execution_note_update_sets_value() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create("Update target", None)))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let updated = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: Some("additive-only".to_string()),
            external_ref: None,
            assignee: None,
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update");
    assert!(
        extract_text(updated).contains("execution_note"),
        "update response must list changed field"
    );

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show");
    assert!(extract_text(shown).contains("Execution Note: additive-only"));
}

/// Unset path: passing an empty string on update clears the field back to None.
#[tokio::test]
async fn test_execution_note_update_empty_string_clears() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Clear target",
            Some("characterization-first".to_string()),
        )))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: Some(String::new()),
            external_ref: None,
            assignee: None,
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update clear");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show");
    assert!(
        !extract_text(shown).contains("Execution Note"),
        "empty string must clear the field"
    );
}

#[tokio::test]
async fn test_task_update() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Update task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Update task
    let update_req = TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: id.to_string(),
        title: Some("Updated title".to_string()),
        notes: Some("Added note".to_string()),
        priority: Some(1),
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: None,
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    };

    let result = service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated") || text.contains("updated"));
}

#[tokio::test]
async fn test_task_update_design_and_acceptance_criteria() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Spec task".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Update design and acceptance_criteria
    let update_req = TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: id.to_string(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: Some("## Technical Spec\nThis is the design.".to_string()),
        acceptance_criteria: Some("- [ ] Criterion 1\n- [ ] Criterion 2".to_string()),
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: None,
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    };

    let result = service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Updated") || text.contains("updated") || text.contains("design"),
        "Update should succeed: {text}"
    );

    // Verify via show
    let show_req = TaskShowRequest {
        id: id.to_string(),
        with_deps: false,
    };

    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task_show should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Technical Spec"),
        "Show should include design: {text}"
    );
    assert!(
        text.contains("Criterion 1"),
        "Show should include acceptance_criteria: {text}"
    );
}

#[tokio::test]
async fn test_task_notes() {
    let (temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Notes task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Add notes
    let notes_req = TaskNotesRequest {
        id: id.to_string(),
        note: "Making progress on implementation".to_string(),
        note_type: "progress".to_string(),
    };

    let result = service
        .cas_task_notes(Parameters(notes_req))
        .await
        .expect("task_notes should succeed");

    let text = extract_text(result);
    assert!(text.contains("Added note") || text.contains("note"));

    let session_id = format!("test-session-{}", std::process::id());
    let event_store = open_event_store(&temp.path().join(".cas")).expect("open event store");
    let events = event_store
        .list_by_session(&session_id, 20)
        .expect("list events for caller session");
    let note_event = events
        .iter()
        .find(|event| {
            event.event_type == EventType::TaskNoteAdded
                && event.entity_id == id
                && event.session_id.as_deref() == Some(session_id.as_str())
        })
        .expect("real task notes handler should record caller-attributed TaskNoteAdded event");
    assert!(note_event.summary.contains("Task note added (progress)"));
    assert!(
        note_event
            .summary
            .contains("Making progress on implementation")
    );
}

/// GH #342: the unified `notes` action is a read when `notes` is absent and
/// remains the existing append operation when `notes` is present.
#[tokio::test]
async fn task_notes_read_and_append_are_disambiguated_by_notes_presence() {
    let (temp, core) = setup_cas();
    let description_marker = "DESCRIPTION-MUST-NOT-LEAK";
    let acceptance_marker = "ACCEPTANCE-MUST-NOT-LEAK";
    let initial_note = "Initial worker progress";

    let created = core
        .cas_task_create(Parameters(TaskCreateRequest {
            description: Some(format!("{description_marker}{}", "x".repeat(32_000))),
            notes: Some(initial_note.to_string()),
            acceptance_criteria: Some(acceptance_marker.to_string()),
            ..basic_create("Long task whose notes are read frequently", None)
        }))
        .await
        .expect("create long task");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    let service = CasService::new(core, None);

    // `note_type` alone is metadata, not append intent. Without `notes`, this
    // must stay read-only and return no heavyweight task fields.
    let read_request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "notes",
        "id": id,
        "note_type": "blocker"
    }))
    .unwrap();
    let first_read = extract_text(service.task(Parameters(read_request)).await.unwrap());
    assert!(first_read.contains(initial_note), "{first_read}");
    assert!(!first_read.contains(description_marker), "{first_read}");
    assert!(!first_read.contains(acceptance_marker), "{first_read}");
    assert!(
        first_read.len() < 1_024,
        "notes read was {} bytes",
        first_read.len()
    );

    let append_request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "notes",
        "id": id,
        "notes": "Supervisor-visible decision",
        "note_type": "decision"
    }))
    .unwrap();
    let appended = extract_text(service.task(Parameters(append_request)).await.unwrap());
    assert!(appended.contains("Added decision note"), "{appended}");

    let second_read: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "notes",
        "id": id
    }))
    .unwrap();
    let second_read = extract_text(service.task(Parameters(second_read)).await.unwrap());
    assert!(second_read.contains(initial_note), "{second_read}");
    assert!(
        second_read.contains("✅ DECISION Supervisor-visible decision"),
        "{second_read}"
    );
    assert!(!second_read.contains(description_marker), "{second_read}");

    let persisted = open_task_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(persisted.notes.matches(initial_note).count(), 1);
    assert_eq!(
        persisted
            .notes
            .matches("Supervisor-visible decision")
            .count(),
        1
    );
    assert!(
        !persisted.notes.contains("🚫 BLOCKER"),
        "read with note_type must not append: {}",
        persisted.notes
    );
}

#[tokio::test]
async fn test_task_notes_succeeds_when_activity_event_recording_fails() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let req = TaskCreateRequest {
        depth: None,
        title: "Notes event failure task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");
    let text = extract_text(result);
    let id = extract_task_id(&text)
        .expect("should have task ID")
        .to_string();

    open_event_store(&cas_dir).expect("event store should initialize");
    let conn = Connection::open(cas_dir.join("cas.db")).expect("open cas db");
    conn.execute_batch(
        r#"
        CREATE TRIGGER fail_task_note_added_events
        BEFORE INSERT ON events
        WHEN NEW.event_type = 'task_note_added'
        BEGIN
            SELECT RAISE(ABORT, 'forced task note event failure');
        END;
        "#,
    )
    .expect("install failing event trigger");

    let result = service
        .cas_task_notes(Parameters(TaskNotesRequest {
            id: id.clone(),
            note: "This note should survive event failure".to_string(),
            note_type: "progress".to_string(),
        }))
        .await
        .expect("task_notes should succeed even if activity event recording fails");

    let text = extract_text(result);
    assert!(text.contains("Added note") || text.contains("note"));

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("task_show should succeed");
    let shown_text = extract_text(shown);
    assert!(shown_text.contains("This note should survive event failure"));
}

#[tokio::test]
async fn test_task_list() {
    let (_temp, service) = setup_cas();

    // Create tasks
    for i in 0..3 {
        let req = TaskCreateRequest {
            depth: None,
            title: format!("List task {i}"),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        };
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed");
    }

    // List tasks
    let list_req = TaskListRequest {
        scope: "all".to_string(),
        limit: Some(10),
        status: None,
        task_type: None,
        label: None,
        assignee: None,
        epic: None,
        sort: None,
        sort_order: None,
        include_foreign: false,
    };
    let result = service
        .cas_task_list(Parameters(list_req))
        .await
        .expect("task_list should succeed");

    let text = extract_text(result);
    assert!(text.contains("List task") || text.contains("Tasks"));
}

#[tokio::test]
async fn test_task_ready() {
    let (_temp, service) = setup_cas();

    // Create ready tasks
    for i in 0..3 {
        let req = TaskCreateRequest {
            depth: None,
            title: format!("Ready task {i}"),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        };
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed");
    }

    // List ready tasks
    let ready_req = TaskReadyBlockedRequest {
        scope: "all".to_string(),
        limit: Some(10),
        sort: None,
        sort_order: None,
        epic: None,
        include_foreign: false,
    };
    let result = service
        .cas_task_ready(Parameters(ready_req))
        .await
        .expect("task_ready should succeed");

    let text = extract_text(result);
    assert!(text.contains("Ready task") || text.contains("ready") || text.contains("Tasks"));
}

#[tokio::test]
async fn test_task_ready_excludes_foreign_origin_project_and_show_exposes_it() {
    let (temp, service) = setup_cas();

    let local = service
        .cas_task_create(Parameters(basic_create("Local origin task", None)))
        .await
        .expect("create local task");
    let local_id = extract_task_id(&extract_text(local))
        .expect("local task id")
        .to_string();
    let foreign = service
        .cas_task_create(Parameters(basic_create("Foreign origin task", None)))
        .await
        .expect("create foreign fixture");
    let foreign_id = extract_task_id(&extract_text(foreign))
        .expect("foreign task id")
        .to_string();

    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");
    let mut foreign_task = task_store.get(&foreign_id).expect("foreign task row");
    foreign_task.origin_project = Some("acme/other".to_string());
    task_store.update(&foreign_task).expect("mark foreign task");

    let text = extract_text(
        service
            .cas_task_ready(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: Some(20),
                sort: None,
                sort_order: None,
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_ready should succeed"),
    );
    assert!(
        text.contains(&local_id),
        "local task must remain visible: {text}"
    );
    assert!(
        !text.contains(&foreign_id),
        "foreign task must be excluded from ready: {text}"
    );

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: foreign_id,
            with_deps: false,
        }))
        .await
        .expect("show foreign task");
    assert!(
        extract_text(shown).contains("Origin project: acme/other"),
        "show must expose foreign origin for diagnosis"
    );
}

/// GH #690 (cas-a0d2): a task created through MCP must carry this project's
/// canonical origin, and the agent that starts it must be accepted on the
/// first attempt — no "origin project does not match current project" wedge.
#[tokio::test]
async fn create_stamps_canonical_origin_and_start_accepts_it_first_try() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let project = cas::cloud::resolve_canonical_id(&cas_dir).expect("project identity resolves");

    let created = core
        .cas_task_create(Parameters(basic_create("Origin stamped on create", None)))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let store = open_task_store(&cas_dir).expect("task store");
    assert_eq!(
        store
            .get(&task_id)
            .expect("created row")
            .origin_project
            .as_deref(),
        Some(project.as_str()),
        "create must stamp the canonical project identity"
    );

    core.cas_task_start(Parameters(IdRequest {
        id: task_id.clone(),
    }))
    .await
    .expect("start must be accepted on the first attempt");
}

/// GH #690 (cas-a0d2): a row with no origin attribution (written by a client
/// that predates origin stamping, or by a still-running older `cas serve`) is
/// listed on this project's board. `start` must therefore adopt it instead of
/// refusing it as an "unassigned legacy row" — that refusal wedged the
/// pixel-hive factory with no MCP-exposed repair for a worker.
#[tokio::test]
async fn start_adopts_an_unattributed_legacy_row_into_the_current_project() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let project = cas::cloud::resolve_canonical_id(&cas_dir).expect("project identity resolves");

    // Opened without a default origin so the NULL column survives the insert.
    let fixture_store = SqliteTaskStore::open(&cas_dir).expect("fixture task store");
    fixture_store.init().expect("fixture task store init");
    let mut legacy = Task::new("cas-lg01".to_string(), "Legacy unattributed task".to_string());
    legacy.origin_project = None;
    fixture_store.add(&legacy).expect("add legacy row");

    let started = core
        .cas_task_start(Parameters(IdRequest {
            id: "cas-lg01".to_string(),
        }))
        .await
        .expect("an unattributed local row must start in its own project");
    assert!(
        extract_text(started).contains("claimed"),
        "start should claim the adopted row"
    );

    let store = open_task_store(&cas_dir).expect("task store");
    assert_eq!(
        store
            .get("cas-lg01")
            .expect("legacy row")
            .origin_project
            .as_deref(),
        Some(project.as_str()),
        "start must persist the adoption so later ownership checks agree"
    );
}

/// The adoption above must not soften the real ownership gate: a row that
/// names a different project is still refused.
#[tokio::test]
async fn start_still_refuses_a_row_owned_by_another_project() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let created = core
        .cas_task_create(Parameters(basic_create("Foreign owned task", None)))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let store = open_task_store(&cas_dir).expect("task store");
    let mut foreign = store.get(&task_id).expect("created row");
    foreign.origin_project = Some("acme/other".to_string());
    store.update(&foreign).expect("mark row foreign");

    let error = core
        .cas_task_start(Parameters(IdRequest { id: task_id }))
        .await
        .expect_err("a row owned by another project must not start here");
    assert!(
        error.message.contains("does not match current project"),
        "unexpected refusal: {}",
        error.message
    );
}

#[tokio::test]
async fn test_task_board_hides_foreign_rows_by_default_and_supports_include_foreign() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[project]\ncanonical_id = \"cas-src\"\n",
    )
    .expect("project identity config should be writable");

    // Use a store opened without a default origin to retain the null-origin
    // fixture row. The MCP core resolves the current project from config.toml.
    let fixture_store = SqliteTaskStore::open(&cas_dir).expect("fixture task store");
    fixture_store.init().expect("fixture task store init");
    let mut null_origin = Task::new("cas-null1".to_string(), "Own null-origin task".to_string());
    null_origin.origin_project = None;
    fixture_store
        .add(&null_origin)
        .expect("add null-origin task");

    let mut own = Task::new("cas-own1".to_string(), "Own explicit task".to_string());
    own.origin_project = Some("cas-src".to_string());
    fixture_store.add(&own).expect("add own task");

    for (id, title) in [
        ("cas-for1", "Foreign ready task one"),
        ("cas-for2", "Foreign ready task two"),
    ] {
        let mut foreign = Task::new(id.to_string(), title.to_string());
        foreign.origin_project = Some("gabber-studio".to_string());
        fixture_store.add(&foreign).expect("add foreign task");
    }

    let service = CasService::new(core, None);
    let default_ready: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "ready",
        "limit": 20,
    }))
    .expect("default ready request");
    let default_ready_text = extract_text(
        service
            .task(Parameters(default_ready))
            .await
            .expect("default ready should succeed"),
    );
    assert!(
        default_ready_text.contains("cas-null1"),
        "null-origin own row hidden: {default_ready_text}"
    );
    assert!(
        default_ready_text.contains("cas-own1"),
        "own row hidden: {default_ready_text}"
    );
    assert!(
        !default_ready_text.contains("cas-for1"),
        "foreign row leaked: {default_ready_text}"
    );
    assert!(
        !default_ready_text.contains("cas-for2"),
        "foreign row leaked: {default_ready_text}"
    );
    assert!(
        default_ready_text.contains("2 foreign-origin tasks hidden (include_foreign=true to show)"),
        "hidden count footer missing: {default_ready_text}"
    );

    let all_ready: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "ready",
        "limit": 20,
        "include_foreign": true,
    }))
    .expect("include_foreign ready request");
    let all_ready_text = extract_text(
        service
            .task(Parameters(all_ready))
            .await
            .expect("include_foreign ready should succeed"),
    );
    for id in ["cas-null1", "cas-own1", "cas-for1", "cas-for2"] {
        assert!(
            all_ready_text.contains(id),
            "include_foreign omitted {id}: {all_ready_text}"
        );
    }
    assert!(
        !all_ready_text.contains("foreign-origin tasks hidden"),
        "opt-in still reports hidden rows: {all_ready_text}"
    );

    let default_list: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "list",
        "limit": 20,
    }))
    .expect("default list request");
    let default_list_text = extract_text(
        service
            .task(Parameters(default_list))
            .await
            .expect("default list should succeed"),
    );
    assert!(
        !default_list_text.contains("cas-for1"),
        "foreign list row leaked: {default_list_text}"
    );
    assert!(
        default_list_text.contains("2 foreign-origin tasks hidden"),
        "list hidden footer missing: {default_list_text}"
    );

    let all_list: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "list",
        "limit": 20,
        "include_foreign": true,
    }))
    .expect("include_foreign list request");
    let all_list_text = extract_text(
        service
            .task(Parameters(all_list))
            .await
            .expect("include_foreign list should succeed"),
    );
    for id in ["cas-null1", "cas-own1", "cas-for1", "cas-for2"] {
        assert!(
            all_list_text.contains(id),
            "include_foreign list omitted {id}: {all_list_text}"
        );
    }
    assert!(
        !all_list_text.contains("foreign-origin tasks hidden"),
        "include_foreign list still reports hidden rows: {all_list_text}"
    );

    let show_foreign: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "show",
        "id": "cas-for1",
        "with_deps": false,
    }))
    .expect("show foreign request");
    let shown = service
        .task(Parameters(show_foreign))
        .await
        .expect("show foreign task");
    assert!(
        extract_text(shown)
            .contains("Origin project: gabber-studio — this task is owned elsewhere"),
        "foreign ownership banner missing"
    );
}

/// cas-06f9 (GH #104): `ready` capped at 10 with a header that printed only
/// the shown count, so a capped list was indistinguishable from a drained
/// queue — and the default ordering was creation order, so low-priority
/// follow-ups created later could fill the window while P0s sat unseen. The
/// reported incident: 30 ready tasks rendered as 10, thirteen ready P0s hidden
/// behind P2/P3 work for hours.
#[tokio::test]
async fn test_task_ready_is_priority_sorted_and_states_the_true_total() {
    let (_temp, service) = setup_cas();

    // Ordering of creation is load-bearing: the OLD default was created/DESC
    // (newest first), so the P0s must be the OLDEST tasks for creation order to
    // push them out of the window. Creating them last would have let the old
    // code surface them and the test would pass against the bug.
    for i in 0..4 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("critical {i}"),
                description: None,
                priority: 0,
                task_type: "bug".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }
    // Enough newer, lower-priority work to fill the 10-row window on its own.
    for (priority, label) in [(2u8, "medium"), (3, "late follow-up")] {
        for i in 0..9 {
            service
                .cas_task_create(Parameters(TaskCreateRequest {
                    depth: None,
                    title: format!("{label} {i}"),
                    description: None,
                    priority,
                    task_type: "task".to_string(),
                    labels: None,
                    notes: None,
                    blocked_by: None,
                    design: None,
                    acceptance_criteria: None,
                    external_ref: None,
                    assignee: None,
                    demo_statement: None,
                    execution_note: None,
                    epic: None,
                }))
                .await
                .expect("create should succeed");
        }
    }

    let text = extract_text(
        service
            .cas_task_ready(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: None, // the reported call: no limit passed
                sort: None,
                sort_order: None,
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_ready should succeed"),
    );

    // The header must state what was withheld, not just what was shown.
    assert!(
        text.contains("showing 10 of 22"),
        "header must carry the true total: {text}"
    );
    assert!(
        text.contains("P0 first"),
        "header must name the ordering applied: {text}"
    );
    assert!(
        text.contains("and 12 more not shown"),
        "footer must say how much is hidden: {text}"
    );
    assert!(
        text.contains("limit=22"),
        "footer must say how to see the rest: {text}"
    );

    // Every P0 must be inside the window — the priority inversion is the
    // damage the truncation actually caused.
    for i in 0..4 {
        assert!(
            text.contains(&format!("critical {i}")),
            "P0 task 'critical {i}' must be visible in the capped window: {text}"
        );
    }
    let first_line = text
        .lines()
        .find(|line| line.starts_with("- ["))
        .expect("at least one task row");
    assert!(
        first_line.contains("P0"),
        "the first row must be a P0: {first_line}"
    );
    assert!(
        !text.contains("late follow-up 8"),
        "P3 work must not displace P0s in the window: {text}"
    );
}

/// cas-e163 (GH #109): `tasks_available` — the surface idle workers are
/// pointed at to self-serve work — caps at 20. Its total was already honest,
/// but nothing said the list had been cut, so a worker could read 20 rows and
/// never learn which call shows the rest.
#[tokio::test]
async fn test_tasks_available_names_withheld_rows() {
    let (_temp, service) = setup_cas();
    for i in 0..25 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("claimable {i}"),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None, // the default cap is what hides rows
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert!(
        text.contains("Available Tasks (25 total, P0 first)"),
        "the honest total stays: {text}"
    );
    assert!(
        text.contains("and 5 more not shown"),
        "the withheld rows must be named: {text}"
    );
    assert!(
        text.contains("limit=25"),
        "the footer must say how to see them: {text}"
    );
}

/// cas-61d3 (GH #111): `sort`/`sort_order` are in this action's schema and
/// were routed all the way into the handler, which ignored them — an agent
/// asking for a different order got the default back with no error and no way
/// to tell. The last of the advertised-but-inert family.
#[tokio::test]
async fn test_tasks_available_honours_an_explicit_sort() {
    let (_temp, service) = setup_cas();
    // Priority order and title order disagree, so the assertion can only pass
    // if the requested field is the one actually applied.
    for (priority, title) in [(0u8, "zulu critical"), (3, "alpha low")] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let by_title = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: Some("title".to_string()),
                sort_order: Some("asc".to_string()),
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );
    assert!(
        by_title.contains("title A-Z"),
        "the header must name the ordering applied: {by_title}"
    );
    let first = by_title
        .lines()
        .find(|l| l.starts_with("[P"))
        .expect("a task row");
    assert!(
        first.contains("alpha low"),
        "sort=title must actually reorder the rows: {first}"
    );

    // Default is unchanged: priority first.
    let by_default = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );
    assert!(by_default.contains("P0 first"), "{by_default}");
    let first_default = by_default
        .lines()
        .find(|l| l.starts_with("[P"))
        .expect("a task row");
    assert!(
        first_default.contains("zulu critical"),
        "the default must stay priority-first: {first_default}"
    );
}

/// cas-61d3 review follow-up: the sort must be applied BEFORE truncation. A
/// regression that sorted only the already-capped window would leave every
/// other test green while making the capped case — the one GH #109 exists for
/// — silently wrong.
#[tokio::test]
async fn test_tasks_available_sorts_before_truncating() {
    let (_temp, service) = setup_cas();
    // Creation order is load-bearing and easy to get backwards: `list_ready`
    // returns priority ASC, created_at DESC, so the NEWEST task is already
    // first. Creating "alpha" last would put it at the top before any sorting
    // and the assertion below would pass even if the sort ran after the cap.
    // Create it FIRST so the natural order leads with "zulu".
    for (priority, title) in [
        (0u8, "alpha critical"),
        (0, "mike critical"),
        (0, "zulu critical"),
    ] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: Some(1),
                scope: "all".to_string(),
                sort: Some("title".to_string()),
                sort_order: Some("asc".to_string()),
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    let rows: Vec<_> = text.lines().filter(|l| l.starts_with("[P")).collect();
    assert_eq!(rows.len(), 1, "limit must still bound the rows: {text}");
    assert!(
        rows[0].contains("alpha critical"),
        "the single surviving row must be the global first, not the first of an \
         unsorted window: {text}"
    );
    assert!(text.contains("and 2 more not shown"), "{text}");
}

/// cas-61d3 review follow-up: `sort_order` alone must behave here exactly as
/// it does on ready/blocked — keep the priority field, flip the direction.
#[tokio::test]
async fn test_tasks_available_sort_order_alone_flips_priority_direction() {
    let (_temp, service) = setup_cas();
    for (priority, title) in [(0u8, "critical one"), (3, "low one")] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: None,
                sort_order: Some("desc".to_string()),
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert!(text.contains("lowest priority first"), "{text}");
    let first = text
        .lines()
        .find(|l| l.starts_with("[P"))
        .expect("a task row");
    assert!(first.contains("low one"), "{first}");
}

/// cas-61d3: an unrecognised sort field means "unspecified" here, exactly as
/// on ready/blocked — it must not silently resurrect creation order.
#[tokio::test]
async fn test_tasks_available_unparseable_sort_falls_back_to_priority() {
    let (_temp, service) = setup_cas();
    // Order matters: the P0 is created FIRST, so a fallback to created/desc
    // (the trap #104 fixed) would put "low one" at the top and the row
    // assertion below would catch it — not just the header label.
    for (priority, title) in [(0u8, "critical one"), (3, "low one")] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: Some("highest".to_string()), // not a valid field
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert!(text.contains("P0 first"), "{text}");
    let first = text
        .lines()
        .find(|l| l.starts_with("[P"))
        .expect("a task row");
    assert!(first.contains("critical one"), "{first}");
}

/// cas-e163 review follow-up: an explicit `limit` must drive the footer too.
/// Without this, a regression that hardcoded the cap and ignored `req.limit`
/// would keep the other tests green while making the footer's own advice
/// ("pass limit=N") a lie.
#[tokio::test]
async fn test_tasks_available_footer_tracks_an_explicit_limit() {
    let (_temp, service) = setup_cas();
    for i in 0..25 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("claimable {i}"),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: Some(5),
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert_eq!(
        text.lines().filter(|l| l.starts_with("[P")).count(),
        5,
        "explicit limit must bound the rows: {text}"
    );
    assert!(
        text.contains("and 20 more not shown"),
        "the withheld count must follow the explicit limit: {text}"
    );
    assert!(text.contains("limit=25"), "{text}");
}

/// cas-e163 review follow-up: the total the footer arithmetic depends on is
/// the post-claim count. A task someone else already holds is not available,
/// and must not inflate either the total or the withheld count.
#[tokio::test]
async fn test_tasks_available_total_excludes_claimed_tasks() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let mut ids = Vec::new();
    for i in 0..25 {
        let created = extract_text(
            service
                .cas_task_create(Parameters(TaskCreateRequest {
                    depth: None,
                    title: format!("claimable {i}"),
                    description: None,
                    priority: 2,
                    task_type: "task".to_string(),
                    labels: None,
                    notes: None,
                    blocked_by: None,
                    design: None,
                    acceptance_criteria: None,
                    external_ref: None,
                    assignee: None,
                    demo_statement: None,
                    execution_note: None,
                    epic: None,
                }))
                .await
                .expect("create should succeed"),
        );
        ids.push(extract_task_id(&created).expect("task id").to_string());
    }

    // Five are already held by another agent. The lease row has a foreign key
    // on the holder, so the holder must be a registered agent.
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let holder = cas::types::Agent::new("other-agent".to_string(), "other-agent".to_string());
    agent_store.register(&holder).expect("register holder");
    for id in ids.iter().take(5) {
        agent_store
            .try_claim(id, "other-agent", 600, Some("held elsewhere"))
            .expect("claim");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert!(
        text.contains("Available Tasks (20 total, P0 first)"),
        "claimed tasks must not inflate the total: {text}"
    );
    assert!(
        !text.contains("more not shown"),
        "20 available under a cap of 20 withholds nothing: {text}"
    );
}

/// cas-e163: a list that fits must not claim anything was withheld.
#[tokio::test]
async fn test_tasks_available_has_no_footer_when_nothing_is_withheld() {
    let (_temp, service) = setup_cas();
    for i in 0..3 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("claimable {i}"),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_tasks_available(Parameters(TaskAvailableRequest {
                limit: None,
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                include_foreign: false,
            }))
            .await
            .expect("tasks_available should succeed"),
    );

    assert!(
        text.contains("Available Tasks (3 total, P0 first)"),
        "{text}"
    );
    assert!(!text.contains("more not shown"), "{text}");
}

/// cas-06f9: `blocked` carried the identical silent cap and creation-order
/// default on the same triage surface. Half the shipped change had no
/// end-to-end coverage, so a revert there would have left the suite green.
#[tokio::test]
async fn test_task_blocked_is_priority_sorted_and_states_the_true_total() {
    let (_temp, service) = setup_cas();

    // One blocker everything depends on, so every other task is Blocked.
    let blocker_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "the blocker".to_string(),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("blocker create"),
    ))
    .expect("blocker id")
    .to_string();

    // P0s oldest, so creation order (newest first) would push them out.
    for i in 0..3 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("critical blocked {i}"),
                description: None,
                priority: 0,
                task_type: "bug".to_string(),
                labels: None,
                notes: None,
                blocked_by: Some(blocker_id.clone()),
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }
    for i in 0..12 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("low blocked {i}"),
                description: None,
                priority: 3,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: Some(blocker_id.clone()),
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_task_blocked(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: None,
                sort: None,
                sort_order: None,
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_blocked should succeed"),
    );

    assert!(
        text.contains("showing 10 of 15"),
        "blocked header must carry the true total: {text}"
    );
    assert!(text.contains("P0 first"), "{text}");
    assert!(
        text.contains("and 5 more not shown"),
        "blocked footer must name the withheld rows: {text}"
    );
    for i in 0..3 {
        assert!(
            text.contains(&format!("critical blocked {i}")),
            "P0 blocked task {i} must be inside the window: {text}"
        );
    }
}

/// cas-06f9 review follow-up: an unrecognised `sort=` must not silently hand
/// back creation order — that is the incident behaviour. Unrecognised means
/// unspecified, and unspecified means priority here.
#[tokio::test]
async fn test_task_ready_unparseable_sort_falls_back_to_priority_not_creation_order() {
    let (_temp, service) = setup_cas();
    for (priority, title) in [(3u8, "low one"), (0, "critical one")] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_task_ready(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: None,
                sort: Some("highest".to_string()), // not a valid sort field
                sort_order: None,
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_ready should succeed"),
    );

    assert!(text.contains("P0 first"), "{text}");
    let first_line = text
        .lines()
        .find(|line| line.starts_with("- ["))
        .expect("a task row");
    assert!(
        first_line.contains("critical one"),
        "an unparseable sort must not resurrect creation order: {first_line}"
    );
}

/// cas-06f9: an uncapped list must not claim to be truncated, and must still
/// name its ordering.
#[tokio::test]
async fn test_task_ready_header_is_plain_when_nothing_is_withheld() {
    let (_temp, service) = setup_cas();
    for i in 0..3 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("task {i}"),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_task_ready(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: None,
                sort: None,
                sort_order: None,
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_ready should succeed"),
    );

    assert!(text.contains("Ready tasks (3, P0 first):"), "{text}");
    assert!(!text.contains("showing"), "{text}");
    assert!(!text.contains("more not shown"), "{text}");
}

/// cas-06f9: an explicit `sort=` still wins, and the header names the ordering
/// actually applied rather than always claiming priority order.
#[tokio::test]
async fn test_task_ready_explicit_sort_overrides_the_priority_default() {
    let (_temp, service) = setup_cas();
    for (priority, title) in [(0u8, "critical one"), (3, "low one")] {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: title.to_string(),
                description: None,
                priority,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create should succeed");
    }

    let text = extract_text(
        service
            .cas_task_ready(Parameters(TaskReadyBlockedRequest {
                scope: "all".to_string(),
                limit: None,
                sort: Some("created".to_string()),
                sort_order: Some("desc".to_string()),
                epic: None,
                include_foreign: false,
            }))
            .await
            .expect("task_ready should succeed"),
    );

    assert!(
        text.contains("newest first"),
        "header must describe the requested ordering, not the default: {text}"
    );
    assert!(!text.contains("P0 first"), "{text}");
    let first_line = text
        .lines()
        .find(|line| line.starts_with("- ["))
        .expect("a task row");
    assert!(
        first_line.contains("low one"),
        "explicit sort must win: {first_line}"
    );
}

/// Regression test for cas-978e: `task action=ready epic=<id>` must return only ready tasks
/// that are children of the specified EPIC; without `epic`, behavior is unchanged.
#[tokio::test]
async fn test_task_ready_epic_filter() {
    let (_temp, service) = setup_cas();

    // Create an epic.
    let epic_result = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Test Epic".to_string(),
            description: None,
            priority: 1,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("epic create should succeed");
    let epic_id = extract_task_id(&extract_text(epic_result))
        .expect("should have epic ID")
        .to_string();

    // Create 2 tasks under the epic.
    for i in 0..2 {
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: format!("Epic subtask {i}"),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: Some(epic_id.clone()),
            }))
            .await
            .expect("subtask create should succeed");
    }

    // Create 1 task NOT under the epic.
    service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Unrelated task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("unrelated task create should succeed");

    // With epic filter: only the 2 subtasks should appear, not the unrelated task.
    let epic_filtered = service
        .cas_task_ready(Parameters(TaskReadyBlockedRequest {
            scope: "all".to_string(),
            limit: Some(20),
            sort: None,
            sort_order: None,
            epic: Some(epic_id.clone()),
            include_foreign: false,
        }))
        .await
        .expect("task_ready with epic filter should succeed");
    let filtered_text = extract_text(epic_filtered);
    assert!(
        filtered_text.contains("Epic subtask"),
        "Epic-filtered ready list must include the epic subtasks: {filtered_text}"
    );
    assert!(
        !filtered_text.contains("Unrelated task"),
        "Epic-filtered ready list must not include tasks outside the epic: {filtered_text}"
    );

    // Without epic filter: all 3 tasks appear (2 subtasks + 1 unrelated).
    let unfiltered = service
        .cas_task_ready(Parameters(TaskReadyBlockedRequest {
            scope: "all".to_string(),
            limit: Some(20),
            sort: None,
            sort_order: None,
            epic: None,
            include_foreign: false,
        }))
        .await
        .expect("task_ready without epic filter should succeed");
    let unfiltered_text = extract_text(unfiltered);
    assert!(
        unfiltered_text.contains("Epic subtask") && unfiltered_text.contains("Unrelated task"),
        "Unfiltered ready list must include all ready tasks: {unfiltered_text}"
    );
}

#[tokio::test]
async fn test_task_delete() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Delete task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Delete task
    let delete_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_task_delete(Parameters(delete_req))
        .await
        .expect("task_delete should succeed");

    let text = extract_text(result);
    assert!(text.contains("Deleted"));
}

#[tokio::test]
async fn test_task_dependencies() {
    let (_temp, service) = setup_cas();

    // Create two tasks
    let req1 = TaskCreateRequest {
        depth: None,
        title: "Blocker task".to_string(),
        description: None,
        priority: 1,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result1 = service
        .cas_task_create(Parameters(req1))
        .await
        .expect("task_create should succeed");

    let text1 = extract_text(result1);
    let blocker_id = extract_task_id(&text1).expect("should have task ID");

    let req2 = TaskCreateRequest {
        depth: None,
        title: "Blocked task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result2 = service
        .cas_task_create(Parameters(req2))
        .await
        .expect("task_create should succeed");

    let text2 = extract_text(result2);
    let blocked_id = extract_task_id(&text2).expect("should have task ID");

    // Add dependency
    let dep_req = DependencyRequest {
        from_id: blocked_id.to_string(),
        to_id: blocker_id.to_string(),
        dep_type: "blocks".to_string(),
    };

    let result = service
        .cas_task_dep_add(Parameters(dep_req))
        .await
        .expect("task_dep_add should succeed");

    let text = extract_text(result);
    assert!(text.contains("dependency") || text.contains("Added") || text.contains("blocks"));

    // List dependencies
    let dep_list_req = IdRequest {
        id: blocked_id.to_string(),
    };
    let result = service
        .cas_task_dep_list(Parameters(dep_list_req))
        .await
        .expect("task_dep_list should succeed");

    let text = extract_text(result);
    assert!(text.contains(blocker_id) || text.contains("blocks"));
}

#[tokio::test]
async fn test_task_show_dependency_direction_labels() {
    let (_temp, service) = setup_cas();

    let blocker = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Direction blocker".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocker create should succeed");
    let blocker_id = extract_task_id(&extract_text(blocker))
        .expect("blocker id")
        .to_string();

    let blocked = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Direction blocked".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: Some(blocker_id.clone()),
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocked create should succeed");
    let blocked_id = extract_task_id(&extract_text(blocked))
        .expect("blocked id")
        .to_string();

    let show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocked_id.clone(),
            with_deps: true,
        }))
        .await
        .expect("task_show should succeed");
    let text = extract_text(show);
    assert!(
        text.contains("Blocked by:") && text.contains(&blocker_id),
        "Blocked task should display inbound blockers clearly: {text}"
    );

    let blocker_show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocker_id.clone(),
            with_deps: true,
        }))
        .await
        .expect("task_show should succeed");
    let blocker_text = extract_text(blocker_show);
    assert!(
        blocker_text.contains("Blocks:") && blocker_text.contains(&blocked_id),
        "Blocker task should show downstream dependent tasks: {blocker_text}"
    );
}

#[tokio::test]
async fn test_close_auto_unblocks_blocked_dependents() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let session_id = format!("test-session-{}", std::process::id());
    let mut agent = agent_store.get(&session_id).expect("test agent");
    agent.role = cas::types::AgentRole::Supervisor;
    agent_store.update(&agent).expect("mark caller supervisor");

    let blocker = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Auto unblock blocker".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocker create should succeed");
    let blocker_id = extract_task_id(&extract_text(blocker))
        .expect("blocker id")
        .to_string();

    let blocked = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Auto unblock dependent".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: Some(blocker_id.clone()),
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocked task create should succeed");
    let blocked_id = extract_task_id(&extract_text(blocked))
        .expect("blocked id")
        .to_string();

    let _ = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: blocked_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("blocked".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("blocked task update should succeed");

    let dispatch = cas_store::create_verification_dispatch(
        &cas_dir,
        &blocker_id,
        &session_id,
        &session_id,
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .expect("create exact supervisor verification dispatch");
    let _ = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: blocker_id.clone(),
            status: "approved".to_string(),
            summary: "approved for close".to_string(),
            confidence: Some(0.9),
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id),
        }))
        .await
        .expect("verification add should succeed");

    let close = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: blocker_id,
            reason: Some("done".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("task close should succeed");
    let close_text = extract_text(close);
    assert!(
        close_text.contains("Auto-unblocked"),
        "Close output should mention auto-unblocked tasks: {close_text}"
    );

    let show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocked_id,
            with_deps: false,
        }))
        .await
        .expect("task_show should succeed");
    let text = extract_text(show);
    assert!(
        text.contains("Status: Open"),
        "Blocked dependent should auto-transition to Open: {text}"
    );
}

#[tokio::test]
async fn test_task_update_invalid_epic_keeps_original_parent_dependency() {
    let (_temp, service) = setup_cas();

    let epic_1 = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Epic 1".to_string(),
            description: None,
            priority: 1,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("epic 1 create should succeed");
    let epic_1_id = extract_task_id(&extract_text(epic_1))
        .expect("epic 1 id")
        .to_string();

    let subtask = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Child task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: Some(epic_1_id.clone()),
        }))
        .await
        .expect("subtask create should succeed");
    let subtask_id = extract_task_id(&extract_text(subtask))
        .expect("subtask id")
        .to_string();

    let update_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: subtask_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: None,
            epic: Some("cas-does-not-exist".to_string()),
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await;
    assert!(
        update_result.is_err(),
        "Invalid epic reassignment should fail"
    );

    let list_result = service
        .cas_task_list(Parameters(TaskListRequest {
            scope: "all".to_string(),
            limit: Some(20),
            status: None,
            task_type: None,
            label: None,
            assignee: None,
            epic: Some(epic_1_id),
            sort: None,
            sort_order: None,
            include_foreign: false,
        }))
        .await
        .expect("task list by epic should succeed");
    let text = extract_text(list_result);
    assert!(
        text.contains(&subtask_id),
        "Original ParentChild dependency should be preserved on failed reassignment: {text}"
    );
}

#[tokio::test]
async fn test_task_update_surfaces_epic_dependency_delete_failure() {
    let (temp, service) = setup_cas();

    let epic = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Epic".to_string(),
            description: None,
            priority: 1,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("epic create should succeed");
    let epic_id = extract_task_id(&extract_text(epic))
        .expect("epic id")
        .to_string();

    let subtask = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Subtask".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: Some(epic_id),
        }))
        .await
        .expect("subtask create should succeed");
    let subtask_id = extract_task_id(&extract_text(subtask))
        .expect("subtask id")
        .to_string();

    let db_path = temp.path().join(".cas").join("cas.db");
    let conn = Connection::open(&db_path).expect("open sqlite db");
    conn.execute(
        "CREATE TRIGGER fail_dependency_delete
         BEFORE DELETE ON dependencies
         BEGIN
             SELECT RAISE(FAIL, 'forced dependency delete failure');
         END;",
        [],
    )
    .expect("create delete failure trigger");

    let update_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: subtask_id,
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: None,
            epic: Some(String::new()),
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await;
    assert!(
        update_result.is_err(),
        "Dependency delete failure should be returned to caller"
    );
}

#[tokio::test]
async fn test_subtask_start_auto_starts_epic() {
    let (_temp, service) = setup_cas();

    // Create an epic
    let epic_req = TaskCreateRequest {
        depth: None,
        title: "Test Epic".to_string(),
        description: Some("An epic with subtasks".to_string()),
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(epic_req))
        .await
        .expect("epic create should succeed");

    let text = extract_text(result);
    let epic_id = extract_task_id(&text).expect("should have epic ID");

    // Verify epic is NOT in progress
    let show_req = TaskShowRequest {
        id: epic_id.to_string(),
        with_deps: false,
    };
    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task show should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("open") || text.contains("Open"),
        "Epic should be open initially: {text}"
    );

    // Create a subtask linked to the epic
    let subtask_req = TaskCreateRequest {
        depth: None,
        title: "Subtask 1".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: Some(epic_id.to_string()),
    };

    let result = service
        .cas_task_create(Parameters(subtask_req))
        .await
        .expect("subtask create should succeed");

    let text = extract_text(result);
    let subtask_id = extract_task_id(&text).expect("should have subtask ID");

    // Start the subtask - this should auto-start the epic
    let start_req = IdRequest {
        id: subtask_id.to_string(),
    };
    let result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("subtask start should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("EPIC OWNERSHIP"),
        "Should show epic ownership message: {text}"
    );
    assert!(text.contains(epic_id), "Should reference epic ID: {text}");
    assert!(
        text.contains("auto-started"),
        "Should indicate epic was auto-started: {text}"
    );
    // Workflow guidance should be included when starting a task
    assert!(
        text.contains("Workflow Guidance"),
        "Task start should include workflow guidance: {text}"
    );
    assert!(
        text.contains("mcp__cas__search"),
        "Workflow guidance should mention CAS search: {text}"
    );

    // Verify the epic is now in progress
    let show_req2 = TaskShowRequest {
        id: epic_id.to_string(),
        with_deps: false,
    };
    let result = service
        .cas_task_show(Parameters(show_req2))
        .await
        .expect("task show should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("in_progress") || text.contains("InProgress") || text.contains("In Progress"),
        "Epic should be in progress after subtask start: {text}"
    );
}

// ============================================================================
// cas-6945: `task action=start` must default `assignee` to the starting
// agent's display name when unset, so the TUI's epic-focus inference gate
// (task_assigned_to_session_agent) can adopt the epic without the supervisor
// manually running `task action=update assignee=<worker>`.
// ============================================================================

#[tokio::test]
async fn test_task_start_sets_assignee_when_unset() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Unassigned task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task create should succeed");
    let create_text = extract_text(result);
    let task_id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.to_string(),
        }))
        .await
        .expect("task start should succeed");

    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");
    let task = task_store.get(&task_id).expect("task should exist");
    assert_eq!(
        task.assignee.as_deref(),
        Some("test-agent"),
        "Starting an unassigned task should set assignee to the starting agent's \
         display name (test-agent)"
    );
}

#[tokio::test]
async fn test_task_start_preserves_existing_assignee() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Pre-assigned task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: Some("other-worker".to_string()),
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task create should succeed");
    let create_text = extract_text(result);
    let task_id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    // Started by the "test-agent" session — must NOT clobber the existing
    // "other-worker" assignee.
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.to_string(),
        }))
        .await
        .expect("task start should succeed");

    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");
    let task = task_store.get(&task_id).expect("task should exist");
    assert_eq!(
        task.assignee.as_deref(),
        Some("other-worker"),
        "Starting a pre-assigned task must preserve the existing assignee, not \
         overwrite it with the starting agent"
    );
}

// ============================================================================
// cas-3558: code-level half of the self-dispatch guard. A worker starting a
// task that is explicitly assigned to a *different* agent must be rejected
// — this is what should have stopped the grok factory run from grabbing
// ready-but-unassigned-to-it tickets that already belonged to someone else.
// Standard/interactive sessions are exempt (see next test).
// ============================================================================

#[tokio::test]
async fn test_worker_cannot_start_task_assigned_to_other_worker() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    // Promote the test session to Worker role.
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|a| a.name == "test-agent")
            .expect("test agent exists");
        agent.role = cas::types::AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Assigned to someone else".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: Some("other-worker".to_string()),
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task create should succeed");
    let create_text = extract_text(result);
    let task_id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    let err = service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.to_string(),
        }))
        .await
        .expect_err("worker starting another worker's assigned task must be rejected");

    assert!(
        err.message.contains("other-worker") && err.message.contains("not you"),
        "error should name the real assignee and explain this agent isn't it: {}",
        err.message
    );

    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");
    let task = task_store.get(&task_id).expect("task should exist");
    assert_eq!(
        task.status,
        cas::types::TaskStatus::Open,
        "rejected start must not flip status to InProgress"
    );
}

#[tokio::test]
async fn test_standard_agent_can_start_task_assigned_to_other_worker() {
    // Standard/interactive sessions (not factory workers) are exempt from
    // the cas-3558 assignee guard — only `AgentRole::Worker` self-dispatch
    // is the problem this guards against.
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Assigned to someone else, standard session".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: Some("other-worker".to_string()),
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task create should succeed");
    let create_text = extract_text(result);
    let task_id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.to_string(),
        }))
        .await
        .expect("standard-role session should not be blocked by the worker-only guard");

    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");
    let task = task_store.get(&task_id).expect("task should exist");
    assert_eq!(task.assignee.as_deref(), Some("other-worker"));
}

// ============================================================================
// cas-5572 (EPIC cas-9508): Spawn-time `action=mine` race regression
//
// Reproduces the factory-session friction described in
// docs/requests/BUG-factory-session-observations-2026-04-22.md §1: after
// `coordination spawn_workers` + `task update assignee=<worker-name>`, a
// freshly-spawned worker's first `action=mine` call was returning "no open
// tasks" even when `task show` on the supervisor side immediately confirmed
// the assignment.
//
// Root cause: `cas_tasks_mine` previously matched only `assignee == agent_id
// || agent_name` where `agent_name` was read from the agent-store row. When
// the worker's agent row has not yet been populated with the final friendly
// name — or the lookup transiently falls back to `agent_id` — the filter
// missed name-based assignments. The fix widens the match to also consider
// `CAS_AGENT_NAME` / `CAS_SESSION_ID` env vars and compares case-insensitively
// on trimmed values.
// ============================================================================

#[tokio::test]
async fn test_task_mine_matches_env_worker_name_during_spawn_race() {
    let (_temp, service) = setup_cas();

    // Simulate the spawn-race condition: the agent-store row still shows
    // the default "test-agent" name, but the supervisor has already assigned
    // the task to the worker's *friendly* name (e.g. "warm-gopher-85"). In
    // the real factory flow the friendly name arrives via CAS_AGENT_NAME in
    // the worker process's env.
    let worker_name = "warm-gopher-85";

    // Acquire the env lock since we're mutating CAS_AGENT_NAME.
    let _env_guard = env_test_lock();
    let prev_name = std::env::var("CAS_AGENT_NAME").ok();
    // SAFETY: env lock is held for the duration of this test body.
    unsafe {
        std::env::set_var("CAS_AGENT_NAME", worker_name);
    }

    // Create a task, then update its assignee to the worker's friendly name —
    // exactly what a supervisor does via `task update assignee=<worker-name>`.
    let create_req = TaskCreateRequest {
        depth: None,
        title: "Spawn-race assignment".to_string(),
        description: None,
        priority: 1,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let created = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let update_req = TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: id.clone(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: Some(worker_name.to_string()),
        status: None,
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    };
    service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    // The worker's first `action=mine` poll — the assignee on the task row
    // is the friendly `worker_name`, but the agent_store row still carries
    // the default "test-agent" name from setup_cas(). Before the fix this
    // returned "No open tasks"; after the fix, CAS_AGENT_NAME bridges the
    // gap and the task surfaces.
    let mine_req = LimitRequest {
        limit: Some(20),
        scope: "all".to_string(),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_tasks_mine(Parameters(mine_req))
        .await
        .expect("tasks_mine should succeed");
    let text = extract_text(result);

    // Restore env before any assertion to avoid poisoning sibling tests on
    // panic. SAFETY: still holding env lock.
    unsafe {
        match prev_name {
            Some(v) => std::env::set_var("CAS_AGENT_NAME", v),
            None => std::env::remove_var("CAS_AGENT_NAME"),
        }
    }

    assert!(
        text.contains(&id),
        "cas_tasks_mine must surface tasks assigned by friendly worker-name \
         (via CAS_AGENT_NAME env) even when the agent-store row still holds \
         the default name. Got: {text}"
    );
    assert!(
        !text.starts_with("No open tasks"),
        "cas_tasks_mine should not report empty during spawn-race window. Got: {text}"
    );
}

// ============================================================================
// cas-1a7c (EPIC cas-9508): task lease + status divergence recovery.
//
// Acceptance criteria:
//   - `action=release` on a lease-less InProgress task clears status to open
//     with an audit trail.
//   - `action=reset` verb exists and is tested for dead-session recovery.
//   - `action=show` called immediately after `action=update` reflects the
//     updated status.
// ============================================================================

#[tokio::test]
async fn test_release_active_started_task_resets_status_to_open_and_ready() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Started task released back to ready".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start should create an active lease");

    let released = service
        .cas_task_release(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
            force: None,
        }))
        .await
        .expect("active lease release should succeed");
    let release_text = extract_text(released);
    assert!(
        release_text.contains("status reset") && release_text.contains("Open"),
        "release must report the lifecycle transition: {release_text}"
    );

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show after release");
    let show_text = extract_text(shown);
    assert!(
        show_text.contains("Status: Open"),
        "started task must return to Open after release: {show_text}"
    );

    let ready = service
        .cas_task_ready(Parameters(TaskReadyBlockedRequest {
            scope: "all".to_string(),
            limit: Some(20),
            sort: None,
            sort_order: None,
            epic: None,
            include_foreign: false,
        }))
        .await
        .expect("ready after release");
    let ready_text = extract_text(ready);
    assert!(
        ready_text.contains(&id),
        "released task must re-enter the ready pool: {ready_text}"
    );
}

#[tokio::test]
async fn test_release_autorecovers_lease_less_in_progress_task() {
    let (_temp, service) = setup_cas();

    // Seed: create task and move it to InProgress without a live lease
    // (simulating a dead-session orphan where status diverged from lease).
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Orphaned in-progress".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: Some("dead-worker".to_string()),
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("status update");

    // Call release — no active lease exists for this agent, and the task is
    // InProgress. The handler must auto-recover instead of surfacing the raw
    // "No active lease found" error.
    let released = service
        .cas_task_release(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
            force: None,
        }))
        .await
        .expect("release auto-recovery must succeed for lease-less InProgress");
    let text = extract_text(released);
    assert!(
        text.contains("auto-recovered") || text.contains("Released"),
        "release output should acknowledge auto-recovery: {text}"
    );

    // Show must reflect Open status immediately after release.
    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "task must be Open after release auto-recovery: {text}"
    );
    assert!(
        text.contains("auto-recovered") || text.contains("assumed orphaned"),
        "task notes must contain audit trail: {text}"
    );
}

#[tokio::test]
async fn test_release_still_errors_when_no_lease_and_task_already_open() {
    let (_temp, service) = setup_cas();

    // Baseline: no lease, status=Open. Release should NOT silently succeed —
    // there's nothing to recover, surface the underlying error.
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Plain open task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let res = service
        .cas_task_release(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
            force: None,
        }))
        .await;
    assert!(
        res.is_err(),
        "release on a plain Open task without a lease should error"
    );
}

#[tokio::test]
async fn test_reset_clears_lease_assignee_and_forces_open() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Needs reset".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: Some("dead-worker".to_string()),
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("status update");

    let res = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
            force: None,
        }))
        .await
        .expect("reset must succeed");
    let text = extract_text(res);
    assert!(
        text.contains("Reset task"),
        "reset output must confirm: {text}"
    );

    // Show must reflect the reset: Open, no assignee, audit note present.
    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "status must be Open after reset: {text}"
    );
    assert!(
        text.contains("reset:") || text.contains("dead-session"),
        "reset audit note must be present: {text}"
    );
}

#[tokio::test]
async fn test_reset_refuses_closed_task() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: Some("light".to_string()),
            title: "Already closed".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("closed".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("close via update");

    let err = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
            force: None,
        }))
        .await;
    assert!(
        err.is_err(),
        "reset must refuse to operate on closed tasks — use reopen instead"
    );
}

/// cas-1a7c AC3: `action=show` immediately after `action=update` must reflect
/// the updated status. Asserts there's no read-after-write snapshot lag in
/// the MCP task store path.
#[tokio::test]
async fn test_show_after_update_reflects_new_status_without_lag() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Status readback".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    // Move to InProgress.
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update to in_progress");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("InProgress") || text.contains("In Progress") || text.contains("in_progress"),
        "show immediately after update must return InProgress: {text}"
    );

    // Now flip back to Open. Show must reflect Open, not a cached InProgress.
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some("new-worker".to_string()),
            status: Some("open".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update back to open");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "show immediately after update back to open must not return stale InProgress: {text}"
    );
    assert!(
        !text.contains("InProgress") && !text.contains("In Progress"),
        "show output must not contain stale InProgress status after update to Open: {text}"
    );
}

#[tokio::test]
async fn test_task_mine_matches_case_insensitive_and_trimmed() {
    let (_temp, service) = setup_cas();

    // Exercise the defensive matching path: assignee spelled with differing
    // case and surrounding whitespace still matches the current agent.
    let create_req = TaskCreateRequest {
        depth: None,
        title: "Case-trim mine match".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let created = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let update_req = TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: id.clone(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        // The default test-agent name is "test-agent"; assert we still
        // match when the supervisor sprays mixed case + whitespace.
        assignee: Some("  TEST-Agent  ".to_string()),
        status: None,
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    };
    service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("update");

    let mine_req = LimitRequest {
        limit: Some(20),
        scope: "all".to_string(),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_tasks_mine(Parameters(mine_req))
        .await
        .expect("mine");
    let text = extract_text(result);
    assert!(
        text.contains(&id),
        "mine should tolerate case + whitespace drift in assignee: {text}"
    );
}

// =============================================================================
// cas-3ed5: supervisor force-transfer (bypass live-worker lease without shutdown)
// =============================================================================

/// RAII guard that sets CAS_AGENT_ROLE=supervisor for the duration of a test.
struct ScopedSupervisorRole;

impl ScopedSupervisorRole {
    fn enter() -> Self {
        // SAFETY: held under env_test_lock() in all callers.
        unsafe { std::env::set_var("CAS_AGENT_ROLE", "supervisor") }
        Self
    }
}

impl Drop for ScopedSupervisorRole {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("CAS_AGENT_ROLE") }
    }
}

fn make_task_create_req(title: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: None,
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    }
}

/// Happy path: supervisor force-transfers a task claimed by a live worker.
///
/// AC: Supervisor has a documented, supported path to reassign a
/// live-worker-claimed task without shutting the worker down.
/// AC: Audit-log entry surfaces the override action with the supervisor session ID.
#[tokio::test]
async fn test_supervisor_force_transfer_live_worker_task() {
    // setup_cas() creates a "test-agent" (the worker that claims the task).
    let (temp, worker_core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");

    // Register a "target worker" the task will be transferred to.
    let mut target_worker =
        cas::types::Agent::new("target-worker-id".to_string(), "target-worker".to_string());
    target_worker.role = cas::types::AgentRole::Worker;
    target_worker.heartbeat(); // mark alive
    agent_store
        .register(&target_worker)
        .expect("register target worker");

    // Register the supervisor agent that will force-transfer.
    let supervisor_id = "supervisor-session-id".to_string();
    let mut supervisor_agent =
        cas::types::Agent::new(supervisor_id.clone(), "test-supervisor".to_string());
    supervisor_agent.role = cas::types::AgentRole::Supervisor;
    supervisor_agent.heartbeat();
    agent_store
        .register(&supervisor_agent)
        .expect("register supervisor");

    // Create a task under the worker CasCore.
    let create_result = worker_core
        .cas_task_create(Parameters(make_task_create_req(
            "Task held by live worker — supervisor force-transfer test",
        )))
        .await
        .expect("task create should succeed");
    let task_id = extract_task_id(&extract_text(create_result))
        .expect("should have task id")
        .to_string();

    // Simulate a live worker claiming the task directly via agent_store
    // (bypassing the assignee check). This represents the state where a worker
    // has started and claimed its task — the scenario the supervisor must bypass.
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&task_id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("test-session-placeholder".to_string()); // worker "holds" it
    task_store.update(&task).expect("update task to InProgress");

    // The "test-agent" (worker_core) has a session id of the form "test-session-<pid>".
    // Use the actual id from the core setup to simulate the lease.
    let worker_session_id = format!("test-session-{}", std::process::id());
    agent_store
        .try_claim(&task_id, &worker_session_id, 600, Some("worker lease"))
        .expect("worker agent store claim should succeed");

    // Confirm the worker owns the lease.
    let task_before = task_store.get(&task_id).expect("task should exist");
    assert_eq!(
        task_before.status,
        cas::types::TaskStatus::InProgress,
        "task should be InProgress after worker claim"
    );

    // Build a second CasCore acting as the supervisor.
    let supervisor_core = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor_core.set_agent_id_for_testing(supervisor_id.clone());

    // Set the supervisor role env var.
    let _role_guard = ScopedSupervisorRole::enter();

    // Supervisor force-transfers the task to the target worker.
    let transfer_req = TaskTransferRequest {
        task_id: task_id.clone(),
        to_agent: "target-worker-id".to_string(),
        note: Some("Supervisor reassign — rebalancing workload".to_string()),
        supervisor_override: Some(true),
    };
    let result = supervisor_core
        .cas_task_transfer(Parameters(transfer_req))
        .await
        .expect("supervisor force-transfer should succeed");

    let text = extract_text(result);

    // Response must confirm transfer and note the override was used.
    assert!(
        text.contains("Transferred task"),
        "response should confirm transfer: {text}"
    );
    assert!(
        text.contains("SUPERVISOR FORCE-TRANSFER") || text.contains("force-transfer"),
        "response should mention the override: {text}"
    );

    // Task notes must contain the audit entry.
    let task_after = task_store
        .get(&task_id)
        .expect("task should exist after transfer");
    assert!(
        task_after.notes.contains("SUPERVISOR FORCE-TRANSFER"),
        "audit entry must be appended to task notes: {}",
        task_after.notes
    );
    assert!(
        task_after.notes.contains(&supervisor_id),
        "audit entry must include supervisor session ID: {}",
        task_after.notes
    );
    assert!(
        task_after.notes.contains("Supervisor reassign"),
        "handoff note must be preserved: {}",
        task_after.notes
    );

    // Task assignee must be updated to the target worker.
    assert_eq!(
        task_after.assignee.as_deref(),
        Some("target-worker-id"),
        "task assignee must be updated to target worker"
    );
}

/// Negative: non-supervisor callers cannot use supervisor_override=true.
///
/// AC: The override is gated — non-supervisors get an explicit rejection.
#[tokio::test]
async fn test_non_supervisor_cannot_force_transfer() {
    let (temp, worker_core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");

    // Register a second live agent holding a conflicting lease.
    let mut other_worker =
        cas::types::Agent::new("other-worker-id".to_string(), "other-worker".to_string());
    other_worker.role = cas::types::AgentRole::Worker;
    other_worker.heartbeat();
    agent_store
        .register(&other_worker)
        .expect("register other worker");

    // Create a task and have the "other worker" claim it directly via the store.
    let create_result = worker_core
        .cas_task_create(Parameters(make_task_create_req(
            "Task for non-supervisor override rejection test",
        )))
        .await
        .expect("task create should succeed");
    let task_id = extract_task_id(&extract_text(create_result))
        .expect("should have task id")
        .to_string();

    agent_store
        .try_claim(
            &task_id,
            "other-worker-id",
            600,
            Some("other worker holds lease"),
        )
        .expect("other worker claim should succeed");

    // Register a target agent for the transfer destination.
    let mut target =
        cas::types::Agent::new("target-agent-id".to_string(), "target-agent".to_string());
    target.role = cas::types::AgentRole::Worker;
    target.heartbeat();
    agent_store.register(&target).expect("register target");

    // Caller is a plain worker (no supervisor role) — must be rejected.
    // Do NOT set CAS_AGENT_ROLE=supervisor.
    let transfer_req = TaskTransferRequest {
        task_id: task_id.clone(),
        to_agent: "target-agent-id".to_string(),
        note: None,
        supervisor_override: Some(true),
    };
    let result = worker_core
        .cas_task_transfer(Parameters(transfer_req))
        .await;

    // Must return an error (McpError) with a clear rejection message.
    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                msg.contains("supervisor") || msg.contains("CAS_AGENT_ROLE"),
                "rejection message should explain the supervisor requirement: {msg}"
            );
        }
        Ok(ok) => {
            let text = extract_text(ok);
            panic!("expected rejection for non-supervisor override, but got success: {text}");
        }
    }
}

// =============================================================================
// cas-6009: dep_remove honors dep_type — does not silently remove the wrong dep
// =============================================================================

/// Regression: the schema allows only one dependency row per (from_id, to_id)
/// pair. Before the fix, `dep_remove A B dep_type=blocks` would silently delete
/// whatever dep existed — including a ParentChild dep — because dep_type was
/// ignored.  After the fix, `dep_remove` must return a clear error when the
/// existing dep is NOT of the requested type, leaving the dep intact.
#[tokio::test]
async fn test_dep_remove_type_mismatch_does_not_delete_existing_dep() {
    let (_temp, service) = setup_cas();

    let task_a = make_task_create_req("Task A — dep_type mismatch regression");
    let task_b = make_task_create_req("Task B — dep_type mismatch regression");

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(task_a))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();

    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(task_b))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();

    // Add a ParentChild dep from A to B.
    // NOTE: dep_add matches "parent" | "parentchild" (not "parent-child").
    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "parent".to_string(),
        }))
        .await
        .expect("add parent-child dep");

    // Verify the ParentChild dep exists
    let deps_before = extract_text(
        service
            .cas_task_dep_list(Parameters(IdRequest { id: id_a.clone() }))
            .await
            .expect("dep_list before"),
    );
    assert!(
        deps_before.to_lowercase().contains("parent"),
        "parent-child dep should exist: {deps_before}"
    );

    // Attempt to remove a Blocks dep (wrong type) — must fail, not delete the ParentChild
    let result = service
        .cas_task_dep_remove(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await;

    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                msg.contains("No") || msg.contains("not found") || msg.contains("found"),
                "error should explain dep not found: {msg}"
            );
        }
        Ok(ok) => {
            let text = extract_text(ok);
            // If the tool returns a tool-error (not McpError), it should surface the not-found message
            assert!(
                text.contains("No") || text.contains("not found") || text.contains("found"),
                "tool response should surface the not-found error: {text}"
            );
        }
    }

    // ParentChild dep must still be intact — the wrong-type dep_remove must NOT have deleted it
    let deps_after = extract_text(
        service
            .cas_task_dep_list(Parameters(IdRequest { id: id_a.clone() }))
            .await
            .expect("dep_list after"),
    );
    assert!(
        deps_after.to_lowercase().contains("parent"),
        "parent-child dep must survive type-mismatched dep_remove: {deps_after}"
    );
}

/// Regression: dep_remove with a dep_type that does NOT match any existing dep
/// between the pair must return a clear error, not a silent success.
/// Here we add a Related dep and try to remove it as Blocks — must fail.
#[tokio::test]
async fn test_dep_remove_wrong_type_returns_error() {
    let (_temp, service) = setup_cas();

    let req_a = make_task_create_req("Task A — no-dep-found error regression");
    let req_b = make_task_create_req("Task B — no-dep-found error regression");

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req_a))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();

    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req_b))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();

    // Add a Related dep (not Blocks)
    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "related".to_string(),
        }))
        .await
        .expect("add related dep");

    // Attempt to remove a Blocks dep (which doesn't exist) — must fail
    let result = service
        .cas_task_dep_remove(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await;

    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                msg.contains("No") || msg.contains("not found") || msg.contains("found"),
                "error should explain dep not found: {msg}"
            );
        }
        Ok(ok) => {
            let text = extract_text(ok);
            // dep_remove returned a tool-error response rather than McpError
            assert!(
                text.contains("No") || text.contains("not found") || text.contains("found"),
                "tool response should surface the not-found error: {text}"
            );
        }
    }

    // Related dep must still be intact
    let deps_after = extract_text(
        service
            .cas_task_dep_list(Parameters(IdRequest { id: id_a.clone() }))
            .await
            .expect("dep_list after"),
    );
    assert!(
        deps_after.contains("Related") || deps_after.contains("related"),
        "related dep must survive type-mismatched dep_remove: {deps_after}"
    );
}

/// Regression: creating a task with the same ID as both `epic` and `blocked_by`
/// must be rejected — the mixed ParentChild+Blocks scenario is the root cause
/// of the silent dep_remove data-loss bug (cas-6009).
#[tokio::test]
async fn test_create_rejects_blocked_by_same_as_epic() {
    let (_temp, service) = setup_cas();

    // Create an epic first
    let epic_create = TaskCreateRequest {
        depth: None,
        title: "Epic for mixed-dep rejection test".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let epic_text = extract_text(
        service
            .cas_task_create(Parameters(epic_create))
            .await
            .expect("create epic"),
    );
    let epic_id = extract_task_id(&epic_text).expect("epic id").to_string();

    // Attempt to create a child task that is ALSO blocked by the same epic
    let bad_create = TaskCreateRequest {
        depth: None,
        title: "Child blocked by its own epic".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: Some(epic_id.clone()),
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: Some(epic_id.clone()),
    };

    let result = service.cas_task_create(Parameters(bad_create)).await;

    match result {
        Err(e) => {
            let msg = e.message.to_string().to_lowercase();
            assert!(
                msg.contains("blocked") || msg.contains("epic") || msg.contains("child"),
                "rejection should reference the conflict: {msg}"
            );
        }
        Ok(ok) => {
            let text = extract_text(ok);
            // Should not succeed
            panic!("expected rejection when blocked_by == epic, got success: {text}");
        }
    }
}

// ============================================================================
// cas-85bf: Task ownership errors surface worker name (not just UUID)
// ============================================================================

/// When a task is locked by another worker, the "locked by" error must include
/// the holding worker's friendly name alongside the session UUID so the
/// supervisor can identify who has the task without cross-referencing
/// worker_status output.
#[tokio::test]
async fn test_task_start_locked_error_includes_worker_name() {
    use cas::store::open_agent_store;
    use cas::types::{Agent, AgentRole};

    let (temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    // Register a "blocker" worker with a recognizable name.
    const BLOCKER_SESSION: &str = "blocker-session-0000-0000-000000000001";
    const BLOCKER_NAME: &str = "worker-backfill";

    let blocker = Agent::new_with_role(
        BLOCKER_SESSION.to_string(),
        BLOCKER_NAME.to_string(),
        AgentRole::Worker,
    );
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    agent_store.register(&blocker).expect("register blocker");

    // Create a task.
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Locked task for name-in-error test".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    // Have the blocker claim the task directly at store level.
    agent_store
        .try_claim(&id, BLOCKER_SESSION, 600, Some("blocking for test"))
        .expect("blocker claim");

    // Now try to start the same task via the test service — should fail
    // because our test agent doesn't own the lease.
    let start_err = service
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest { id: id.clone() }))
        .await
        .expect_err("start must fail when another agent holds the lease");

    let msg = start_err.message.to_string();
    assert!(
        msg.contains(BLOCKER_NAME),
        "error must contain holder's name '{BLOCKER_NAME}': {msg}"
    );
    assert!(
        msg.contains(BLOCKER_SESSION),
        "error must contain holder's session UUID '{BLOCKER_SESSION}': {msg}"
    );
    assert!(msg.contains("locked"), "error must mention 'locked': {msg}");

    drop(temp);
}

// worker_status UUID surfacing is verified by code inspection + build:
// factory_ops.rs emits "    session: {uuid}" for every active worker entry.
// The format is tested indirectly via the lib unit test
// `test_worker_status_format_includes_session_uuid` in factory_ops.rs.

// =============================================================================
// cas-86c5: alive-worker safety guard on task reset
// =============================================================================

/// Orphaned task (assignee has stale heartbeat) resets without a guard warning.
///
/// AC: `action=reset` on a task whose assignee's heartbeat is older than
/// WORKER_STALE_SECS (30 s) must succeed unconditionally — this is the
/// intended dead-session recovery path.
#[tokio::test]
async fn test_reset_orphaned_task_stale_assignee_succeeds() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");

    // Register a worker with a STALE heartbeat (60 s ago; threshold is 30 s).
    let mut stale_worker =
        cas::types::Agent::new("stale-worker-8c5a".to_string(), "stale-worker".to_string());
    stale_worker.role = cas::types::AgentRole::Worker;
    stale_worker.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(60);
    agent_store
        .register(&stale_worker)
        .expect("register stale worker");

    // Create a task and assign it to the stale worker.
    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "Orphaned task — stale assignee reset guard test (cas-86c5)",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let mut task = task_store.get(&task_id).expect("get task");
    task.assignee = Some("stale-worker-8c5a".to_string());
    task.status = cas::types::TaskStatus::InProgress;
    task_store.update(&task).expect("set stale assignee");

    // Reset without force — must succeed because heartbeat is stale.
    let res = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: task_id.clone(),
            force: None,
        }))
        .await
        .expect("reset must succeed for stale-assignee task");
    let text = extract_text(res);
    assert!(
        text.contains("Reset task"),
        "orphaned-task reset must confirm success; got: {text}"
    );
    assert!(
        !text.contains("SAFETY GUARD"),
        "stale-assignee reset must NOT trigger safety guard; got: {text}"
    );
}

/// Live task with fresh-heartbeat assignee — reset without `force` must warn
/// and leave the task untouched.
///
/// AC: `action=reset` on a task whose assignee's heartbeat is within
/// WORKER_STALE_SECS (30 s) must return an Ok result containing "SAFETY GUARD"
/// and must NOT change the task's status or assignee.
#[tokio::test]
async fn test_reset_alive_worker_task_without_force_returns_safety_guard() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");

    // Register a worker with a FRESH heartbeat (just now).
    let mut alive_worker =
        cas::types::Agent::new("alive-worker-86c5".to_string(), "alive-worker".to_string());
    alive_worker.role = cas::types::AgentRole::Worker;
    alive_worker.heartbeat(); // freshest possible
    agent_store
        .register(&alive_worker)
        .expect("register alive worker");

    // Create a task and assign it to the alive worker.
    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "Live task — alive assignee reset guard test (cas-86c5)",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let mut task = task_store.get(&task_id).expect("get task");
    task.assignee = Some("alive-worker-86c5".to_string());
    task.status = cas::types::TaskStatus::InProgress;
    task_store.update(&task).expect("set alive assignee");

    // Reset without force — must return Ok with SAFETY GUARD text, not an error.
    let res = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: task_id.clone(),
            force: None,
        }))
        .await
        .expect("reset must return Ok (not McpError) with SAFETY GUARD text");
    let text = extract_text(res);
    assert!(
        text.contains("SAFETY GUARD"),
        "reset of alive-worker task must surface SAFETY GUARD warning; got: {text}"
    );
    assert!(
        text.contains("force=true"),
        "SAFETY GUARD message must hint at force=true; got: {text}"
    );

    // Task must NOT have been mutated — still InProgress with the original assignee.
    let task_after = task_store
        .get(&task_id)
        .expect("get task after blocked reset");
    assert_eq!(
        task_after.status,
        cas::types::TaskStatus::InProgress,
        "task status must remain InProgress after blocked reset"
    );
    assert_eq!(
        task_after.assignee.as_deref(),
        Some("alive-worker-86c5"),
        "assignee must be unchanged after blocked reset"
    );
}

/// Live task with fresh-heartbeat assignee + `force=true` — reset must succeed
/// and record a "bypassed" audit note.
///
/// AC: `action=reset force=true` on a live-worker task bypasses the guard,
/// transitions to Open, clears the assignee, and appends an audit note that
/// mentions the bypass.
#[tokio::test]
async fn test_reset_alive_worker_task_with_force_succeeds_and_logs_audit() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");

    // Register a worker with a FRESH heartbeat.
    let mut alive_worker = cas::types::Agent::new(
        "alive-force-worker-86c5".to_string(),
        "alive-force-worker".to_string(),
    );
    alive_worker.role = cas::types::AgentRole::Worker;
    alive_worker.heartbeat();
    agent_store
        .register(&alive_worker)
        .expect("register alive worker");

    // Create a task and assign it to the alive worker.
    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "Force-reset live task test (cas-86c5)",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let mut task = task_store.get(&task_id).expect("get task");
    task.assignee = Some("alive-force-worker-86c5".to_string());
    task.status = cas::types::TaskStatus::InProgress;
    task_store.update(&task).expect("set alive assignee");

    // Reset WITH force=true — must bypass the guard and succeed.
    let res = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: task_id.clone(),
            force: Some(true),
        }))
        .await
        .expect("force reset must succeed");
    let text = extract_text(res);
    assert!(
        text.contains("Reset task"),
        "force reset must confirm success; got: {text}"
    );
    assert!(
        !text.contains("SAFETY GUARD"),
        "force reset must NOT emit safety guard; got: {text}"
    );

    // Task must be Open with assignee cleared.
    let task_after = task_store
        .get(&task_id)
        .expect("get task after force reset");
    assert_eq!(
        task_after.status,
        cas::types::TaskStatus::Open,
        "task must be Open after force reset"
    );
    assert!(
        task_after.assignee.is_none(),
        "assignee must be cleared after force reset"
    );
    // Audit note must mention bypass.
    let notes = &task_after.notes;
    assert!(
        notes.contains("force") || notes.contains("bypassed"),
        "audit note must mention force/bypassed; notes: {notes}"
    );
}

// =============================================================================
// cas-dbbb: factory-mode session UUID → display-name normalization in task.update
// =============================================================================

#[tokio::test]
async fn rejected_assignee_reports_that_the_whole_multi_field_update_was_aborted() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[factory]\nwarn_stale_assignment = true\nblock_stale_assignment = true\nstale_threshold_commits = 1\n",
    )
    .unwrap();

    let upstream = temp.path().join("assignment-upstream");
    let worker_checkout = temp.path().join("assignment-worker");
    std::fs::create_dir_all(&upstream).unwrap();
    let git = |repo: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&upstream, &["init", "-q", "-b", "main"]);
    git(&upstream, &["config", "user.email", "test@test.com"]);
    git(&upstream, &["config", "user.name", "Test"]);
    std::fs::write(upstream.join("seed.txt"), "seed\n").unwrap();
    git(&upstream, &["add", "seed.txt"]);
    git(&upstream, &["commit", "-q", "-m", "seed"]);
    let clone = std::process::Command::new("git")
        .args([
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            worker_checkout.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(clone.status.success());
    std::fs::write(upstream.join("later.txt"), "later\n").unwrap();
    git(&upstream, &["add", "later.txt"]);
    git(&upstream, &["commit", "-q", "-m", "later"]);

    let agent_store = open_agent_store(&cas_dir).unwrap();
    let mut worker = cas::types::Agent::new(
        "stale-worker-session".to_string(),
        "stale-worker".to_string(),
    );
    worker.metadata.insert(
        "clone_path".to_string(),
        worker_checkout.to_string_lossy().into_owned(),
    );
    agent_store.register(&worker).unwrap();

    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "multi-field assignee rejection is explicit",
        )))
        .await
        .unwrap();
    let task_id = extract_task_id(&extract_text(created)).unwrap().to_string();
    unsafe { std::env::set_var("CAS_FACTORY_MODE", "1") }
    let error = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: Some("no-code".to_string()),
            external_ref: None,
            assignee: Some("stale-worker".to_string()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect_err("stale assignment must reject the batch");
    unsafe { std::env::remove_var("CAS_FACTORY_MODE") }

    let message = error.message.to_string();
    assert!(message.contains("TASK UPDATE BATCH ABORTED"), "{message}");
    assert!(
        message.contains("no requested task fields were applied"),
        "{message}"
    );
    assert!(message.contains("execution_note, assignee"), "{message}");
    let stored = open_task_store(&cas_dir).unwrap().get(&task_id).unwrap();
    assert!(stored.execution_note.is_none());
    assert!(stored.assignee.is_none());
}

/// When CAS_FACTORY_MODE is set and a supervisor assigns a task using a
/// worker's session UUID instead of their display name, `task.update` must
/// automatically normalize the assignee to the display name so `task mine`
/// can dispatch correctly.
///
/// Smoke-test evidence (2026-06-30): director auto-prompts were using session
/// IDs as the `assignee=` value. `task.update` silently accepted them, but
/// `task mine` on the target worker returned nothing because it matches against
/// display name / CAS_AGENT_NAME, not agent IDs.
#[tokio::test]
async fn test_factory_mode_normalizes_session_uuid_assignee_to_display_name() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");

    // Register a worker with a distinct session ID and display name.
    const WORKER_SESSION: &str = "sess-uuid-abcdef-1234";
    const WORKER_NAME: &str = "calm-owl";

    let worker = cas::types::Agent::new(WORKER_SESSION.to_string(), WORKER_NAME.to_string());
    agent_store.register(&worker).expect("register worker");

    // Set CAS_FACTORY_MODE so the normalization branch activates.
    // SAFETY: we hold the process-wide env lock for the full test body.
    unsafe { std::env::set_var("CAS_FACTORY_MODE", "1") }

    // Create a task.
    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "UUID-normalization test (cas-dbbb)",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    // Assign by session UUID — mimics a director prompt that used the wrong identifier.
    let update_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some(WORKER_SESSION.to_string()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("task update with session UUID must succeed");

    // Restore env before assertions so a panic doesn't poison sibling tests.
    // SAFETY: still holding env lock.
    unsafe { std::env::remove_var("CAS_FACTORY_MODE") }

    let text = extract_text(update_result);

    // Response must warn that the value was normalized.
    assert!(
        text.to_lowercase().contains("session id") || text.contains("Normalized"),
        "cas-dbbb: update with session UUID must emit normalization warning; got: {text}"
    );
    assert!(
        text.contains(WORKER_NAME),
        "cas-dbbb: normalization warning must include display name '{WORKER_NAME}'; got: {text}"
    );

    // The stored assignee must be the display name, not the session UUID.
    // (task show does not render the assignee field; read from store directly.)
    let task = task_store.get(&task_id).expect("get task after update");
    assert_eq!(
        task.assignee.as_deref(),
        Some(WORKER_NAME),
        "cas-dbbb: stored assignee must be display name '{WORKER_NAME}' after normalization; \
         got: {:?}",
        task.assignee
    );
    assert_ne!(
        task.assignee.as_deref(),
        Some(WORKER_SESSION),
        "cas-dbbb: stored assignee must NOT retain session UUID '{WORKER_SESSION}' after normalization"
    );
}

// =============================================================================
// cas-bf98: empty assignee clear must unassign — never remap to a live worker
// =============================================================================

/// Supervisor used `assignee=""` to clear so a worker would not hold two concurrent
/// tasks. Factory-mode session-id normalization treated `""` as a session id and
/// rewrote it to a live worker display name (e.g. hv-scope). Empty/whitespace must
/// clear the assignee (None), not assign anyone.
#[tokio::test]
async fn test_factory_mode_empty_assignee_clears_without_remapping_to_live_worker() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = cas::store::open_task_store(&cas_dir).expect("open task store");

    // Live worker that must NOT receive a silent reassignment on clear.
    const WORKER_SESSION: &str = "sess-uuid-bf98-hv-scope";
    const WORKER_NAME: &str = "hv-scope";
    const PRIOR_ASSIGNEE: &str = "std-life";

    let worker = cas::types::Agent::new(WORKER_SESSION.to_string(), WORKER_NAME.to_string());
    agent_store.register(&worker).expect("register worker");

    unsafe { std::env::set_var("CAS_FACTORY_MODE", "1") }

    let created = service
        .cas_task_create(Parameters(make_task_create_req(
            "empty-assignee clear (cas-bf98)",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    // Seed a real assignee, then clear with empty string (supervisor intent).
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some(PRIOR_ASSIGNEE.to_string()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("seed assignee");

    let seeded = task_store.get(&task_id).expect("get after seed");
    assert_eq!(
        seeded.assignee.as_deref(),
        Some(PRIOR_ASSIGNEE),
        "precondition: assignee seeded to {PRIOR_ASSIGNEE}"
    );

    let clear_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some(String::new()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("empty assignee update must succeed as clear");

    unsafe { std::env::remove_var("CAS_FACTORY_MODE") }

    let text = extract_text(clear_result);
    assert!(
        !text.contains("Normalized to display name"),
        "cas-bf98: empty assignee must not session-id-normalize; got: {text}"
    );
    assert!(
        !text.contains(WORKER_NAME),
        "cas-bf98: clear must not mention live worker '{WORKER_NAME}'; got: {text}"
    );

    let task = task_store.get(&task_id).expect("get task after clear");
    assert!(
        task.assignee.is_none(),
        "cas-bf98: empty assignee must unassign (None); got {:?}",
        task.assignee
    );
    assert_ne!(
        task.assignee.as_deref(),
        Some(WORKER_NAME),
        "cas-bf98: must never remap empty clear to live worker '{WORKER_NAME}'"
    );
    assert_ne!(
        task.assignee.as_deref(),
        Some(PRIOR_ASSIGNEE),
        "cas-bf98: prior assignee must be cleared"
    );

    // Whitespace-only is also an explicit clear.
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some(PRIOR_ASSIGNEE.to_string()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("re-seed");

    unsafe { std::env::set_var("CAS_FACTORY_MODE", "1") }
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            blocked_by: None,
            depth: None,
            id: task_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some("   \t  ".to_string()),
            status: None,
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("whitespace assignee must clear");
    unsafe { std::env::remove_var("CAS_FACTORY_MODE") }

    let after_ws = task_store
        .get(&task_id)
        .expect("get after whitespace clear");
    assert!(
        after_ws.assignee.is_none(),
        "cas-bf98: whitespace-only assignee must unassign; got {:?}",
        after_ws.assignee
    );
}
