use super::*;

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
        agent_id: "remote-id".into(),
        holder: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
        live: Some(true),
    };
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id":"pm-conflict","status":"queued"})),
        )
        .mount(&server)
        .await;
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold).await;
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold).await;
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
    remote_focus_conflict(&root, &sender.id, "cas-remote", &hold).await;
    let rows = open_prompt_queue_store(&root)
        .unwrap()
        .peek_all(10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, "operator");
    assert_eq!(rows[0].factory_session.as_deref(), Some("sender-session"));
    assert!(rows[0].prompt.contains("Remote notice failed"));
    hold.live = Some(false);
    remote_focus_conflict(&root, &sender.id, "cas-stale", &hold).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
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
