use super::*;
use cas_store::{AgentStore, ReminderStore, TaskStore};
use cas_types::{Agent, Dependency, DependencyType, Task, TaskStatus, TaskType};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn active_ready_epic(stores: &DirectorStores, task_id: &str) {
    let mut epic = Task::new("cas-active".into(), "Active work".into());
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;
    stores.task_store.add(&epic).unwrap();
    stores.task_store.add_dependency(&Dependency::new(
        task_id.into(), epic.id, DependencyType::ParentChild,
    )).unwrap();
}

#[tokio::test]
async fn cas_7806_stall_assignment_respects_peer_splits_and_live_claims() {
    let server = MockServer::start().await;
    let now = Utc::now();
    // Live, expired and local ownership claims exercise the real start policy.
    for (id, owner, expires) in [
        (
            "cas-claimed",
            "peer-worker",
            now + chrono::Duration::minutes(10),
        ),
        (
            "cas-expired",
            "peer-worker",
            now - chrono::Duration::minutes(1),
        ),
        ("cas-own", "id-local", now + chrono::Duration::minutes(10)),
        (
            "cas-loose-claimed",
            "peer-worker",
            now + chrono::Duration::minutes(10),
        ),
    ] {
        let key = crate::cloud::peer_claims::claim_key("github.com/acme/widgets", id);
        Mock::given(method("GET"))
            .and(path(format!("/api/agents/tasks/{key}/lock")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"lock": {
                "id": format!("lock-{id}"), "task_id": key, "agent_id": owner,
                "status": "active", "expires_at": expires, "renewed_at": now,
                "renewal_count": 0, "epoch": 1, "claim_reason": null
            }})))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/agents/locks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"locks": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"agents": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"lock": null})))
        .with_priority(10)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", "fixture");
        let dir = tempfile::tempdir().unwrap();
        let cas = crate::store::init_cas_dir(dir.path()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&cas, "github.com/acme/widgets").unwrap();
        crate::cloud::CloudConfig { endpoint, token: Some("fixture".into()), ..Default::default() }
            .save_to_cas_dir(&cas).unwrap();
        let stores = DirectorStores::open(&cas).unwrap();
        let mut local = Agent::new("id-local".into(), "local".into());
        local.role = cas_types::AgentRole::Worker;
        local.factory_session = Some("fixture".into());
        local.registered_at = now - chrono::Duration::hours(1);
        stores.agent_store.register(&local).unwrap();
        for id in ["cas-epic", "cas-split", "cas-claimed", "cas-free", "cas-expired", "cas-own", "cas-running"] {
            let mut task = Task::new(id.into(), id.into());
            if id == "cas-epic" { task.task_type = TaskType::Epic; task.status = TaskStatus::InProgress; }
            if id == "cas-running" { task.status = TaskStatus::InProgress; task.assignee = Some("busy-worker".into()); }
            stores.task_store.add(&task).unwrap();
            if id != "cas-epic" {
                stores.task_store.add_dependency(&Dependency::new(id.into(), "cas-epic".into(), DependencyType::ParentChild)).unwrap();
            }
        }
        for id in ["cas-loose-free", "cas-loose-claimed"] {
            stores.task_store.add(&Task::new(id.into(), id.into())).unwrap();
        }
        cas_store::insert_epic_split(&cas, &cas_store::EpicSplit {
            id: "split-peer".into(), epic_id: "cas-epic".into(), local_agent_id: "supervisor".into(),
            peer_agent_id: "peer-supervisor".into(), peer_name: "peer".into(), peer_machine: "elsewhere".into(),
            direction: cas_store::SplitDirection::Outgoing, mine: vec!["cas-free".into()],
            theirs: vec!["cas-split".into()], state: cas_store::SplitState::Accepted,
            reason: None, message_id: None, expires_at: now - chrono::Duration::hours(1),
            created_at: now - chrono::Duration::hours(2), decided_at: Some(now),
            decided_by: None,
            cleanup_pending: false,
            expiry_notice_pending: false,
        }).unwrap();
        let mut app = FactoryApp::for_test();
        app.cas_dir = cas.clone();
        app.factory_session = Some("fixture".into());
        app.current_epic_id = Some("cas-epic".into());
        app.unfiltered_director_data = DirectorData::load_fast(&cas).unwrap();
        let classify = |app: &mut FactoryApp| {
            if let Some(request) = app.ci_status_request() {
                app.apply_ci_status(request.run());
            }
            app.supervisor_actionable_from_evidence(&app.unfiltered_director_data, &Default::default(), &HashSet::new(), now)
        };
        let expected = |ids: &[&str]| Some(super::super::director::SupervisorActionableState::AssignReadyWork {
            task_ids: ids.iter().map(|id| (*id).to_string()).collect(), idle_workers: vec!["local".into()],
        });
        assert_eq!(classify(&mut app), expected(&["cas-expired", "cas-free", "cas-own"]),
            "binding peer split and live peer claim cannot be assigned here; expired/local claims stay available");
        for held in ["local", "id-local"] {
            assert_eq!(app.supervisor_actionable_from_evidence(&app.unfiltered_director_data,
                &Default::default(), &HashSet::from([held.to_string()]), now), None,
                "a held worker cannot receive an assignment by either identity");
        }
        // cas-f497: unreadable holds refuse an assignment, and only an assignment.
        assert_eq!(app.supervisor_actionable_from_ownership(&app.unfiltered_director_data,
            &Default::default(), None, now, &app.assignment_evidence), None);
        let mut with_merge = app.unfiltered_director_data.clone();
        let mut parked = with_merge.ready_tasks[0].clone();
        parked.id = "cas-parked".into();
        parked.status = TaskStatus::AwaitingMerge;
        parked.assignee = Some("local".into());
        parked.epic = Some("cas-epic".into());
        with_merge.in_progress_tasks.push(parked);
        let merge_ready = std::collections::HashMap::from([("cas-parked".to_string(), delivery_evidence::DeliveryEvidence {
            worker: "local".into(),
            delivery: ("factory/local".into(), Some("tip".into())),
            hold: None,
            merged_close_blocked: None,
        })]);
        assert!(matches!(
            app.supervisor_actionable_from_ownership(&with_merge, &merge_ready, None, now, &app.assignment_evidence),
            Some(super::super::director::SupervisorActionableState::MergeBranches { .. })
        ));
        // A split accepted after detection must retract the old assignment at
        // delivery, even while the loop still has its earlier ownership evidence.
        let detected = app.supervisor_actionable_from_evidence(&app.unfiltered_director_data,
            &Default::default(), &HashSet::new(), now).unwrap();
        let event = DirectorEvent::SupervisorStalled {
            next_step: detected, occurrence: now.to_rfc3339(), actionable_idle_secs: 600,
            operator_page_episode: None,
        };
        let mut later = cas_store::open_epic_splits(&cas).unwrap().remove(0);
        later.id = "split-after-detection".into();
        later.theirs = vec!["cas-free".into()];
        later.mine.clear();
        cas_store::insert_epic_split(&cas, &later).unwrap();
        let request = app.delivery_request(std::slice::from_ref(&event));
        assert!(request.needs_external_lane(), "claim reads cannot run on the shared read lane");
        let inputs = request.read(Some(&stores));
        let progress = SessionProgressRead {
            progress: Some((None, Default::default())), held_workers: Some(HashSet::new()),
        };
        assert!(!app.supervisor_stall_event_is_current(&event, inputs.data.as_ref().unwrap(),
            &Default::default(), &progress, inputs.assignment.as_ref()),
            "fresh delivery ownership must override the older assign set");
        cas_store::decide_epic_split(&cas, "split-after-detection", &[cas_store::SplitState::Accepted], cas_store::SplitState::Cancelled, None, now).unwrap();
        // Accepted ownership survives proposal expiry, but cancellation releases it without task edits.
        cas_store::decide_epic_split(&cas, "split-peer", &[cas_store::SplitState::Accepted], cas_store::SplitState::Cancelled, None, now).unwrap();
        assert_eq!(classify(&mut app), expected(&["cas-expired", "cas-free", "cas-own", "cas-split"]));
        // Close every ready child but keep one in-progress child, so the
        // focused epic has no ready work without becoming assemble-ready.
        for id in ["cas-expired", "cas-free", "cas-own", "cas-split", "cas-claimed"] {
            let mut task = stores.task_store.get(id).unwrap();
            task.status = TaskStatus::Closed;
            stores.task_store.update(&task).unwrap();
        }
        app.unfiltered_director_data = DirectorData::load_fast(&cas).unwrap();
        assert_eq!(classify(&mut app), None,
            "with no ready children, a pinned epic must not wake for unrelated fallback tasks");
        app.current_epic_id = None;
        assert_eq!(classify(&mut app), None,
            "no focused scope cannot recruit from the historical backlog");
        let request = assignment_evidence::AssignmentRequest::new(
            cas.clone(),
            &app.unfiltered_director_data,
            "fixture",
            "supervisor",
        )
        .unwrap();
        let mut closed = stores.task_store.get("cas-loose-free").unwrap();
        closed.status = TaskStatus::Closed;
        stores.task_store.update(&closed).unwrap();
        assert!(!request.read().allows("cas-loose-free"), "live terminal state must override a stale ready snapshot");

    }).await.unwrap();
}

#[test]
fn operator_ask_does_not_count_workers_holding_blocked_tasks_as_idle() {
    let now = Utc::now();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "operator-ask-stall")]);
    let dir = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(dir.path()).unwrap();
    let stores = DirectorStores::open(&cas).unwrap();

    let mut supervisor = cas_types::Agent::new("id-supervisor".into(), "supervisor".into());
    supervisor.role = cas_types::AgentRole::Supervisor;
    supervisor.factory_session = Some("operator-ask-stall".into());
    stores.agent_store.register(&supervisor).unwrap();
    for (name, owner) in [("blocked-owl", "operator"), ("blocked-fox", "supervisor")] {
        let mut worker = cas_types::Agent::new(format!("id-{name}"), name.into());
        worker.role = cas_types::AgentRole::Worker;
        worker.factory_session = Some("operator-ask-stall".into());
        worker.registered_at = now - chrono::Duration::hours(1);
        stores.agent_store.register(&worker).unwrap();

        let mut blocked =
            Task::new(format!("cas-{name}"), "waiting for the operator".into());
        blocked.status = TaskStatus::Blocked;
        blocked.assignee = Some(name.into());
        blocked.notes = format!(
            "[2026-11-08 17:00] 🚫 BLOCKER owner: {owner}; waiting for the missing rustc link."
        );
        stores.task_store.add(&blocked).unwrap();
    }
    stores
        .task_store
        .add(&Task::new(
            "cas-ready".into(),
            "unassigned backlog".into(),
        ))
        .unwrap();
    stores.reminder_store.as_ref().unwrap().init().unwrap();
    stores
        .reminder_store
        .as_ref()
        .unwrap()
        .create_with_scope(
            &supervisor.id,
            Some(&supervisor.id),
            "Operator ask deadline",
            cas_store::ReminderTriggerType::Time,
            Some(now + chrono::Duration::hours(1)),
            None,
            Some(&json!({"operator_ask": 4232719})),
            0,
            Some("operator-ask-stall"),
            Some("supervisor-session"),
            false,
            None,
        )
        .unwrap();

    active_ready_epic(&stores, "cas-ready");
    let mut app = FactoryApp::for_test();
    app.cas_dir = cas.clone();
    app.current_epic_id = Some("cas-active".into());
    app.factory_session = Some("operator-ask-stall".into());
    app.supervisor_name = "supervisor".into();
    let data = DirectorData::load_fast(&cas).unwrap();
    let assignment = assignment_evidence::AssignmentRequest::new(
        cas.clone(),
        &data,
        "operator-ask-stall",
        "supervisor",
    )
    .unwrap()
    .read();
    app.unfiltered_director_data = data.clone();
    app.assignment_evidence = assignment.clone();
    app.session_progress = SessionProgressRead {
        progress: Some((Some(now - chrono::Duration::minutes(30)), Default::default())),
        held_workers: Some(HashSet::new()),
    };
    assert!(
        app.detect_supervisor_stall().is_none(),
        "a supervisor silent past the threshold must not receive a supervisor_stalled event"
    );
    assert_eq!(
        app.supervisor_actionable_from_ownership(
            &data,
            &Default::default(),
            Some(&HashSet::new()),
            now,
            &assignment,
        ),
        None,
        "the live operator ask leaves neither supervisor- nor operator-blocked workers as assignment capacity"
    );

    let mut free = cas_types::Agent::new("id-free-worker".into(), "free-worker".into());
    free.role = cas_types::AgentRole::Worker;
    free.factory_session = Some("operator-ask-stall".into());
    free.registered_at = now - chrono::Duration::hours(1);
    stores.agent_store.register(&free).unwrap();
    let data = DirectorData::load_fast(&cas).unwrap();
    let assignment = assignment_evidence::AssignmentRequest::new(
        cas,
        &data,
        "operator-ask-stall",
        "supervisor",
    )
    .unwrap()
    .read();
    assert_eq!(
        app.supervisor_actionable_from_ownership(
            &data,
            &Default::default(),
            Some(&HashSet::new()),
            now,
            &assignment,
        ),
        Some(super::super::director::SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-ready".into()],
            idle_workers: vec!["free-worker".into()],
        }),
        "a worker with no task remains idle capacity"
    );
}

#[test]
fn failed_harness_is_not_idle_staffing_capacity_cas_8f40() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(project.path()).unwrap();
    let stores = DirectorStores::open(&cas).unwrap();
    stores.task_store.add(&Task::new("cas-ready".into(), "Ready work".into())).unwrap();
    active_ready_epic(&stores, "cas-ready");
    let now = Utc::now();
    let clone = cas.join("worktrees/failed-worker");
    std::fs::create_dir_all(&clone).unwrap();
    let account = env.home().join(".codex-selected");
    let sessions = account.join("sessions/2026/11/08");
    std::fs::create_dir_all(&sessions).unwrap();
    let rollout = sessions.join("rollout-failed-worker.jsonl");
    let mut worker = Agent::new("codex-failed-worker-session".into(), "failed-worker".into());
    worker.role = cas_types::AgentRole::Worker;
    worker.factory_session = Some("failed-staffing".into());
    worker.registered_at = now - chrono::Duration::hours(1);
    worker.metadata.insert("worker_cli".into(), "codex".into());
    worker.metadata.insert("clone_path".into(), clone.to_string_lossy().into_owned());
    worker.metadata.insert("worker_account_dir".into(), account.to_string_lossy().into_owned());
    stores.agent_store.register(&worker).unwrap();
    let meta = json!({"type":"session_meta", "payload":{"cwd":clone, "id":"failed-worker"}}).to_string();
    let healthy = json!({"type":"event_msg", "payload":{"type":"task_complete"}}).to_string();
    let failed = json!({"type":"event_msg", "payload":{"type":"task_complete", "error":{
        "codex_error_info":"other", "message":"The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account"}}}).to_string();
    let mut app = FactoryApp::for_test();
    app.cas_dir = cas.clone();
    app.current_epic_id = Some("cas-active".into());
    app.factory_session = Some("failed-staffing".into());
    app.supervisor_name = "supervisor".into();
    for (terminal, eligible) in [(&healthy, true), (&failed, false), (&healthy, true)] {
        std::fs::write(&rollout, format!("{meta}\n{terminal}\n")).unwrap();
        let data = DirectorData::load_fast(&cas).unwrap();
        let evidence = assignment_evidence::AssignmentRequest::new(cas.clone(), &data, "failed-staffing", "supervisor").unwrap().read();
        let action = app.supervisor_actionable_from_ownership(&data, &Default::default(), Some(&HashSet::new()), now, &evidence);
        assert_eq!(action.is_some(), eligible, "{terminal}");
    }
}
