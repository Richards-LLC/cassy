use super::lifecycle::{WorkerAttentionRelayOutcome, enqueue_merge_queue_ejection_relay};
use crate::store::{open_agent_store, open_prompt_queue_store, open_task_store};
use cas_types::{Agent, AgentRole, AgentStatus, Task};

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
        (3, "operator", Some("ended-session")),
    ] {
        let id = format!("cas-route-{stage}");
        tasks
            .add(&Task::new(id.clone(), "Ejected PR".into()))
            .unwrap();
        let result = enqueue_merge_queue_ejection_relay(
            &root,
            &id,
            "",
            12,
            Some(stage),
            &format!("run-{stage}"),
            None,
        );
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
