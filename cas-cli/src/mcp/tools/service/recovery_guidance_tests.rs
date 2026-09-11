//! Recovery hints are commands for the caller, not aliases in user-authored text.
use super::*;
use crate::test_env_guard::TestEnvGuard;

const HARNESSES: [(&str, &str); 4] = [
    ("claude", "mcp__cas__"),
    ("codex", "mcp__cs__"),
    ("grok", "cas__"),
    ("opencode", "cas_"),
];

fn service(harness: &str, role: crate::types::AgentRole) -> (tempfile::TempDir, CasService) {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".cas")).unwrap();
    let core = CasCore::with_daemon(dir.path().join(".cas"), None, None);
    core.register_agent("recovery-caller".into(), "recovery-caller".into(), None)
        .unwrap();
    let store = core.open_agent_store().unwrap();
    let mut agent = store.get("recovery-caller").unwrap();
    agent.role = role;
    agent.metadata.insert("worker_cli".into(), harness.into());
    store.update(&agent).unwrap();
    (dir, CasService::new(core, None))
}

#[tokio::test]
async fn recovery_guidance_missing_parameters_four_harnesses() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    // Registered caller evidence must win over a stale process hint.
    env.set("CAS_FACTORY_WORKER_CLI", "claude");
    for (harness, prefix) in HARNESSES {
        let (_dir, service) = service(harness, crate::types::AgentRole::Worker);
        for action in [
            "show",
            "start",
            "close",
            "notes",
            "claim",
            "reset",
            "transfer",
            "update",
            "reopen",
            "delete",
            "release",
            "dep_add",
            "dep_remove",
            "dep_list",
        ] {
            let error = service
                .task(Parameters(
                    serde_json::from_value(serde_json::json!({"action": action})).unwrap(),
                ))
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.data.is_none());
            assert!(
                error
                    .message
                    .contains(&format!("{prefix}task action={action}")),
                "{harness}/{action}: {error}"
            );
        }
        let error = service
            .memory(Parameters(
                serde_json::from_value(serde_json::json!({"action": "get"})).unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{prefix}memory action=get")),
            "{error}"
        );
        assert!(!error.message.contains("task ID"), "{error}");
        let error = service
            .coordination(Parameters(
                serde_json::from_value(serde_json::json!({"action": "epic_status"})).unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{prefix}coordination action=epic_status")),
            "{error}"
        );
    }
}

fn task(service: &CasService, id: &str, kind: crate::types::TaskType) {
    let mut task = crate::types::Task::new(id.into(), "literal mcp__cas__task must stay".into());
    task.task_type = kind;
    task.assignee = Some("recovery-caller".into());
    service.inner.open_task_store().unwrap().add(&task).unwrap();
}

fn text(result: CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn recovery_guidance_start_success_and_delegation_four_harnesses() {
    let mut env = TestEnvGuard::temp_home();
    for (harness, prefix) in HARNESSES {
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_FACTORY_WORKER_CLI", harness);
        let (_dir, service) = service(harness, crate::types::AgentRole::Worker);
        task(&service, "cas-recovery", crate::types::TaskType::Task);
        let result = service
            .task(Parameters(
                serde_json::from_value(
                    serde_json::json!({"action": "start", "id": "cas-recovery"}),
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let output = text(result);
        for tool in ["search", "task", "memory"] {
            assert!(
                output.contains(&format!("{prefix}{tool}")),
                "{harness}/{tool}: {output}"
            );
        }
        assert!(
            output.contains("literal mcp__cas__task must stay"),
            "{output}"
        );

        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_FACTORY_SUPERVISOR_CLI", harness);
        env.set("CAS_FACTORY_WORKER_CLI", "claude");
        let (_dir, service) = self::service(harness, crate::types::AgentRole::Supervisor);
        task(&service, "cas-refusal", crate::types::TaskType::Task);
        let error = service
            .task(Parameters(
                serde_json::from_value(serde_json::json!({"action": "start", "id": "cas-refusal"}))
                    .unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("Supervisors cannot start"),
            "{error}"
        );
        assert!(
            error
                .message
                .contains(&format!("{prefix}coordination action=message")),
            "{error}"
        );
        assert!(
            error.message.contains("summary="),
            "delegation must be executable: {error}"
        );
        assert!(
            error.message.contains("task_id="),
            "spawn must name work: {error}"
        );
    }
}

#[tokio::test]
async fn recovery_guidance_gate_decision_four_harnesses() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_FACTORY_WORKER_CLI", "claude");
    for (harness, prefix) in HARNESSES {
        env.set("CAS_FACTORY_SUPERVISOR_CLI", harness);
        let (_dir, service) = service(harness, crate::types::AgentRole::Supervisor);
        task(&service, "cas-decision", crate::types::TaskType::Gate);
        let result = service
            .task(Parameters(
                serde_json::from_value(
                    serde_json::json!({"action": "close", "id": "cas-decision"}),
                )
                .unwrap(),
            ))
            .await;
        let output = match result {
            Ok(result) => text(result),
            Err(error) => error.message.to_string(),
        };
        assert!(output.contains("GATE CLOSE REJECTED"), "{output}");
        assert!(
            output.contains(&format!("{prefix}task action=notes")),
            "{output}"
        );
        assert_eq!(
            service
                .inner
                .open_task_store()
                .unwrap()
                .get("cas-decision")
                .unwrap()
                .status,
            crate::types::TaskStatus::Open
        );
    }
}

#[tokio::test]
async fn recovery_guidance_unknown_caller_uses_own_role_fallback() {
    let mut env = TestEnvGuard::temp_home();
    for (role, own, other, expected) in [
        ("supervisor", "codex", "claude", "mcp__cs__"),
        ("worker", "grok", "codex", "cas__"),
        ("worker", "unknown", "codex", "mcp__cas__"),
    ] {
        env.set("CAS_AGENT_ROLE", role);
        env.set(
            "CAS_FACTORY_WORKER_CLI",
            if role == "worker" { own } else { other },
        );
        env.set(
            "CAS_FACTORY_SUPERVISOR_CLI",
            if role == "supervisor" { own } else { other },
        );
        let dir = tempfile::TempDir::new().unwrap();
        let service = CasService::new(
            CasCore::with_daemon(dir.path().join(".cas"), None, None),
            None,
        );
        let error = service
            .task(Parameters(
                serde_json::from_value(serde_json::json!({"action": "show"})).unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{expected}task action=show")),
            "{error}"
        );
    }
}

#[tokio::test]
async fn recovery_guidance_named_recipient_is_not_the_callers_harness() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("codex")),
        ("CAS_FACTORY_WORKER_CLI", Some("claude")),
    ]);
    let (_dir, service) = service("claude", crate::types::AgentRole::Supervisor);
    let store = service.inner.open_agent_store().unwrap();
    let mut worker = crate::types::Agent::new("other-id".into(), "other-worker".into());
    worker.role = crate::types::AgentRole::Worker;
    for (harness, prefix) in HARNESSES {
        worker.metadata.insert("worker_cli".into(), harness.into());
        store.register(&worker).unwrap();
        assert_eq!(
            service.inner.recipient_guidance_prefix("other-worker"),
            Some(prefix)
        );
        assert_eq!(
            service.inner.recipient_guidance_prefix("other-id"),
            Some(prefix)
        );
    }
    worker
        .metadata
        .insert("worker_cli".into(), "unknown".into());
    store.update(&worker).unwrap();
    assert_eq!(
        service.inner.recipient_guidance_prefix("other-worker"),
        None
    );
    assert_eq!(service.inner.recipient_guidance_prefix("absent"), None);
    // The caller still receives its own executable recovery action.
    let error = service
        .task(Parameters(
            serde_json::from_value(serde_json::json!({"action":"show"})).unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(
        error.message.contains("mcp__cs__task action=show"),
        "{error}"
    );
}

#[tokio::test]
async fn recovery_guidance_claim_and_message_errors_four_harnesses() {
    let mut env = TestEnvGuard::temp_home();
    for (harness, prefix) in HARNESSES {
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_FACTORY_WORKER_CLI", "claude");
        let (_dir, worker) = service(harness, crate::types::AgentRole::Worker);
        task(&worker, "cas-claim", crate::types::TaskType::Task);
        let result = worker
            .task(Parameters(
                serde_json::from_value(serde_json::json!({"action":"claim", "id":"cas-claim"}))
                    .unwrap(),
            ))
            .await
            .unwrap();
        let output = text(result);
        assert!(output.contains("Task claimed"), "{output}");
        assert!(
            output.contains(&format!("{prefix}task action=start")),
            "{output}"
        );
        assert!(
            output.contains(&format!("{prefix}memory action=remember")),
            "{output}"
        );
        let error = worker
            .coordination(Parameters(
                serde_json::from_value(
                    serde_json::json!({"action":"message", "target":"supervisor"}),
                )
                .unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{prefix}coordination action=message")),
            "{error}"
        );
        assert!(error.message.contains("summary="), "{error}");

        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_FACTORY_SUPERVISOR_CLI", harness);
        let (_dir, supervisor) = service("claude", crate::types::AgentRole::Supervisor);
        task(
            &supervisor,
            "cas-refuse-claim",
            crate::types::TaskType::Task,
        );
        let error = supervisor
            .task(Parameters(
                serde_json::from_value(
                    serde_json::json!({"action":"claim", "id":"cas-refuse-claim"}),
                )
                .unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("Supervisors cannot claim"),
            "{error}"
        );
        assert!(
            error.message.contains(&format!(
                "{prefix}coordination action=spawn_workers count=1 task_id="
            )),
            "{error}"
        );
        assert!(
            error
                .message
                .contains(&format!("{prefix}task action=update")),
            "{error}"
        );
    }
}

#[tokio::test]
async fn recovery_guidance_parked_start_names_the_registered_supervisor_harness() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("CAS_FACTORY_WORKER_CLI", "claude");
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
    for (harness, prefix) in HARNESSES {
        let (_dir, service) = service("grok", crate::types::AgentRole::Worker);
        let store = service.inner.open_agent_store().unwrap();
        let mut supervisor =
            crate::types::Agent::new("named-supervisor".into(), "named-supervisor".into());
        supervisor.role = crate::types::AgentRole::Supervisor;
        supervisor
            .metadata
            .insert("supervisor_cli".into(), harness.into());
        store.register(&supervisor).unwrap();
        let mut worker = store.get("recovery-caller").unwrap();
        worker.parent_id = Some(supervisor.id.clone());
        store.update(&worker).unwrap();
        task(&service, "cas-parked", crate::types::TaskType::Task);
        let tasks = service.inner.open_task_store().unwrap();
        let mut parked = tasks.get("cas-parked").unwrap();
        parked.status = crate::types::TaskStatus::AwaitingMerge;
        tasks.update(&parked).unwrap();
        let error = service
            .task(Parameters(
                serde_json::from_value(serde_json::json!({"action":"start", "id":"cas-parked"}))
                    .unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{prefix}task action=request_changes")),
            "{error}"
        );
        assert_eq!(
            tasks.get("cas-parked").unwrap().status,
            crate::types::TaskStatus::AwaitingMerge
        );
        supervisor.metadata.remove("supervisor_cli");
        store.update(&supervisor).unwrap();
        let error = service
            .task(Parameters(
                serde_json::from_value(serde_json::json!({"action":"start", "id":"cas-parked"}))
                    .unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("`task action=request_changes"),
            "unknown recipient must stay neutral: {error}"
        );
    }
}
