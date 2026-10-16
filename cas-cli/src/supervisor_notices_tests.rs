use super::*;

#[test]
fn cas_5c2c_review_orphan_resume_and_same_name_successor() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let tasks = open_task_store(&root).unwrap();
    let mut ending =
        Agent::new_with_role("ending-id".into(), "owner".into(), AgentRole::Supervisor);
    ending.factory_session = Some("ended-session".into());
    agents.register(&ending).unwrap();
    let mut epic = cas_types::Task::new("cas-owned".into(), "Owned".into());
    epic.task_type = TaskType::Epic;
    epic.epic_verification_owner = Some(ending.id.clone());
    tasks.add(&epic).unwrap();
    orphan_epics(&root, &ending).unwrap();
    let mut epic = tasks.get(&epic.id).unwrap();
    epic.epic_verification_owner = Some(ending.id.clone());
    tasks.update(&epic).unwrap();
    orphan_epics(&root, &ending).unwrap();
    let sq = open_supervisor_queue_store(&root).unwrap();
    assert_eq!(
        sq.peek("operator", 10).unwrap().len(),
        2,
        "a resumed same-id owner's second end is a distinct occurrence"
    );
    orphan_epics(&root, &ending).unwrap();
    assert_eq!(
        sq.peek("operator", 10).unwrap().len(),
        2,
        "ownership already cleared is idempotent"
    );
    let mut successor = Agent::new_with_role(
        "successor-id".into(),
        ending.name.clone(),
        AgentRole::Supervisor,
    );
    successor.registered_at = ending.registered_at + chrono::Duration::seconds(1);
    successor.factory_session = Some("successor-session".into());
    agents.register(&successor).unwrap();
    epic.epic_verification_owner = Some(ending.name.clone());
    tasks.update(&epic).unwrap();
    orphan_epics(&root, &ending).unwrap();
    assert_eq!(
        tasks.get(&epic.id).unwrap().epic_verification_owner,
        Some(ending.name)
    );
}

#[tokio::test]
async fn remote_focus_conflict_uses_mailbox_and_operator_fallback() {
    use crate::cloud::{CloudConfig, peer_claims::PeerHold};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let server = MockServer::start().await;
    CloudConfig {
        endpoint: server.uri(),
        token: Some("fixture-token".into()),
        ..Default::default()
    }
    .save_to_cas_dir(&root)
    .unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id = \"github.com/fixture/repo\"\n",
    )
    .unwrap();
    let agents = open_agent_store(&root).unwrap();
    let mut sender =
        Agent::new_with_role("sender-id".into(), "sender".into(), AgentRole::Supervisor);
    sender.factory_session = Some("sender-session".into());
    agents.register(&sender).unwrap();
    let mut hold = PeerHold {
        user_id: None,
        agent_id: "remote-id".into(),
        holder: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
        live: Some(true),
    };
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(100))
                .set_body_json(serde_json::json!({"id":"pm-conflict","status":"queued"})),
        )
        .mount(&server)
        .await;
    let start = std::time::Instant::now();
    let delivery = remote_focus_conflict(&root, &sender.id, "cas-remote", &hold);
    assert!(
        start.elapsed() < std::time::Duration::from_millis(50),
        "focus dispatch must not wait for HTTP"
    );
    delivery.await.unwrap();
    hold.expires_at = hold.expires_at.map(|at| at + chrono::Duration::minutes(1));
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        2,
        "only the event's mailbox sends; no discovery or extra Cloud calls"
    );
    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["recipient_agent_id"], "remote-id");
    assert_eq!(bodies[0]["project_id"], "github.com/fixture/repo");
    assert_eq!(
        bodies[0]["dedupe_key"], bodies[1]["dedupe_key"],
        "repeated claim conflict reuses Cloud identity"
    );
    assert!(bodies[0]["body"].as_str().unwrap().contains("cas-remote"));
    assert!(
        open_supervisor_queue_store(&root)
            .unwrap()
            .peek("operator", 10)
            .unwrap()
            .is_empty(),
        "a slow successful peer send must not also create an operator fallback"
    );
    assert_eq!(
        open_prompt_queue_store(&root)
            .unwrap()
            .pending_count()
            .unwrap(),
        0
    );
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold)
        .await
        .unwrap();
    adopt_operator_notices(&root, "sender-session").unwrap();
    let rows = open_prompt_queue_store(&root)
        .unwrap()
        .peek_all(10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, "operator");
    assert_eq!(rows[0].factory_session.as_deref(), Some("sender-session"));
    let payload: crate::ui::factory::OperatorReplyPayload =
        serde_json::from_str(&rows[0].prompt).unwrap();
    assert_eq!(payload.schema_version, 2);
    assert!(rows[0].prompt.contains("cas-remote") && rows[0].prompt.contains("503"));
    // Renewing the same holder is still the same conflict episode.
    hold.expires_at = hold.expires_at.map(|at| at + chrono::Duration::minutes(1));
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold)
        .await
        .unwrap();
    adopt_operator_notices(&root, "sender-session").unwrap();
    assert_eq!(
        open_prompt_queue_store(&root)
            .unwrap()
            .peek_all(10)
            .unwrap()
            .len(),
        1
    );
    hold.live = Some(false);
    remote_focus_conflict(&root, &sender.id, "cas-stale", &hold)
        .await
        .unwrap();
    adopt_operator_notices(&root, "sender-session").unwrap();
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "a dead remote owner receives no HTTP notice"
    );
    assert_eq!(
        open_prompt_queue_store(&root)
            .unwrap()
            .peek_all(10)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn superseded_session_end_preserves_epic_ownership() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let tasks = open_task_store(&root).unwrap();
    let mut agent = Agent::new_with_role("old-id".into(), "rebound".into(), AgentRole::Supervisor);
    agent.cc_session_id = Some("new-id".into());
    agents.register(&agent).unwrap();
    let mut epic = cas_types::Task::new("cas-preserve".into(), "Preserve".into());
    epic.task_type = TaskType::Epic;
    epic.epic_verification_owner = Some(agent.id.clone());
    tasks.add(&epic).unwrap();
    crate::hooks::handlers::cleanup_agent_leases(&root, &agent.id);
    assert_eq!(
        tasks.get(&epic.id).unwrap().epic_verification_owner,
        epic.epic_verification_owner
    );
    assert!(agents.get(&agent.id).is_ok());
    assert_eq!(
        open_prompt_queue_store(&root)
            .unwrap()
            .pending_count()
            .unwrap(),
        0
    );
}
