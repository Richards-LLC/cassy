use crate::support::*;
use cas::mcp::tools::*;
use cas::store::{open_agent_store, open_verification_store};
use cas::types::{AgentStatus, Verification};
use rmcp::ServerHandler;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

/// Helper to create CasService for server protocol tests
fn setup_service() -> (TempDir, CasService) {
    let (temp, core) = setup_cas();
    (temp, CasService::new(core, None))
}

#[test]
fn test_server_info() {
    let (_temp, service) = setup_service();

    let info = service.get_info();

    assert_eq!(info.server_info.name, "cas");
    assert_eq!(
        info.server_info.title,
        Some("Coding Agent System".to_string())
    );
    assert!(info.instructions.is_some());

    let instructions = info.instructions.unwrap();
    assert!(instructions.contains("CAS"));
}

#[test]
fn test_server_info_version() {
    let (_temp, service) = setup_service();

    let info = service.get_info();

    // Version should be set from Cargo.toml
    assert!(!info.server_info.version.is_empty());
}

#[test]
fn test_server_capabilities() {
    let (_temp, service) = setup_service();

    let info = service.get_info();

    // Server should enable tools, resources, and prompts
    assert!(info.capabilities.tools.is_some());
    assert!(info.capabilities.resources.is_some());
    assert!(info.capabilities.prompts.is_some());
}

#[test]
fn test_server_protocol_version() {
    let (_temp, service) = setup_service();

    let info = service.get_info();

    // Should use the expected protocol version
    assert_eq!(info.protocol_version, rmcp::model::ProtocolVersion::LATEST);
}

#[tokio::test]
async fn test_store_operations_work() {
    // Verify the service can open stores (indirectly tests server infrastructure)
    let (_temp, service) = setup_cas();

    // Create an entry to verify store access works
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Store access test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("should be able to access store");

    let text = extract_text(result);
    assert!(text.contains("Created entry"));
}

#[tokio::test]
async fn test_all_store_types_accessible() {
    let (_temp, service) = setup_cas();

    // Test entry store
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Entry store test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        personal: None,
    };

    service
        .cas_remember(Parameters(req))
        .await
        .expect("entry store");

    // Test task store
    let req = TaskCreateRequest {
        depth: None,
        title: "Task store test".to_string(),
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
        .expect("task store");

    // Test rule store
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Rule store test".to_string(),
        paths: None,
        tags: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };
    service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule store");

    // Test skill store
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Skill store test".to_string(),
        description: "Test skill".to_string(),
        invocation: "test".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };
    service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill store");
}

// ========================================================================
// Task-scoped verification isolation tests
// ========================================================================

#[tokio::test]
async fn test_start_allowed_with_other_task_pending_verification() {
    let (_temp, service) = setup_cas();

    // Create and start first task
    let req = TaskCreateRequest {
        depth: None,
        title: "First task".to_string(),
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
    let first_id = extract_task_id(&text).expect("should have task ID");

    // Start first task (claims it)
    let start_req = IdRequest {
        id: first_id.to_string(),
    };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Try to close first task - should get VERIFICATION REQUIRED
    let close_req = TaskCloseRequest {
        id: first_id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Close should be blocked: {text}"
    );

    // Create second task
    let req2 = TaskCreateRequest {
        depth: None,
        title: "Second task".to_string(),
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
        .cas_task_create(Parameters(req2))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let second_id = extract_task_id(&text).expect("should have second task ID");

    // Starting unrelated task B remains available while A's close is pending.
    let start_req2 = IdRequest {
        id: second_id.to_string(),
    };
    let result = service
        .cas_task_start(Parameters(start_req2))
        .await
        .expect("task_start should return result");

    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "Task A must not block starting task B: {text}"
    );
}

#[tokio::test]
async fn test_claim_allowed_with_other_task_pending_verification() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    // Create and start first task
    let req = TaskCreateRequest {
        depth: None,
        title: "First task for claim test".to_string(),
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
    let first_id = extract_task_id(&text).expect("should have task ID");

    // Start first task (claims it)
    let start_req = IdRequest {
        id: first_id.to_string(),
    };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Try to close first task - should get VERIFICATION REQUIRED
    let close_req = TaskCloseRequest {
        id: first_id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Close should be blocked: {text}"
    );

    // Create second task
    let req2 = TaskCreateRequest {
        depth: None,
        title: "Second task for claim test".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: Some(
            open_agent_store(&cas_dir)
                .unwrap()
                .list(Some(AgentStatus::Active))
                .unwrap_or_default()
                .first()
                .map(|agent| agent.id.clone())
                .unwrap_or_default(),
        ),
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req2))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let second_id = extract_task_id(&text).expect("should have second task ID");

    // Claiming unrelated task B remains available while A's close is pending.
    let claim_req = TaskClaimRequest {
        task_id: second_id.to_string(),
        duration_secs: 600,
        reason: Some("Testing".to_string()),
    };
    let result = service
        .cas_task_claim(Parameters(claim_req))
        .await
        .expect("task_claim should return result");

    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "Task A must not block claiming task B: {text}"
    );
}

#[tokio::test]
async fn test_start_allowed_after_verification_approved() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create and start first task
    let req = TaskCreateRequest {
        depth: None,
        title: "First task".to_string(),
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
    let first_id = extract_task_id(&text).expect("should have task ID");

    // Start first task
    let start_req = IdRequest {
        id: first_id.to_string(),
    };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Add approved verification
    let verification = Verification::approved(
        "ver-approved".to_string(),
        first_id.to_string(),
        "All checks passed".to_string(),
    );
    verification_store.add(&verification).unwrap();

    // Create second task
    let req2 = TaskCreateRequest {
        depth: None,
        title: "Second task".to_string(),
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
        .cas_task_create(Parameters(req2))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let second_id = extract_task_id(&text).expect("should have second task ID");

    // Try to start second task - should SUCCEED now that first is verified
    let start_req2 = IdRequest {
        id: second_id.to_string(),
    };
    let result = service
        .cas_task_start(Parameters(start_req2))
        .await
        .expect("task_start should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Started") || text.contains("claimed"),
        "Start should succeed: {text}"
    );
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "Should not be blocked: {text}"
    );
}

#[tokio::test]
async fn test_start_same_task_allowed_when_pending() {
    let (_temp, service) = setup_cas();

    // Create and start task
    let req = TaskCreateRequest {
        depth: None,
        title: "Task to resume".to_string(),
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
    let task_id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest {
        id: task_id.to_string(),
    };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Try to close - should get VERIFICATION REQUIRED
    let close_req = TaskCloseRequest {
        id: task_id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Close should be blocked: {text}"
    );

    // Try to start THE SAME task again - should be ALLOWED (resuming work)
    let start_req2 = IdRequest {
        id: task_id.to_string(),
    };
    let result = service
        .cas_task_start(Parameters(start_req2))
        .await
        .expect("task_start should succeed for same task");

    let text = extract_text(result);
    // Should succeed or indicate already in progress - NOT blocked
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "Same task should not be blocked: {text}"
    );
}

#[tokio::test]
async fn test_task_list_type_filter() {
    let (_temp, service) = setup_cas();

    // Create an epic
    let epic_req = TaskCreateRequest {
        depth: None,
        title: "Test Epic for filtering".to_string(),
        description: None,
        notes: None,
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
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
        .cas_task_create(Parameters(epic_req))
        .await
        .expect("create epic");

    // Create a regular task
    let task_req = TaskCreateRequest {
        depth: None,
        title: "Test Task for filtering".to_string(),
        description: None,
        notes: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
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
        .cas_task_create(Parameters(task_req))
        .await
        .expect("create task");

    // List only epics
    let list_req = TaskListRequest {
        scope: "all".to_string(),
        limit: Some(10),
        status: None,
        task_type: Some("epic".to_string()),
        label: None,
        assignee: None,
        epic: None,
        sort: None,
        sort_order: None,
    };
    let result = service
        .cas_task_list(Parameters(list_req))
        .await
        .expect("list epics");
    let text = extract_text(result);
    assert!(text.contains("epic"), "Should contain epic task type");
    assert!(text.contains("Test Epic"), "Should contain epic title");
    // Should NOT contain the regular task
    assert!(
        !text.contains("Test Task for filtering"),
        "Should NOT contain regular task"
    );
}

#[tokio::test]
async fn test_task_list_epic_filter() {
    let (_temp, service) = setup_cas();

    // Create an epic
    let epic_req = TaskCreateRequest {
        depth: None,
        title: "My Test Epic".to_string(),
        description: Some("An epic to test filtering".to_string()),
        notes: None,
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
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
        .expect("create epic");
    let text = extract_text(result);
    // Extract epic ID from creation result
    let epic_id = text
        .split_whitespace()
        .find(|s| s.starts_with("cas-"))
        .map(|s| s.trim_end_matches([':', ')', '.']))
        .expect("Should find epic ID in output");

    // Create a subtask linked to the epic
    let subtask_req = TaskCreateRequest {
        depth: None,
        title: "Subtask of Epic".to_string(),
        description: None,
        notes: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: Some(epic_id.to_string()),
    };
    service
        .cas_task_create(Parameters(subtask_req))
        .await
        .expect("create subtask");

    // Create an unrelated task (NOT linked to the epic)
    let unrelated_req = TaskCreateRequest {
        depth: None,
        title: "Unrelated Task".to_string(),
        description: None,
        notes: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
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
        .cas_task_create(Parameters(unrelated_req))
        .await
        .expect("create unrelated task");

    // List tasks filtered by epic - should only show subtask
    let list_req = TaskListRequest {
        scope: "all".to_string(),
        limit: Some(10),
        status: None,
        task_type: None,
        label: None,
        assignee: None,
        epic: Some(epic_id.to_string()),
        sort: None,
        sort_order: None,
    };
    let result = service
        .cas_task_list(Parameters(list_req))
        .await
        .expect("list tasks by epic");
    let text = extract_text(result);

    // Should contain the subtask
    assert!(
        text.contains("Subtask of Epic"),
        "Should contain subtask of epic: {text}"
    );
    // Should NOT contain the unrelated task
    assert!(
        !text.contains("Unrelated Task"),
        "Should NOT contain unrelated task: {text}"
    );
    // Should NOT contain the epic itself (only its children)
    assert!(
        !text.contains("My Test Epic"),
        "Should NOT contain the epic itself: {text}"
    );
}
