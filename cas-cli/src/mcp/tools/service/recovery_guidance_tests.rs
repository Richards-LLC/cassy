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

#[test]
fn qa_rejection_quotes_never_grant_merge_sources_cas_09f7() {
    use cas_types::{AgentRole, Task, TaskStatus, WorkTarget};
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    let (dir, service) = service("codex", AgentRole::Worker);
    let root = dir.path().join(".cas");
    env.set("CAS_ROOT", &root);
    let tasks = service.inner.open_task_store().unwrap();
    for (index, marker) in ["a".repeat(40), "release/moving".into(), String::new()]
        .iter()
        .enumerate()
    {
        let mut task = Task::new(format!("cas-quoted-{index}"), "Quoted QA finding".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("implementer".into());
        task.deliverables.parked_branch = Some("factory/implementer".into());
        task.deliverables.factory_branch_anchor = Some("b".repeat(40));
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:qa-quoted-fixture".into(),
            target_branch: "main".into(),
        });
        tasks.add(&task).unwrap();
        let now = chrono::Utc::now();
        let cas_store::QaPassOpen::Dispatched(pass) = cas_store::open_qa_pass(
            &root,
            &cas_store::NewQaPass {
                task_id: &task.id,
                implementer_agent_id: "implementer",
                branch: "factory/implementer",
                bound_head: &"b".repeat(40),
                deadline_at: now + chrono::Duration::minutes(5),
                max_rounds: 3,
            },
            now,
        )
        .unwrap() else {
            panic!("fixture must open a QA round")
        };
        let quoted = format!("Copied finding contains merge_source={marker}");
        let routed = service.route_qa_rejection(&pass, "independent-reviewer", &quoted);
        let reopened = tasks.get(&task.id).unwrap();
        assert_eq!(reopened.status, TaskStatus::Open, "{routed}");
        assert!(
            reopened.deliverables.authorized_merge_sources.is_empty(),
            "QA findings are data: {marker}"
        );
        assert!(reopened.notes.contains(&quoted));
    }
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
            .factory(Parameters(
                serde_json::from_value(serde_json::json!({"action": "epic_status"})).unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("{prefix}factory action=epic_status")),
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
async fn retained_worker_work_is_visible_after_session_shutdown_cas_7772d() {
    use cas_types::{
        Agent, AgentRole, AgentStatus, Dependency, DependencyType, Task, TaskStatus, TaskType,
        WorkTarget,
    };
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let (dir, service) = service("codex", AgentRole::Supervisor);
    let repo = dir.path();
    let root = repo.join(".cas");
    env.set("CAS_ROOT", &root);
    env.set("CAS_FACTORY_SESSION", "new-session");
    let git = |path: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "cas@example.test")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "cas@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-qm", "base"]);
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id='retained-fixture'\n",
    )
    .unwrap();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        repo,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo, &["push", "-q", "origin", "main"]);
    let agents = service.inner.open_agent_store().unwrap();
    let tasks = service.inner.open_task_store().unwrap();
    let target = WorkTarget {
        repo_selector: "project:retained-fixture".into(),
        target_branch: "main".into(),
    };
    let mut epic = Task::new("cas-7772d-epic".into(), "Retired work".into());
    epic.task_type = TaskType::Epic;
    epic.deliverables.work_target = Some(target.clone());
    tasks.add(&epic).unwrap();
    let mut retained = Vec::new();
    for (worker, id, dirty, live) in [
        ("warm-retired", "cas-aa11", false, false),
        ("dirty-retired", "cas-bb22", true, false),
        ("dirty-only", "cas-ee55", true, false),
        ("clean-retired", "cas-cc33", false, false),
        ("live-worker", "cas-dd44", true, true),
    ] {
        let path = root.join("worktrees").join(worker);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let branch = format!("factory/{worker}");
        git(
            repo,
            &[
                "worktree",
                "add",
                "-qb",
                &branch,
                path.to_str().unwrap(),
                "main",
            ],
        );
        let mut owner =
            Agent::new_with_role(format!("old-{worker}"), worker.into(), AgentRole::Worker);
        owner.factory_session = Some("old-session".into());
        owner.status = if live {
            AgentStatus::Active
        } else {
            AgentStatus::Shutdown
        };
        owner.pid = live.then(std::process::id);
        owner
            .metadata
            .insert("clone_path".into(), path.display().to_string());
        agents.register(&owner).unwrap();
        if worker == "clean-retired" {
            continue;
        }
        std::fs::write(path.join("delivered.txt"), worker).unwrap();
        git(&path, &["add", "delivered.txt"]);
        git(&path, &["commit", "-qm", &format!("fix({id}): delivery")]);
        let anchor = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["push", "-q", "origin", &branch]);
        if worker == "warm-retired" {
            git(repo, &["merge", "--squash", &branch]);
            git(repo, &["commit", "-qm", "squash delivered worker change"]);
            git(repo, &["push", "-q", "origin", "main"]);
            std::fs::write(path.join("local.txt"), "unpushed follow-up").unwrap();
            git(&path, &["add", "local.txt"]);
            git(
                &path,
                &["commit", "-qm", &format!("fix({id}): local follow-up")],
            );
        } else if !live && worker != "dirty-only" {
            std::fs::write(path.join("local.txt"), "unpushed work").unwrap();
            git(&path, &["add", "local.txt"]);
            git(
                &path,
                &["commit", "-qm", &format!("fix({id}): local follow-up")],
            );
        }
        if dirty {
            std::fs::write(path.join("dirty.txt"), "uncommitted").unwrap();
        }
        let mut child = Task::new(id.into(), worker.into());
        child.status = TaskStatus::Closed;
        child.assignee = Some(worker.into());
        child.deliverables.work_target = Some(target.clone());
        child.deliverables.parked_branch = Some(branch);
        child.deliverables.factory_branch_anchor = Some(anchor);
        tasks.add(&child).unwrap();
        tasks
            .add_dependency(&Dependency::new(
                id.into(),
                epic.id.clone(),
                DependencyType::ParentChild,
            ))
            .unwrap();
        if !live {
            retained.push((worker, id, path.clone(), git(&path, &["rev-parse", "HEAD"])));
        }
    }
    // A live successor with the same display name in a different checkout
    // cannot hide the retired worker's local work.
    let mut successor = Agent::new_with_role(
        "new-warm-retired".into(),
        "warm-retired".into(),
        AgentRole::Worker,
    );
    successor.pid = Some(std::process::id());
    successor.metadata.insert(
        "clone_path".into(),
        root.join("worktrees/live-worker").display().to_string(),
    );
    agents.register(&successor).unwrap();
    let before = git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let index_before: Vec<_> = retained
        .iter()
        .map(|(_, _, path, _)| {
            let index = path.join(git(path, &["rev-parse", "--git-path", "index"]));
            let contents = std::fs::read(&index).unwrap();
            (index, contents)
        })
        .collect();
    let banner = crate::hooks::handlers::session_hygiene::build_session_start_orphan_banner(&root)
        .expect("retained local-only work must surface even with no orphan process");
    for (worker, id, _, head) in &retained {
        assert!(
            banner.contains(worker) && banner.contains(id) && banner.contains(head),
            "{banner}"
        );
    }
    assert!(
        !banner.contains("clean-retired") && !banner.contains("live-worker"),
        "{banner}"
    );
    assert!(
        banner.contains("unpushed=1")
            && banner.contains("dirty=1")
            && banner.contains("push origin"),
        "{banner}"
    );
    assert!(
        banner.lines().any(|line| line.contains("dirty-only")
            && line.contains("unpushed=0")
            && line.contains("dirty=1")),
        "{banner}"
    );
    let gc = text(
        service
            .factory(Parameters(
                serde_json::from_value(serde_json::json!({"action":"gc_report", "full":true}))
                    .unwrap(),
            ))
            .await
            .unwrap(),
    );
    for (worker, id, _, head) in &retained {
        assert!(
            gc.contains(worker) && gc.contains(id) && gc.contains(head),
            "{gc}"
        );
    }
    let status = text(
        service
            .factory(Parameters(
                serde_json::from_value(serde_json::json!({"action":"epic_status", "id":epic.id}))
                    .unwrap(),
            ))
            .await
            .unwrap(),
    );
    let row = status
        .lines()
        .find(|line| line.starts_with("| cas-aa11 |"))
        .unwrap();
    assert!(
        row.contains("merged (squash, ancestry lost)"),
        "retained work must not hide the historical delivery proof: {status}"
    );
    assert!(!status.contains("✓ All child factory branches are merged"), "{status}");
    let unproven = status.lines().find(|line| line.starts_with("| cas-ee55 |")).unwrap();
    assert!(unproven.contains("retained local work"), "{status}");
    assert!(
        status.contains("Retained worker") && status.contains("unpushed=1"),
        "{status}"
    );
    assert_eq!(
        git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        before,
        "visibility never changes refs"
    );
    for (path, contents) in index_before {
        assert_eq!(
            std::fs::read(path).unwrap(),
            contents,
            "visibility never refreshes an index"
        );
    }
    let partial = crate::worktree::retained::scan_until(&root, std::time::Instant::now());
    assert!(partial.needs_attention() && partial.compact().contains("partial inventory"));
    assert_eq!(
        git(&retained[1].2, &["status", "--porcelain"]),
        "?? dirty.txt"
    );
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
                    // a8fed56db defaults to a bounded receipt. Full workflow
                    // guidance remains available through explicit full start.
                    serde_json::json!({"action": "start", "id": "cas-recovery", "brief": false}),
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
                "{prefix}factory action=spawn_workers count=1 task_id="
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

#[tokio::test]
async fn recovery_guidance_pending_close_uses_registered_supervisor_verification_tool() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("CAS_FACTORY_WORKER_CLI", "grok");
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
    for (harness, prefix) in HARNESSES
        .into_iter()
        .map(|(h, p)| (Some(h), p))
        .chain([(None, "")])
    {
        let (_dir, service) = service("grok", crate::types::AgentRole::Worker);
        let store = service.inner.open_agent_store().unwrap();
        let mut supervisor =
            crate::types::Agent::new("verification-owner".into(), "verification-owner".into());
        supervisor.role = crate::types::AgentRole::Supervisor;
        if let Some(harness) = harness {
            supervisor
                .metadata
                .insert("supervisor_cli".into(), harness.into());
        }
        store.register(&supervisor).unwrap();
        let mut worker = store.get("recovery-caller").unwrap();
        worker.parent_id = Some(supervisor.id.clone());
        store.update(&worker).unwrap();
        task(&service, "cas-exact-dispatch", crate::types::TaskType::Task);
        let dispatch = cas_store::create_verification_dispatch(
            &service.inner.cas_root,
            "cas-exact-dispatch",
            &worker.id,
            &supervisor.id,
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .unwrap();
        let result = service
            .task(Parameters(
                serde_json::from_value(
                    serde_json::json!({"action":"close", "id":"cas-exact-dispatch"}),
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let output = text(result);
        assert!(output.contains("VERIFICATION REQUIRED"), "{output}");
        assert!(
            output.contains(&format!(
                "recover without them: {prefix}verification action=add"
            )),
            "{harness:?}: {output}"
        );
        assert!(
            output.contains(&format!("dispatch_id={}", dispatch.id)),
            "{output}"
        );
        assert_eq!(
            service
                .inner
                .open_task_store()
                .unwrap()
                .get("cas-exact-dispatch")
                .unwrap()
                .status,
            crate::types::TaskStatus::Open
        );
        assert_eq!(
            cas_store::get_latest_verification_dispatch(
                &service.inner.cas_root,
                "cas-exact-dispatch"
            )
            .unwrap()
            .unwrap()
            .state,
            crate::types::VerificationDispatchState::Pending
        );
    }
}

/// One public action rescues an explicit unpushed commit without assuming the
/// dead worker's identity. The original checkout and source ref remain intact.
#[tokio::test]
async fn recover_dead_worker_branch_uses_successor_authority_cas_790e() {
    use crate::types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};
    use std::path::Path;
    use std::process::Command;
    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let (dir, service) = service("codex", AgentRole::Supervisor);
    let root = dir.path();
    git(root, &["init"]);
    git(root, &["config", "user.email", "fixture@example.test"]);
    git(root, &["config", "user.name", "Fixture"]);
    let source = "factory/dead-worker-cas-recover";
    git(root, &["switch", "-c", source]);
    git(root, &["commit", "--allow-empty", "-m", "base"]);
    let anchor = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("unpublished.txt"), "rescued work").unwrap();
    git(root, &["add", "unpublished.txt"]);
    git(root, &["commit", "-m", "unpublished"]);
    let tip = git(root, &["rev-parse", "HEAD"]);
    let remote = tempfile::TempDir::new().unwrap();
    git(remote.path(), &["init", "--bare"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let agents = service.inner.open_agent_store().unwrap();
    crate::store::open_event_store(&service.inner.cas_root).unwrap();
    let mut dead = Agent::new_with_role(
        "dead-worker-id".into(),
        "dead-worker".into(),
        AgentRole::Worker,
    );
    dead.status = AgentStatus::Shutdown;
    agents.register(&dead).unwrap();
    let tasks = service.inner.open_task_store().unwrap();
    let mut task = Task::new("cas-recover".into(), "Rescue unpushed work".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(dead.name.clone());
    task.deliverables.factory_branch_anchor = Some(anchor);
    task.deliverables.parked_branch = Some(source.into());
    tasks.add(&task).unwrap();
    let payload = serde_json::json!({"branch":source,"head":tip,"expected_remote":""});
    // All refusal rows must leave Git and task ownership alone. These exercise
    // registered identity, source ownership, liveness and exact expected tips.
    for mode in [
        "worker-caller",
        "live-owner",
        "moved-source",
        "foreign-source",
        "remote-mismatch",
        "retired-live-process",
    ] {
        let mut input = payload.clone();
        let mut caller = agents.get("recovery-caller").unwrap();
        let mut owned_child = None;
        match mode {
            "worker-caller" => {
                caller.role = AgentRole::Worker;
                agents.update(&caller).unwrap();
            }
            "live-owner" => {
                dead.status = AgentStatus::Active;
                agents.update(&dead).unwrap();
            }
            "moved-source" => input["head"] = "0000000000000000000000000000000000000000".into(),
            "foreign-source" => input["branch"] = "factory/unrelated".into(),
            "remote-mismatch" => input["expected_remote"] = tip.clone().into(),
            "retired-live-process" => {
                let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
                dead.pid = Some(child.id());
                agents.update(&dead).unwrap();
                agents.unregister(&dead.id).unwrap();
                owned_child = Some(child);
            }
            _ => unreachable!(),
        }
        let request = serde_json::from_value(serde_json::json!({"action":"recover_branch","task_id":task.id,"payload":input.to_string()})).unwrap();
        let result = service.factory(Parameters(request)).await;
        if let Some(mut child) = owned_child {
            let _ = child.kill();
            let _ = child.wait();
        }
        dead.pid = None;
        agents.register(&dead).unwrap();
        assert!(result.is_err(), "{mode} must refuse: {result:?}");
        caller.role = AgentRole::Supervisor;
        agents.update(&caller).unwrap();
        dead.status = AgentStatus::Shutdown;
        agents.update(&dead).unwrap();
        assert_eq!(
            tasks.get(&task.id).unwrap().assignee,
            task.assignee,
            "{mode}"
        );
        assert_eq!(git(root, &["rev-parse", "HEAD"]), tip, "{mode}");
        assert!(
            git(remote.path(), &["for-each-ref", "--format=%(refname)"]).is_empty(),
            "{mode}"
        );
    }
    agents.unregister(&dead.id).unwrap();
    let request = serde_json::from_value(serde_json::json!({"action":"recover_branch","task_id":task.id,"payload":payload.to_string()})).unwrap();
    let result = service
        .factory(Parameters(request))
        .await
        .expect("authenticated successor must rescue unpushed work");
    let receipt = text(result);
    // pin: the command receipt is readable at 80 columns and has a JSON escape.
    assert_eq!(
        receipt
            .replace(&tip, "0123456789012345678901234567890123456789")
            .trim(),
        include_str!("../../../../../docs/design/cli/factory-recover-branch.capture.txt").trim()
    );
    let destination = "refs/heads/factory/recovery-caller-cas-recover";
    assert_eq!(git(remote.path(), &["rev-parse", destination]), tip);
    assert_eq!(
        git(root, &["rev-parse", "HEAD"]),
        tip,
        "source checkout untouched"
    );
    let after = tasks.get(&task.id).unwrap();
    assert_eq!(after.assignee.as_deref(), Some("recovery-caller"));
    assert!(
        agents.get_lease(&task.id).unwrap().is_none(),
        "bounded recovery releases only its own lease"
    );
    assert!(after.notes.contains("BRANCH RECOVERED"), "{}", after.notes);
    for expected in [source, &tip, "recovery-caller"] {
        assert!(after.notes.contains(expected), "missing audit {expected}");
        assert!(receipt.contains(expected), "missing receipt {expected}");
    }
    let replay = serde_json::from_value(serde_json::json!({"action":"recover_branch","task_id":task.id,"payload":payload.to_string()})).unwrap();
    assert_eq!(
        text(service.factory(Parameters(replay)).await.unwrap()),
        receipt,
        "same exact recovery is idempotent"
    );
    // The real private pre-push guard still refuses a supervisor's ordinary
    // push from the dead worker's checkout. Recovery has no foreign-ref bypass.
    let hooks = tempfile::TempDir::new().unwrap();
    let hook = hooks.path().join("pre-push");
    std::fs::write(
        &hook,
        crate::ui::factory::daemon::runtime::teams::TeamsManager::WORKER_PRE_PUSH_HOOK,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let denied = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.path().display()))
        .args(["push", "origin", &format!("{source}:refs/heads/{source}")])
        .env("CAS_AGENT_NAME", "recovery-caller")
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("PUSH GUARD"));
}
