use crate::support::*;
use cas::mcp::CasService;
use cas::mcp::tools::{IdRequest, TaskUpdateRequest};
use cas::store::{open_agent_store, open_task_store};
use cas::types::{
    Agent, AgentRole, TaskStatus, TaskTerminalOutcome, TaskType, WorkerCompletionReceiptInput,
    WorkerDeliveryState,
};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

fn set_test_agent_role(cas_dir: &std::path::Path, role: AgentRole) {
    let store = open_agent_store(cas_dir).unwrap();
    let id = format!("test-session-{}", std::process::id());
    let mut agent = store.get(&id).unwrap();
    agent.role = role;
    store.update(&agent).unwrap();
}

async fn execution_note_repair_cas_1b74(stored: &str, replacement: &str, merged: bool) {
    let mut env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut env);
    let root = temp.path().join(".cas");
    let store = open_task_store(&root).unwrap();
    let mut task = cas::types::Task::new("cas-methodology".into(), "Correct methodology".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("worker".into());
    task.execution_note = Some(stored.into());
    if merged {
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        task.deliverables.files_changed = vec!["src/style.css".into()];
    }
    store.add(&task).unwrap();
    let mut delivery_id = None;
    let dispatch;
    if merged {
        let receipt = cas_store::build_worker_completion_receipt(
            &WorkerCompletionReceiptInput {
                task_id: task.id.clone(), worker_agent_id: "worker".into(),
                repo_selector: "remote:github.com/example/methodology".into(),
                source_branch: "factory/worker".into(), commit_sha: "a".repeat(40),
                merge_base_sha: "b".repeat(40), target_branch: "main".into(),
                target_sha: "c".repeat(40), proof_reference: "reviewed delivery".into(),
                scope_summary: "comment-only CSS delivery".into(), artifact_path: None,
            }, "worker", chrono::Utc::now(),
        );
        let (delivery, bound) = cas_store::create_worker_delivery_with_dispatch(
            &root, &receipt, WorkerDeliveryState::AwaitingMerge, "worker", "supervisor",
            chrono::Utc::now() + chrono::Duration::minutes(10),
        ).unwrap();
        cas_store::transition_worker_delivery(
            &root, &delivery.id, &[WorkerDeliveryState::AwaitingMerge],
            WorkerDeliveryState::Merged, "supervisor", Some("supervisor"),
            None, Some(&"d".repeat(40)), None,
        ).unwrap();
        delivery_id = Some(delivery.id);
        dispatch = bound;
    } else {
        dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, "worker", "supervisor",
            &cas::types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
    }
    let service = CasService::new(core, None);
    let send = |value: serde_json::Value| {
        let service = service.clone();
        async move {
            let request = serde_json::from_value(value).unwrap();
            match service.task(Parameters(request)).await {
                Ok(result) => extract_text(result), Err(error) => error.message.to_string(),
            }
        }
    };
    let ordinary = send(serde_json::json!({
        "action":"update", "id":task.id, "execution_note":replacement,
    })).await;
    assert!(ordinary.contains("DELIVERY PROOF SCOPE LOCKED"), "{ordinary}");
    assert!(ordinary.contains("proof_scope_fix=true"), "{ordinary}");
    if replacement == "no-code" {
        let inline = send(serde_json::json!({
            "action":"close", "id":task.id, "execution_note":"no-code",
            "external_ref":"artifact:methodology-investigation",
        })).await;
        assert!(inline.contains("INLINE NO-CODE INTENT REJECTED") && inline.contains("proof_scope_fix=true"), "{inline}");
    }
    let mut repair = serde_json::json!({
        "action":"update", "id":task.id, "execution_note":replacement,
        "proof_scope_fix":true, "reason":"Reviewed incorrect execution methodology",
    });
    if replacement == "no-code" {
        repair["external_ref"] = serde_json::json!("artifact:methodology-investigation");
    }
    let denied = send(repair.clone()).await;
    assert!(denied.contains("only a live registered supervisor"), "{denied}");
    assert_eq!(store.get(&task.id).unwrap().execution_note, task.execution_note);
    set_test_agent_role(&root, AgentRole::Supervisor);
    let mut missing_reason = repair.clone();
    missing_reason["reason"] = serde_json::json!("");
    assert!(send(missing_reason).await.contains("non-empty reason"));
    let mut unchanged = repair.clone();
    unchanged["execution_note"] = serde_json::json!(stored);
    unchanged.as_object_mut().unwrap().remove("external_ref");
    assert!(send(unchanged).await.contains("unchanged"));
    if replacement == "no-code" {
        let mut local_proof = repair.clone();
        local_proof["external_ref"] = serde_json::json!("/tmp/report.html");
        assert!(send(local_proof).await.contains("local absolute path"));
    }
    let mut bundled = repair.clone();
    bundled["title"] = serde_json::json!("unreviewed scope");
    assert!(send(bundled).await.contains("unrelated field"));
    assert_eq!(cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().state,
        cas::types::VerificationDispatchState::Pending);
    let fixed = send(repair).await;
    assert!(fixed.contains("Corrected proof scope"), "{fixed}");
    let corrected = store.get(&task.id).unwrap();
    assert_eq!(corrected.status, TaskStatus::Open);
    assert_eq!(corrected.execution_note.as_deref(), if replacement.is_empty() { None } else { Some(replacement) });
    assert_eq!(corrected.assignee, task.assignee);
    assert_eq!(corrected.deliverables.factory_branch_anchor, task.deliverables.factory_branch_anchor);
    assert!(corrected.notes.contains("Execution methodology corrected"), "{}", corrected.notes);
    assert_eq!(cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().state,
        cas::types::VerificationDispatchState::Invalidated);
    if let Some(id) = delivery_id {
        let (_, delivery) = cas_store::get_latest_worker_delivery(&root, &task.id).unwrap().unwrap();
        assert_eq!(delivery.id, id);
        assert_eq!(delivery.state, WorkerDeliveryState::Merged);
    } else {
        assert_eq!(corrected.external_ref.as_deref(), Some("artifact:methodology-investigation"));
        let closed = send(serde_json::json!({
            "action":"close", "id":task.id, "reason":"Investigation completed",
            "supervisor_override":true,
        })).await;
        assert!(closed.contains("VERIFICATION REQUIRED"), "{closed}");
        let fresh = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
        assert_ne!(fresh.id, dispatch.id, "the old methodology's proof must not authorize close");
        service.verification(Parameters(serde_json::from_value(serde_json::json!({
            "action":"add", "task_id":task.id, "dispatch_id":fresh.id,
            "status":"approved", "summary":"Reviewed corrected no-code investigation and portable proof",
        })).unwrap())).await.unwrap();
        let closed = send(serde_json::json!({
            "action":"close", "id":task.id, "reason":"Investigation completed",
            "supervisor_override":true,
        })).await;
        assert!(closed.contains("Closed task"), "{closed}");
        assert_eq!(store.get(&task.id).unwrap().status, TaskStatus::Closed);
    }
}

#[tokio::test]
async fn no_code_to_code_merged_methodology_repair_cas_1b74() {
    execution_note_repair_cas_1b74("no-code", "test-first", true).await;
}

#[tokio::test]
async fn clear_no_code_merged_methodology_repair_cas_1b74() {
    execution_note_repair_cas_1b74("no-code", "", true).await;
}

#[tokio::test]
async fn test_first_to_no_code_methodology_repair_cas_1b74() {
    execution_note_repair_cas_1b74("test-first", "no-code", false).await;
}

async fn unified_task(service: &CasService, request: serde_json::Value) -> String {
    let request: cas_mcp::TaskRequest = serde_json::from_value(request).unwrap();
    extract_text(service.task(Parameters(request)).await.unwrap())
}

#[tokio::test]
async fn supervisor_gate_closes_on_decision_and_unblocks_dependent_without_commit() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let service = CasService::new(core.clone(), None);

    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Approve rollout",
            "task_type": "gate"
        }),
    )
    .await;
    let gate_id = extract_task_id(&created).unwrap().to_string();

    let dependent = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Begin rollout",
            "task_type": "task",
            "blocked_by": gate_id
        }),
    )
    .await;
    let dependent_id = extract_task_id(&dependent).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    assert_eq!(store.get(&gate_id).unwrap().task_type, TaskType::Gate);
    assert!(
        store
            .list_ready()
            .unwrap()
            .iter()
            .any(|task| task.id == dependent_id),
        "an open Gate may be prepared in parallel while close remains gated"
    );

    let started = unified_task(
        &service,
        serde_json::json!({"action": "start", "id": gate_id, "brief": true}),
    )
    .await;
    assert!(started.contains("Started task"), "{started}");
    assert_eq!(store.get(&gate_id).unwrap().status, TaskStatus::InProgress);

    // A structured but empty decision note is not a decision.
    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate_id,
            "note_type": "decision",
            "notes": "   "
        }),
    )
    .await;
    let refused = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": gate_id,
            "reason": "promotion approved"
        }),
    )
    .await;
    assert!(
        refused.contains("no non-empty recorded DECISION note"),
        "{refused}"
    );
    let unchanged = store.get(&gate_id).unwrap();
    assert_eq!(unchanged.status, TaskStatus::InProgress);
    assert_eq!(unchanged.terminal_outcome, None);

    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate_id,
            "note_type": "decision",
            "notes": "Approve the staged rollout after the error-budget review."
        }),
    )
    .await;
    let closed = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": gate_id,
            "reason": "promotion approved"
        }),
    )
    .await;
    assert!(closed.contains("Closed task"), "{closed}");

    let persisted = store.get(&gate_id).unwrap();
    assert_eq!(persisted.status, TaskStatus::Closed);
    assert_eq!(
        persisted.terminal_outcome,
        Some(TaskTerminalOutcome::Decision)
    );
    assert!(!persisted.counts_as_delivered());
    assert!(persisted.deliverables.merge_commit.is_none());
    assert!(
        store
            .list_ready()
            .unwrap()
            .iter()
            .any(|task| task.id == dependent_id),
        "Decision-close must satisfy the Gate dependency"
    );
}

#[tokio::test]
async fn gate_start_is_supervisor_only_without_weakening_ordinary_start_protection() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core.clone(), None);
    set_test_agent_role(&cas_dir, AgentRole::Worker);

    let mut gate = cas::types::Task::new("cas-gate-worker-start".into(), "Gate".into());
    gate.task_type = TaskType::Gate;
    store.add(&gate).unwrap();
    let worker_error = core
        .cas_task_start(Parameters(IdRequest {
            id: gate.id.clone(),
        }))
        .await
        .expect_err("a worker must not start a supervisor-owned Gate");
    assert!(
        worker_error
            .message
            .contains("only a live registered supervisor"),
        "{}",
        worker_error.message
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate.id.clone(),
            "note_type": "decision",
            "notes": "Approve the rollout."
        }),
    )
    .await;
    let worker_close = unified_task(
        &service,
        serde_json::json!({"action": "close", "id": gate.id.clone()}),
    )
    .await;
    assert!(
        worker_close.contains("only a live registered supervisor"),
        "{worker_close}"
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let direct_close: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": gate.id.clone(),
        "status": "closed"
    }))
    .unwrap();
    let direct_close_error = core
        .cas_task_update(Parameters(direct_close))
        .await
        .expect_err("direct status update must not bypass the Gate decision classifier");
    assert!(
        direct_close_error.message.contains("GATE UPDATE REJECTED"),
        "{}",
        direct_close_error.message
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    let ordinary = cas::types::Task::new("cas-ordinary-supervisor-start".into(), "Work".into());
    store.add(&ordinary).unwrap();
    let supervisor_error = core
        .cas_task_start(Parameters(IdRequest {
            id: ordinary.id.clone(),
        }))
        .await
        .expect_err("supervisors must still delegate ordinary work");
    assert!(
        supervisor_error
            .message
            .contains("Supervisors cannot start non-epic tasks"),
        "{}",
        supervisor_error.message
    );
    assert_eq!(store.get(&ordinary.id).unwrap().status, TaskStatus::Open);
}

#[tokio::test]
async fn supervisor_can_close_worker_assigned_gate_without_worker_branch_validation() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/cas-gate-worker-fixture.git",
    ]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    let commit = Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(temp.path())
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    assert!(commit.success(), "seed commit should succeed");
    cas::store::known_repos::ensure_host_schema().unwrap();

    let worker = Agent::new_with_role(
        "gate-worker-session".to_string(),
        "gate-worker".to_string(),
        AgentRole::Worker,
    );
    open_agent_store(&cas_dir)
        .unwrap()
        .register(&worker)
        .unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);

    let worker_worktree = cas_dir.join("worktrees").join("gate-worker");
    std::fs::create_dir_all(worker_worktree.parent().unwrap()).unwrap();
    let worker_worktree_string = worker_worktree.to_str().unwrap();
    git(&["worktree", "add", "--detach", worker_worktree_string, "HEAD"]);

    let service = CasService::new(core.clone(), None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "title": "Approve worker delivery",
            "task_type": "gate",
            "assignee": "gate-worker",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main"
        }),
    )
    .await;
    let gate_id = extract_task_id(&created).unwrap().to_string();

    let started = unified_task(
        &service,
        serde_json::json!({"action": "start", "id": gate_id, "brief": true}),
    )
    .await;
    assert!(started.contains("Started task"), "{started}");
    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate_id,
            "note_type": "decision",
            "notes": "Approve the worker delivery after review."
        }),
    )
    .await;

    let closed = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": gate_id,
            "reason": "worker delivery approved"
        }),
    )
    .await;
    assert!(closed.contains("Closed task"), "{closed}");
    assert_eq!(
        open_task_store(&cas_dir).unwrap().get(&gate_id).unwrap().status,
        TaskStatus::Closed
    );
}

#[tokio::test]
async fn assigning_gate_to_worker_warns_that_gate_is_supervisor_owned() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    test_env.set("CAS_FACTORY_MODE", "1");
    let worker = Agent::new_with_role(
        "gate-assignment-worker-session".to_string(),
        "gate-assignment-worker".to_string(),
        AgentRole::Worker,
    );
    open_agent_store(&cas_dir)
        .unwrap()
        .register(&worker)
        .unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);

    let service = CasService::new(core, None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "title": "Warn on worker gate assignment",
            "task_type": "gate"
        }),
    )
    .await;
    let gate_id = extract_task_id(&created).unwrap().to_string();
    let updated = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": gate_id,
            "assignee": "gate-assignment-worker"
        }),
    )
    .await;
    test_env.remove("CAS_FACTORY_MODE");

    assert!(
        updated.contains("Gate task assigned to worker")
            && updated.contains("cannot start or close"),
        "{updated}"
    );
    assert_eq!(
        open_task_store(&cas_dir).unwrap().get(&gate_id).unwrap().assignee,
        Some("gate-assignment-worker".to_string())
    );
}

#[tokio::test]
async fn cas525c_supervisor_proof_scope_fix_reopens_with_decision_not_review_failure() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/cas525c-fixture.git",
    ]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(temp.path())
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    git(&["branch", "release"]);
    cas::store::known_repos::ensure_host_schema().unwrap();

    let service = CasService::new(core.clone(), None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Correct delivery branch",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = store.get(&task_id).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    store.update(&task).unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);

    let fixed = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": task_id,
            "target_branch": "release",
            "proof_scope_fix": true,
            "reason": "The task was created against the wrong integration branch."
        }),
    )
    .await;
    assert!(fixed.contains("Corrected proof scope"), "{fixed}");
    let corrected = store.get(&task_id).unwrap();
    assert_eq!(corrected.status, TaskStatus::Open);
    assert_eq!(
        corrected
            .deliverables
            .work_target
            .as_ref()
            .unwrap()
            .target_branch,
        "release"
    );
    assert!(corrected.notes.contains("DECISION: proof scope corrected"));
    assert!(
        corrected
            .notes
            .contains("no failed-review verdict was recorded")
    );
    assert!(!corrected.notes.contains("changes requested by supervisor"));

    let no_code_created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Clear obsolete code anchor",
            "execution_note": "no-code",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main",
            "confirm_warning": true
        }),
    )
    .await;
    let no_code_id = extract_task_id(&no_code_created).unwrap().to_string();
    let mut no_code = store.get(&no_code_id).unwrap();
    no_code.status = TaskStatus::AwaitingMerge;
    store.update(&no_code).unwrap();

    let cleared = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": no_code_id,
            "target_repo": "",
            "proof_scope_fix": true,
            "reason": "This delivery produces an external report and no longer has code scope."
        }),
    )
    .await;
    assert!(cleared.contains("work target was cleared"), "{cleared}");
    let cleared_task = store.get(&no_code_id).unwrap();
    assert_eq!(cleared_task.status, TaskStatus::Open);
    assert!(cleared_task.deliverables.work_target.is_none());
    assert!(
        cleared_task
            .notes
            .contains("stale code work target was cleared")
    );
    assert!(
        !cleared_task
            .notes
            .contains("changes requested by supervisor")
    );

    // A worktree_merge can land before any worker completion receipt exists.
    // Its observed WorkTarget merge is sufficient for an audited correction;
    // no worker proof or delivery transaction is fabricated.
    let observed_created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "platform",
            "title": "Correct risk on observed merge",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main"
        }),
    )
    .await;
    let observed_id = extract_task_id(&observed_created).unwrap().to_string();
    let mut observed_task = store.get(&observed_id).unwrap();
    observed_task.status = TaskStatus::InProgress;
    store.update(&observed_task).unwrap();
    cas_store::record_observed_delivery_merge(
        &cas_dir,
        &observed_id,
        "factory/worker",
        "release",
        &"a".repeat(40),
        &"b".repeat(40),
        "supervisor-session",
    )
    .unwrap();
    let wrong_target: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": observed_id,
        "risk": "none",
        "proof_scope_fix": true,
        "reason": "A merge on another branch cannot correct this WorkTarget."
    }))
    .unwrap();
    let rejected = service.task(Parameters(wrong_target)).await.unwrap_err();
    assert!(rejected.message.contains("WorkTarget"), "{}", rejected.message);
    assert_eq!(store.get(&observed_id).unwrap().risk, [cas::types::TaskRisk::Platform]);
    cas_store::record_observed_delivery_merge(
        &cas_dir,
        &observed_id,
        "factory/worker",
        "main",
        &"a".repeat(40),
        &"b".repeat(40),
        "supervisor-session",
    )
    .unwrap();
    let corrected_observed = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": observed_id,
            "risk": "none",
            "proof_scope_fix": true,
            "reason": "WorkTarget merge was observed, and platform risk was declared in error."
        }),
    )
    .await;
    assert!(corrected_observed.contains("Corrected proof scope"), "{corrected_observed}");
    assert_eq!(store.get(&observed_id).unwrap().risk, [cas::types::TaskRisk::None]);
    assert!(cas_store::get_latest_worker_delivery(&cas_dir, &observed_id)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn casd86d_supervisor_widens_targets_on_merged_delivery_without_review_failure() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let service = CasService::new(core.clone(), None);

    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "blast-radius",
            "proof_targets": ["protocol"],
            "depth": "light",
            "title": "Widen merged delivery proof"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = store.get(&task_id).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    task.assignee = Some("test-agent".to_string());
    store.update(&task).unwrap();

    let receipt = cas_store::build_worker_completion_receipt(
        &WorkerCompletionReceiptInput {
            task_id: task_id.clone(),
            worker_agent_id: "worker-session".into(),
            repo_selector: "remote:github.com/example/casd86d".into(),
            source_branch: "factory/worker".into(),
            commit_sha: "a".repeat(40),
            merge_base_sha: "b".repeat(40),
            target_branch: "main".into(),
            target_sha: "c".repeat(40),
            proof_reference: "scoped proof".into(),
            scope_summary: "merged proof scope regression".into(),
            artifact_path: None,
        },
        "worker",
        chrono::Utc::now(),
    );
    let delivery = cas_store::create_worker_delivery(
        &cas_dir,
        &receipt,
        WorkerDeliveryState::AwaitingMerge,
        "worker-session",
    )
    .unwrap();
    cas_store::transition_worker_delivery(
        &cas_dir,
        &delivery.id,
        &[WorkerDeliveryState::AwaitingMerge],
        WorkerDeliveryState::Merged,
        "supervisor-session",
        Some("supervisor-session"),
        None,
        Some(&"d".repeat(40)),
        None,
    )
    .unwrap();
    cas_store::transition_worker_delivery(
        &cas_dir,
        &delivery.id,
        &[WorkerDeliveryState::Merged],
        WorkerDeliveryState::CloseReady,
        "supervisor-session",
        Some("supervisor-session"),
        None,
        Some(&"d".repeat(40)),
        None,
    )
    .unwrap();

    // Reproduce a legacy serialized array, including duplicate fragments.
    // This predates the store's JSON normalization and must not block widening.
    let db = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    db.execute(
        "UPDATE tasks SET proof_targets=?1 WHERE id=?2",
        rusqlite::params![r#"["protocol", "protocol"]"#, task_id],
    )
    .unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    for targets in [
        serde_json::json!(["protocol"]),
        serde_json::json!(["queue_and_events", "other"]),
    ] {
        let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
            "action":"update", "id":task_id, "proof_targets":targets,
            "proof_scope_fix":true, "reason":"Regression rejection probe"
        }))
        .unwrap();
        let error = service.task(Parameters(request)).await.unwrap_err();
        assert!(error.message.contains("strictly widen"), "{error}");
        assert_eq!(store.get(&task_id).unwrap().proof_targets, ["protocol"]);
    }
    let fixed = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": task_id,
            "proof_targets": ["protocol", "queue_and_events"],
            "proof_scope_fix": true,
            "reason": "The merged delivery changed queue_and_events but the original proof declaration omitted it."
        }),
    )
    .await;
    assert!(fixed.contains("Corrected proof scope"), "{fixed}");

    let corrected = store.get(&task_id).unwrap();
    assert_eq!(corrected.status, TaskStatus::Open);
    assert_eq!(corrected.proof_targets, ["protocol", "queue_and_events"]);
    assert!(corrected.notes.contains("Proof targets widened"));
    assert!(!corrected.notes.contains("changes requested by supervisor"));
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_dir, &task_id)
            .unwrap()
            .unwrap()
            .1
            .state,
        WorkerDeliveryState::CloseReady,
        "scope correction must preserve the immutable merged delivery fact"
    );

    // The parked delivery is now scoped correctly. The worker can re-close
    // the reopened task without a synthetic request_changes/review-failed
    // record; the immutable merged transaction remains the delivery proof.
    set_test_agent_role(&cas_dir, AgentRole::Worker);
    let reclosed = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": task_id,
            "reason": "The merged delivery is now covered by the corrected proof scope."
        }),
    )
    .await;
    assert!(reclosed.contains("Closed task"), "{reclosed}");
    assert_eq!(store.get(&task_id).unwrap().status, TaskStatus::Closed);
}

#[tokio::test]
async fn cas8d38_supervisor_corrects_platform_risk_after_close_ready_merge() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let service = CasService::new(core, None);

    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "platform",
            "title": "Correct mistaken platform risk"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = store.get(&task_id).unwrap();
    task.status = TaskStatus::InProgress;
    store.update(&task).unwrap();

    let receipt = cas_store::build_worker_completion_receipt(
        &WorkerCompletionReceiptInput {
            task_id: task_id.clone(),
            worker_agent_id: "worker-session".into(),
            repo_selector: "remote:github.com/example/cas8d38".into(),
            source_branch: "factory/worker".into(),
            commit_sha: "a".repeat(40),
            merge_base_sha: "b".repeat(40),
            target_branch: "main".into(),
            target_sha: "c".repeat(40),
            proof_reference: "scoped proof".into(),
            scope_summary: "platform risk correction".into(),
            artifact_path: None,
        },
        "worker",
        chrono::Utc::now(),
    );
    let delivery = cas_store::create_worker_delivery(
        &cas_dir,
        &receipt,
        WorkerDeliveryState::AwaitingMerge,
        "worker-session",
    )
    .unwrap();
    cas_store::transition_worker_delivery(
        &cas_dir,
        &delivery.id,
        &[WorkerDeliveryState::AwaitingMerge],
        WorkerDeliveryState::Merged,
        "supervisor-session",
        Some("supervisor-session"),
        None,
        Some(&"d".repeat(40)),
        None,
    )
    .unwrap();
    cas_store::transition_worker_delivery(
        &cas_dir,
        &delivery.id,
        &[WorkerDeliveryState::Merged],
        WorkerDeliveryState::CloseReady,
        "supervisor-session",
        Some("supervisor-session"),
        None,
        Some(&"d".repeat(40)),
        None,
    )
    .unwrap();

    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let fixed = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": task_id,
            "risk": "none",
            "proof_scope_fix": true,
            "reason": "Platform risk was assigned to this Linux-only change by mistake."
        }),
    )
    .await;
    assert!(fixed.contains("Corrected proof scope"), "{fixed}");
    let corrected = store.get(&task_id).unwrap();
    assert_eq!(corrected.status, TaskStatus::Open);
    assert_eq!(corrected.risk, [cas::types::TaskRisk::None]);
    assert!(corrected.notes.contains("Risk corrected to none"));
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_dir, &task_id)
            .unwrap()
            .unwrap()
            .1
            .state,
        WorkerDeliveryState::CloseReady
    );
}

#[tokio::test]
async fn cas8d38_supervisor_override_closes_platform_mismatch_with_measurement() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let service = CasService::new(core, None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "platform",
            "depth": "light",
            "title": "Close mistaken platform risk"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();

    let ordinary = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": task_id,
            "reason": "Linux-only change has no macOS evidence."
        }),
    )
    .await;
    assert!(ordinary.contains("platform_proof receipt is incomplete"), "{ordinary}");

    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let overridden = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": task_id,
            "supervisor_override": true,
            "reason": "Reviewed the Linux-only scope and accepted the mistaken platform declaration."
        }),
    )
    .await;
    assert!(overridden.contains("Closed task"), "{overridden}");
    let closed = open_task_store(&cas_dir).unwrap().get(&task_id).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed);
    assert!(closed.notes.contains("supervisor waived declared-risk close gate"));
    assert!(closed.notes.contains("platform_proof missing evidence: typed note_type=platform_proof"));
}

#[tokio::test]
async fn casb123_proof_scope_fix_rejects_immutable_merged_delivery() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/casb123-fixture.git",
    ]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(temp.path())
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    git(&["branch", "release"]);
    cas::store::known_repos::ensure_host_schema().unwrap();

    let service = CasService::new(core.clone(), None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Do not rewrite merged delivery",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = store.get(&task_id).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    store.update(&task).unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);

    let receipt = cas_store::build_worker_completion_receipt(
        &WorkerCompletionReceiptInput {
            task_id: task_id.clone(),
            worker_agent_id: "worker-session".into(),
            repo_selector: "remote:github.com/example/casb123-fixture".into(),
            source_branch: "factory/worker".into(),
            commit_sha: "a".repeat(40),
            merge_base_sha: "b".repeat(40),
            target_branch: "main".into(),
            target_sha: "c".repeat(40),
            proof_reference: "proof:cas-b123".into(),
            scope_summary: "merged delivery".into(),
            artifact_path: None,
        },
        "worker",
        chrono::Utc::now(),
    );
    let delivery = cas_store::create_worker_delivery(
        &cas_dir,
        &receipt,
        WorkerDeliveryState::AwaitingMerge,
        "worker-session",
    )
    .unwrap();
    cas_store::transition_worker_delivery(
        &cas_dir,
        &delivery.id,
        &[WorkerDeliveryState::AwaitingMerge],
        WorkerDeliveryState::Merged,
        "supervisor-session",
        Some("supervisor-session"),
        None,
        Some(&"d".repeat(40)),
        None,
    )
    .unwrap();

    let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "target_branch": "release",
        "proof_scope_fix": true,
        "reason": "Wrong target was declared before the completed merge."
    }))
    .unwrap();
    let error = service
        .task(Parameters(request))
        .await
        .expect_err("a merged delivery is immutable proof, not stale proof");
    assert!(
        error.message.contains("merged delivery transaction")
            && error.message.contains("immutable delivery fact"),
        "{}",
        error.message
    );
    assert_eq!(
        store.get(&task_id).unwrap().status,
        TaskStatus::AwaitingMerge,
        "rejected correction must not reopen the task"
    );
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_dir, &task_id)
            .unwrap()
            .unwrap()
            .1
            .state,
        WorkerDeliveryState::Merged,
        "rejected correction must preserve the immutable merge fact"
    );
}

/// GH #1006 item 3 / GH #1353: a worker whose branch sits on a delivery parked
/// for merge on another target starts a new task: the parked branch stays at
/// its tip, untouched, and the new task gets its own per-task branch cut from
/// the task's declared target. Before, start either refused with a rebase
/// remedy or moved the parked branch, so a later push could alter the parked
/// delivery under independent QA.
#[tokio::test]
async fn start_beside_a_branch_parked_for_another_target_preserves_its_tip_gh_1006() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let repo = temp.path();
    let git = |args: &[&str]| -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", "https://github.com/example/gh1006-fixture.git"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    // The new task's target has moved on without the parked delivery.
    git(&["checkout", "-q", "-b", "epic/burn-down"]);
    std::fs::write(repo.join("burn.txt"), "burn-down work\n").unwrap();
    git(&["add", "burn.txt"]);
    git(&["commit", "-q", "-m", "burn-down work"]);
    // The worker's branch holds a delivery parked for merge elsewhere.
    git(&["checkout", "-q", "-b", "factory/test-agent", "main"]);
    std::fs::write(repo.join("hub.css"), ".row{gap:8px}\n").unwrap();
    git(&["add", "hub.css"]);
    git(&["commit", "-q", "-m", "parked hub delivery"]);
    let parked_tip = git(&["rev-parse", "HEAD"]);
    cas::store::known_repos::ensure_host_schema().unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Worker);

    let store = open_task_store(&cas_dir).unwrap();
    let mut parked = cas::types::Task::new("cas-pk06".to_string(), "Parked hub delivery".to_string());
    parked.status = TaskStatus::AwaitingMerge;
    parked.assignee = Some("test-agent".to_string());
    parked.deliverables.factory_branch_anchor = Some(parked_tip.clone());
    store.add(&parked).unwrap();

    let service = CasService::new(core.clone(), None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "risk": "none",
            "title": "Burn-down fix",
            "target_repo": repo.to_str().unwrap(),
            "target_branch": "epic/burn-down",
            "assignee": "test-agent"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();

    let started = core
        .cas_task_start(Parameters(IdRequest { id: task_id.clone() }))
        .await
        .map(extract_text)
        .unwrap_or_else(|error| panic!("start must preserve the parked branch: {}", error.message));
    assert!(started.contains("PARKED BRANCH PRESERVED"), "{started}");
    // The parked task, branch and tip are named as the preserved delivery.
    assert!(
        started.contains(&format!("cas-pk06 stays on factory/test-agent at {parked_tip}")),
        "{started}"
    );
    assert_eq!(
        git(&["rev-parse", "factory/test-agent"]),
        parked_tip,
        "the parked branch keeps its tip"
    );
    assert_eq!(
        git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        format!("factory/test-agent-{task_id}"),
        "the new task works on its own branch"
    );
    assert_eq!(git(&["rev-parse", "HEAD"]), git(&["rev-parse", "epic/burn-down"]));
}

#[tokio::test]
async fn cas7a69_unified_create_and_update_accept_all_proof_target_wire_forms() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let service = CasService::new(core, None);
    let store = open_task_store(&cas_dir).unwrap();
    for (form, input) in [
        serde_json::json!(["cas --lib rules", "cas --lib maintenance_jobs"]),
        serde_json::json!(r#"["cas --lib rules", "cas --lib maintenance_jobs"]"#),
        serde_json::json!("cas --lib rules, cas --lib maintenance_jobs"),
    ]
    .into_iter()
    .enumerate()
    {
        // Distinct titles: the duplicate-task guard rejects a repeated title.
        let title = format!("Proof forms regression, wire form {form}");
        let created = unified_task(&service, serde_json::json!({
            "action":"create", "title":title, "risk":"blast-radius", "proof_targets":input
        })).await;
        let id = extract_task_id(&created).unwrap().to_string();
        assert_eq!(
            store.get(&id).unwrap().proof_targets,
            ["cas --lib rules", "cas --lib maintenance_jobs"]
        );
        unified_task(
            &service,
            serde_json::json!({
                "action":"update", "id":id, "proof_targets":["rules", "maintenance_jobs", "rules"]
            }),
        )
        .await;
        assert_eq!(
            store.get(&id).unwrap().proof_targets,
            ["rules", "maintenance_jobs"]
        );
    }
}

/// cas-4d67: a proof-scope repair changes the integration destination, not
/// the fact that this task still owns its parked, unmerged delivery.
#[tokio::test]
async fn cas_4d67_double_retarget_to_fresh_origin_only_epic_still_requires_merge() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas_as(&mut env, AgentRole::Worker);
    env.set("CAS_AGENT_ROLE", "worker");
    let repo = temp.path();
    let root = repo.join(".cas");
    let remote = tempfile::tempdir().unwrap();
    let git = |at: &std::path::Path, args: &[&str]| {
        let output = Command::new("git").args(args).current_dir(at)
            .env("GIT_AUTHOR_NAME", "Test").env("GIT_AUTHOR_EMAIL", "test@example.test")
            .env("GIT_COMMITTER_NAME", "Test").env("GIT_COMMITTER_EMAIL", "test@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(remote.path(), &["init", "-q", "--bare"]);
    git(repo, &["init", "-q", "-b", "main"]);
    // Canonical project identity plus a real local bare transport, no network.
    git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    let base = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["branch", "epic/old"]);
    git(repo, &["push", "-q", "origin", "main", "epic/old"]);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='cas-4d67-fixture'\n[verification]\nenabled=false\n[qa]\nindependent_pass=false\n").unwrap();
    cas::store::known_repos::ensure_host_schema().unwrap();
    let store = open_task_store(&root).unwrap();
    let mut epic = cas::types::Task::new("cas-4d00".into(), "Retarget parent".into());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/old".into());
    store.add(&epic).unwrap();
    let service = CasService::new(core, None);
    let created = unified_task(&service, serde_json::json!({
        "action":"create", "title":"Retarget delivery", "epic":epic.id,
        "risk":"none", "depth":"light", "target_repo":repo, "target_branch":"epic/old"
    })).await;
    let id = extract_task_id(&created).expect(&created).to_string();
    git(repo, &["checkout", "-q", "-b", &format!("factory/test-agent-{id}")]);
    let started = unified_task(&service, serde_json::json!({"action":"start","id":id})).await;
    assert_eq!(store.get(&id).unwrap().status, TaskStatus::InProgress, "{started}");
    std::fs::write(repo.join("delivery.rs"), "pub fn delivery() {}\n").unwrap();
    git(repo, &["add", "delivery.rs"]);
    git(repo, &["commit", "-q", "-m", &format!("fix({id}): unmerged delivery")]);
    let delivered = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["push", "-q", "origin", "HEAD"]);
    let first = unified_task(&service, serde_json::json!({"action":"close","id":id,"reason":"ready"})).await;
    // pin: MERGE REQUIRED is the public gate heading and worker handoff contract.
    assert!(first.contains("MERGE REQUIRED"), "{first}");
    assert_eq!(store.get(&id).unwrap().status, TaskStatus::AwaitingMerge);
    assert_eq!(store.get(&id).unwrap().deliverables.factory_branch_anchor.as_deref(), Some(delivered.as_str()));

    set_test_agent_role(&root, AgentRole::Supervisor);
    let first_fix = unified_task(&service, serde_json::json!({
        "action":"update", "id":id, "proof_scope_fix":true,
        "target_branch":"epic/new-short-name", "reason":"Move parked delivery after release"
    })).await;
    assert!(first_fix.contains("Corrected proof scope"), "{first_fix}");
    set_test_agent_role(&root, AgentRole::Worker);
    unified_task(&service, serde_json::json!({"action":"start","id":id})).await;
    let target = "epic/new-generated-name";
    git(repo, &["push", "-q", "origin", &format!("{base}:refs/heads/{target}")]);
    assert!(!Command::new("git").args(["show-ref", "--verify", &format!("refs/heads/{target}")])
        .current_dir(repo).output().unwrap().status.success(), "target must exist only on origin");
    epic.branch = Some(target.into());
    store.update(&epic).unwrap();
    set_test_agent_role(&root, AgentRole::Supervisor);
    let second_fix = unified_task(&service, serde_json::json!({
        "action":"update", "id":id, "proof_scope_fix":true,
        "target_branch":target, "reason":"Correct epic-generated target name"
    })).await;
    assert!(second_fix.contains("Corrected proof scope"), "{second_fix}");
    set_test_agent_role(&root, AgentRole::Worker);
    unified_task(&service, serde_json::json!({"action":"start","id":id})).await;
    #[cfg(unix)]
    let notice_socket = {
        use std::io::Write;
        let listener = std::os::unix::net::UnixListener::bind(cas::mcp::socket::socket_path(&root)).unwrap();
        listener.set_nonblocking(true).unwrap();
        // Prove the capture sees writes; final assertions filter the actual
        // epic-completion event, not this test-boundary control.
        let mut control = std::os::unix::net::UnixStream::connect(cas::mcp::socket::socket_path(&root)).unwrap();
        let event = cas::mcp::socket::DaemonEvent::WorkerActivity {
            session_id: "capture-control".into(), event_type: "capture_control".into(),
            description: "capture active".into(), entity_id: Some(epic.id.clone()),
        };
        writeln!(control, "{}", serde_json::to_string(&event).unwrap()).unwrap();
        listener
    };
    let refused = unified_task(&service, serde_json::json!({"action":"close","id":id,"reason":"re-park unchanged tip"})).await;
    assert!(refused.contains("MERGE REQUIRED"), "double retarget must retain merge gate: {refused}");
    assert_eq!(store.get(&id).unwrap().status, TaskStatus::AwaitingMerge);
    assert!(store.get(&id).unwrap().closed_at.is_none());
    assert!(!refused.contains("All subtasks"), "unmerged delivery must not complete its epic: {refused}");
    #[cfg(unix)]
    {
        use std::io::BufRead;
        let mut captured = Vec::new();
        loop {
            match notice_socket.accept() {
                Ok((stream, _)) => {
                    // Events were written synchronously before accept. Darwin
                    // rejects SO_RCVTIMEO once a sender has closed its socket.
                    stream.set_nonblocking(true).unwrap();
                    let mut line = String::new();
                    std::io::BufReader::new(stream).read_line(&mut line).unwrap();
                    captured.push(serde_json::from_str::<cas::mcp::socket::DaemonEvent>(&line).unwrap());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read daemon event: {error}"),
            }
        }
        assert!(captured.iter().any(|event| matches!(event,
            cas::mcp::socket::DaemonEvent::WorkerActivity { event_type, .. } if event_type == "capture_control")));
        assert!(!captured.iter().any(|event| matches!(event,
            cas::mcp::socket::DaemonEvent::WorkerActivity { event_type, entity_id, .. }
            if event_type == "epic_subtasks_complete" && entity_id.as_deref() == Some(epic.id.as_str()))),
            "an unmerged task must emit no parent-completion notice: {captured:?}");
    }
    assert_eq!(git(repo, &["rev-parse", &format!("origin/{target}")]), base);
    assert!(!Command::new("git").args(["merge-base", "--is-ancestor", &delivered, &format!("origin/{target}")])
        .current_dir(repo).status().unwrap().success(), "fixture delivery must remain unmerged");
}
