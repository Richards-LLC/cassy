//! Own-account transcript queries cross the real coordination/mailbox boundary.
use crate::test_env_guard::TestEnvGuard;
use cas::cloud::{CloudConfig, peer_mailbox::PeerMailboxRuntime};
use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_agent_store, open_prompt_queue_store};
use cas::types::{Agent, AgentRole};
use rmcp::{handler::server::wrapper::Parameters, model::RawContent};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn text(result: rmcp::model::CallToolResult) -> String {
    result
        .content
        .into_iter()
        .filter_map(|item| match item.raw {
            RawContent::Text(t) => Some(t.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn own_account_peer_transcript_is_bounded_redacted_and_daemon_served_cas_02c5() {
    let env = TestEnvGuard::temp_home();
    let server = MockServer::start().await;
    let local = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let a = init_cas_dir(local.path()).unwrap();
    let b = init_cas_dir(remote.path()).unwrap();
    for (root, repo) in [
        (&a, "github.com/acme/local"),
        (&b, "github.com/acme/remote"),
    ] {
        cas::cloud::set_canonical_id_in_config_toml(root, repo).unwrap();
        CloudConfig {
            endpoint: server.uri(),
            token: Some("fixture-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(root)
        .unwrap();
    }
    for (root, id, name, session) in [
        (&a, "local", "local-sup", "local-factory"),
        (&b, "remote", "remote-sup", "remote-factory"),
    ] {
        let mut agent = Agent::new(id.into(), name.into());
        agent.role = AgentRole::Supervisor;
        agent.factory_session = Some(session.into());
        agent.cc_session_id = Some("transcript-session".into());
        open_agent_store(root).unwrap().register(&agent).unwrap();
    }
    let slug = remote.path().to_str().unwrap().replace(['/', '.'], "-");
    let transcript = env
        .home()
        .join(".claude/projects")
        .join(slug)
        .join("transcript-session.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let original = format!(
        "{}\n{}{}\n{}\n{}",
        json!({"text":"OLD-MATCH /mcp"}),
        (json!({"text":"padding"}).to_string() + "\n").repeat(100_000),
        json!({"password":"fixture-secret"}),
        json!({"text":"Authorization: Bearer fixture.token.value"}),
        (json!({"text":"/mcp reload recorded"}).to_string() + "\n").repeat(10_000)
    );
    std::fs::write(&transcript, &original).unwrap();
    let row = |id: &str, name: &str, owner: &str, repo: &str| {
        json!({
            "id":id, "name":name, "user_id":owner, "agent_type":"primary", "status":"active",
            "last_heartbeat":chrono::Utc::now().to_rfc3339(), "active_tasks":0, "machine_id":id,
            "metadata":{"role":"supervisor", "canonical_id":repo, "hostname":id,
                "factory_session":format!("{id}-factory")}
        })
    };
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"agents":[
            row("local","local-sup","owner","github.com/acme/local"),
            row("remote","remote-sup","owner","github.com/acme/remote"),
            row("foreign","foreign-sup","other-owner","github.com/acme/remote")
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"query-1","status":"queued"})),
        )
        .mount(&server)
        .await;
    let core = CasCore::with_daemon(a.clone(), None, None);
    core.set_agent_id_for_testing("local".into());
    let service = CasService::new(core, None);
    let request = |peer: &str, session: &str| {
        serde_json::from_value(json!({
        "action":"peer_transcript", "peer":peer, "session_id":session, "message":"/mcp", "limit":2
    })).unwrap()
    };
    assert!(
        service
            .coordination(Parameters(request("foreign-sup@foreign", "current")))
            .await
            .is_err()
    );
    assert!(
        service
            .coordination(Parameters(request("remote-sup@remote", "../../secret")))
            .await
            .is_err()
    );
    let queued = text(
        service
            .coordination(Parameters(request("remote-sup@remote", "current")))
            .await
            .unwrap(),
    );
    assert!(
        queued.contains("query-1") && queued.contains("inbox_poll"),
        "{queued}"
    );
    let sent: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/peer-messages")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(sent.len(), 1, "foreign/invalid requests must not send");
    assert_eq!(sent[0]["scope"], "user");
    let body = sent[0]["body"].as_str().unwrap();
    let incoming = |id: &str, scope: &str| {
        json!({
            "id":id, "project_id":"github.com/acme/local", "recipient_agent_id":"remote",
            "sender_agent_id":"local", "sender_name":"local-sup", "sender_machine_id":"local",
            "body":body, "summary":"transcript query", "created_at":chrono::Utc::now().to_rfc3339(), "scope":scope
        })
    };
    let mut foreign = incoming("team-query", "project");
    foreign["project_id"] = json!("github.com/acme/remote");
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/claim"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"messages":[incoming("query-1","user"),foreign]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/ack"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[
            {"id":"query-1","status":200},{"id":"team-query","status":200}
        ]})))
        .mount(&server)
        .await;
    let remote_root = b.clone();
    let report = tokio::task::spawn_blocking(move || {
        PeerMailboxRuntime::start(&remote_root, "remote-factory")
            .unwrap()
            .tick_blocking()
    })
    .await
    .unwrap();
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!((report.delivered, report.rejected), (1, 1), "{report:?}");
    assert!(
        open_prompt_queue_store(&b)
            .unwrap()
            .peek_all(10)
            .unwrap()
            .is_empty(),
        "daemon must serve without prompting the peer"
    );
    let sent: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/peer-messages")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(
        sent.len(),
        2,
        "non-user scope must not receive transcript data"
    );
    assert_eq!(sent[1]["recipient_agent_id"], "local");
    assert_eq!(sent[1]["in_reply_to"], "query-1");
    assert_eq!(sent[1]["scope"], "user");
    let excerpt = sent[1]["body"].as_str().unwrap();
    assert!(excerpt.len() <= 8192, "{} bytes", excerpt.len());
    assert!(
        excerpt.contains("/mcp reload recorded") && excerpt.contains("[REDACTED"),
        "{excerpt}"
    );
    assert!(
        excerpt.contains("scan_truncated=true") && excerpt.contains("output_truncated=true"),
        "{excerpt}"
    );
    for secret in ["fixture-secret", "fixture.token.value", "OLD-MATCH"] {
        assert!(!excerpt.contains(secret), "{secret}");
    }
    assert_eq!(std::fs::read_to_string(transcript).unwrap(), original);
}
