use super::*;
use cas_store::{AgentStore, TaskStore};
use cas_types::{Agent, Dependency, DependencyType, Task, TaskStatus, TaskType};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        assert_eq!(classify(&mut app), expected(&["cas-loose-free"]),
            "without focus, production evidence must still cover ready tasks and filter the live peer claim");

    }).await.unwrap();
}
