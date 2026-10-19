use super::lifecycle::{WorkerAttentionRelayOutcome, enqueue_merge_queue_ejection_relay};
use crate::store::{open_agent_store, open_prompt_queue_store, open_task_store};
use cas_types::{Agent, AgentRole, AgentStatus, Task};

#[test]
fn cas_5c2c_round2_sessionless_owner_receives_lifecycle() {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LifecyclePushResult, LifecycleTransition, emit_task_lifecycle_transition,
        prepare_task_lifecycle_outbox,
    };
    let home = tempfile::TempDir::new().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
        ("HOME", Some(home.path().to_str().unwrap())),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let owner = Agent::new_with_role(
        "sole-owner".into(),
        "sole-owner".into(),
        AgentRole::Supervisor,
    );
    agents.register(&owner).unwrap();
    assert!(agents.get(&owner.id).unwrap().factory_session.is_none());
    let mut worker = Agent::new_with_role("worker".into(), "worker".into(), AgentRole::Worker);
    worker.factory_session = Some("unstamped-owner-session".into());
    agents.register(&worker).unwrap();
    let prepared = prepare_task_lifecycle_outbox(
        agents.as_ref(),
        "cas-sessionless",
        "Sessionless",
        cas_types::TaskStatus::Open,
        cas_types::TaskStatus::InProgress,
        "worker",
        None,
        LifecycleTransition::Started,
        "once",
    );
    assert!(
        prepared.is_some(),
        "GH1171: the sole live sessionless supervisor owns this transition"
    );
    let sq = crate::store::open_supervisor_queue_store(&root).unwrap();
    let pq = open_prompt_queue_store(&root).unwrap();
    let result = emit_task_lifecycle_transition(
        sq.as_ref(),
        Some(pq.as_ref()),
        agents.as_ref(),
        "cas-sessionless",
        "Sessionless",
        cas_types::TaskStatus::Open,
        cas_types::TaskStatus::InProgress,
        "worker",
        None,
        LifecycleTransition::Started,
        "once",
    )
    .unwrap();
    assert!(matches!(result, LifecyclePushResult::Enqueued { .. }));
    assert_eq!(pq.peek_all(10).unwrap()[0].target, owner.name);
    assert!(sq.peek("operator", 10).unwrap().is_empty());
    agents
        .register(&Agent::new_with_role(
            "second-owner".into(),
            "second-owner".into(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    assert!(
        prepare_task_lifecycle_outbox(
            agents.as_ref(),
            "cas-ambiguous",
            "Ambiguous",
            cas_types::TaskStatus::Open,
            cas_types::TaskStatus::InProgress,
            "worker",
            None,
            LifecycleTransition::Started,
            "once",
        )
        .is_none(),
        "multiple sessionless supervisors remain ambiguous"
    );
}

#[test]
fn ct2_p3_dead_owner_notice_rerouted() {
    let _env =
        crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "ended-session")]);
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let queue = open_prompt_queue_store(&root).unwrap();
    let tasks = open_task_store(&root).unwrap();
    let mut dead = Agent::new_with_role("dead-id".into(), "owner".into(), AgentRole::Supervisor);
    dead.factory_session = Some("ended-session".into());
    dead.status = AgentStatus::Shutdown;
    agents.register(&dead).unwrap();
    let mut successor =
        Agent::new_with_role("successor-id".into(), "owner".into(), AgentRole::Supervisor);
    successor.factory_session = Some("new-session".into());
    agents.register(&successor).unwrap();
    let mut peer = Agent::new_with_role("peer-id".into(), "peer".into(), AgentRole::Supervisor);
    peer.factory_session = Some("peer-session".into());
    agents.register(&peer).unwrap();
    // A heartbeat-expired Active row is as unusable as Shutdown.
    let mut expired =
        Agent::new_with_role("expired-id".into(), "expired".into(), AgentRole::Supervisor);
    expired.factory_session = Some("expired-session".into());
    expired.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(10);
    agents.register(&expired).unwrap();
    for (stage, target, session) in [
        (1, "owner", Some("new-session")),
        (2, "peer", Some("peer-session")),
        (3, "operator", Some("connected-observer-session")),
    ] {
        let id = format!("cas-route-{stage}");
        let mut epic = Task::new(id.clone(), "Ejected PR".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/release".into());
        tasks.add(&epic).unwrap();
        let result = enqueue_merge_queue_ejection_relay(
            &root,
            &id,
            "",
            12,
            Some(stage),
            &format!("run-{stage}"),
            Some("Fast Validation › publication fixture"),
        );
        if stage == 3 {
            // The ejection outlives its factory. Reuse the existing Commander
            // adoption tick of another daemon, with no live supervisor.
            crate::supervisor_notices::adopt_operator_notices(&root, "connected-observer-session")
                .unwrap();
        }
        assert!(
            matches!(result, WorkerAttentionRelayOutcome::Persisted { .. }),
            "{result:?}"
        );
        let rows = queue.peek_all(100).unwrap();
        let row = rows
            .iter()
            .find(|r| r.prompt.contains(&id))
            .expect("notice is enqueued");
        assert_eq!(
            row.target, target,
            "the live receiver must replace dead routing"
        );
        assert_eq!(row.factory_session.as_deref(), session);
        if stage == 3 {
            let payload: crate::ui::factory::OperatorReplyPayload =
                serde_json::from_str(&row.prompt).unwrap();
            assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Blocker);
            assert!(payload.message.contains("#12"));
            assert!(payload.message.contains("run 3"));
            assert!(payload.message.contains("Fast Validation › publication fixture"));
            crate::supervisor_notices::adopt_operator_notices(&root, "connected-observer-session")
                .unwrap();
            assert_eq!(queue.peek_all(100).unwrap().iter().filter(|r| r.target == "operator").count(), 1);
        }
        assert!(rows.iter().all(
            |r| r.target == "operator" || r.factory_session.as_deref() != Some("ended-session")
        ));
        if stage == 1 {
            agents.unregister(&successor.id).unwrap();
        }
        if stage == 2 {
            agents.unregister(&peer.id).unwrap();
        }
    }
}

#[test]
fn cas_5c2c_review_pending_lifecycle_does_not_follow_unrelated_peer() {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LifecycleTransition, drain_lifecycle_outbox_with_agents, emit_task_lifecycle_transition,
    };
    let _env =
        crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "old-session")]);
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let sq = crate::store::open_supervisor_queue_store(&root).unwrap();
    let pq = open_prompt_queue_store(&root).unwrap();
    let mut owner =
        Agent::new_with_role("former-id".into(), "former".into(), AgentRole::Supervisor);
    owner.factory_session = Some("old-session".into());
    agents.register(&owner).unwrap();
    let result = emit_task_lifecycle_transition(
        sq.as_ref(),
        None,
        agents.as_ref(),
        "cas-pending",
        "Pending",
        cas_types::TaskStatus::InProgress,
        cas_types::TaskStatus::AwaitingMerge,
        "worker",
        Some("ready"),
        LifecycleTransition::AwaitingMerge,
        "once",
    );
    assert!(
        result.is_err(),
        "the durable row remains pending without a prompt store"
    );
    agents.unregister(&owner.id).unwrap();
    let mut peer = Agent::new_with_role("new-id".into(), "new-owner".into(), AgentRole::Supervisor);
    peer.factory_session = Some("new-session".into());
    agents.register(&peer).unwrap();
    let report =
        drain_lifecycle_outbox_with_agents(sq.as_ref(), pq.as_ref(), agents.as_ref(), 10).unwrap();
    assert_eq!(report.recovered, 0);
    let rows = pq.peek_all(10).unwrap();
    assert!(rows.is_empty());
    let result = emit_task_lifecycle_transition(
        sq.as_ref(),
        Some(pq.as_ref()),
        agents.as_ref(),
        "cas-fresh",
        "Fresh",
        cas_types::TaskStatus::Open,
        cas_types::TaskStatus::InProgress,
        "worker",
        None,
        LifecycleTransition::Started,
        "fresh",
    )
    .unwrap();
    assert!(matches!(result, crate::mcp::tools::core::task::lifecycle::supervisor_push::LifecyclePushResult::NoSupervisor));
    assert!(sq.peek("operator", 10).unwrap().is_empty());
    let mut successor = Agent::new_with_role(
        "successor".into(),
        owner.name.clone(),
        AgentRole::Supervisor,
    );
    successor.factory_session = Some("successor-session".into());
    agents.register(&successor).unwrap();
    let report =
        drain_lifecycle_outbox_with_agents(sq.as_ref(), pq.as_ref(), agents.as_ref(), 10).unwrap();
    assert_eq!(
        report.recovered, 1,
        "the same owner's successor may receive the saved transition"
    );
    let rows = pq.peek_all(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, successor.name);
    assert_eq!(rows[0].factory_session, successor.factory_session);
}
