//! cas-e477: the coordinator's peer-discovery wire contract against the
//! Cassy Cloud `/api/agents` routes (register, heartbeat, paged list).
use super::*;
use crate::cloud::peers::{META_CANONICAL_ID, META_FOCUS, META_ROLE};
use crate::types::AgentRole;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "github.com/acme/widgets";

fn coordinator(endpoint: &str) -> CloudCoordinator {
    CloudCoordinator::new(CloudConfig {
        endpoint: endpoint.to_string(),
        token: Some("test-token".to_string()),
        ..Default::default()
    })
    .unwrap()
    .with_canonical_id(Some(REPO.to_string()))
}

fn agent_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": format!("name-{id}"),
        "agent_type": "primary",
        "status": "active",
        "pid": null,
        "session_id": null,
        "machine_id": "machine-a",
        "last_heartbeat": "2026-10-11T03:00:00.000Z",
        "active_tasks": 0,
        "metadata": {}
    })
}

#[tokio::test]
async fn register_carries_role_repo_and_focus_in_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/register"))
        .and(body_partial_json(json!({
            "metadata": {
                META_ROLE: "supervisor",
                META_FOCUS: "cas-571d",
            },
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "success", "agent": agent_json("sup-1")})),
        )
        .expect(4)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let registered = tokio::task::spawn_blocking(move || {
        // The shared daemon may register another factory's successor. Its
        // ambient factory identity must never override the registered slot.
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_ID", "another-factory"),
        ]);
        for (id, machine, project) in [
            ("sup-1", "machine-a", REPO),
            ("sup-successor", "machine-a", REPO),
            ("other-machine", "machine-b", REPO),
            ("other-repo", "machine-a", "github.com/acme/other"),
        ] {
            let mut coord = coordinator(&endpoint).with_canonical_id(Some(project.into()));
            let mut agent = Agent::new(id.into(), "bright-lark-8".into());
            agent.role = AgentRole::Supervisor;
            agent.machine_id = Some(machine.into());
            agent.factory_session = Some(format!("launch-{id}"));
            coord.register_with_focus(&agent, Some("cas-571d"))?;
        }
        Ok::<_, crate::error::CasError>(())
    })
    .await
    .unwrap();
    registered.unwrap();
    server.verify().await;
    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<serde_json::Value> = requests.iter().map(|r| r.body_json().unwrap()).collect();
    assert_eq!(bodies[0]["metadata"][META_CANONICAL_ID], REPO);
    let slot = &bodies[0]["factory_id"];
    assert!(slot.as_str().is_some_and(|id| !id.is_empty() && id != "another-factory"));
    assert_eq!(slot, &bodies[1]["factory_id"], "a full relaunch keeps its Cloud slot");
    for body in &bodies[2..] {
        assert_ne!(slot, &body["factory_id"], "machine and project separate default slots");
    }
}

#[tokio::test]
async fn heartbeat_agent_targets_that_agent_and_reads_the_flat_body() {
    let server = MockServer::start().await;
    // The server answers a heartbeat with the bare agent, not {status, agent}.
    Mock::given(method("POST"))
        .and(path("/api/agents/sup-2/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(agent_json("sup-2")))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let beat = tokio::task::spawn_blocking(move || coordinator(&endpoint).heartbeat_agent("sup-2"))
        .await
        .unwrap();
    assert_eq!(beat.unwrap().id, "sup-2");
}

#[tokio::test]
async fn list_agent_infos_follows_every_keyset_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("cursor_id", "a2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": [agent_json("a3")],
            "next_cursor": null,
            "next_cursor_id": null,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": [agent_json("a1"), agent_json("a2")],
            "next_cursor": "2026-10-10T00:00:00.000Z",
            "next_cursor_id": "a2",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let agents = tokio::task::spawn_blocking(move || coordinator(&endpoint).list_agent_infos())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["a1", "a2", "a3"]);
}

// cas-f707: every agent and claim call carries the project scope
// (petra-stella-cloud#151) under the one wire key.

fn lock_json(task: &str) -> serde_json::Value {
    json!({
        "id": "lock-1",
        "task_id": task,
        "agent_id": "sup-1",
        "status": "active",
        "expires_at": "2026-10-13T22:00:00Z",
        "renewed_at": "2026-10-13T21:30:00Z",
        "renewal_count": 0,
        "epoch": 1,
        "claim_reason": null,
    })
}

#[tokio::test]
async fn cas_f707_register_list_and_claim_calls_carry_the_canonical_id() {
    // pin: body field name agreed with the cloud on petra-stella-cloud#151.
    assert_eq!(PROJECT_SCOPE_KEY, "canonical_id");
    // pin: production's discovery query parameter (cloud PR #153).
    assert_eq!(PROJECT_QUERY_KEY, "project");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/register"))
        .and(body_partial_json(json!({
            PROJECT_SCOPE_KEY: REPO,
            "metadata": {
                crate::cloud::peers::META_HOSTNAME: crate::cloud::peers::machine_label(),
            },
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "success", "agent": agent_json("sup-1")})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param(PROJECT_QUERY_KEY, REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": [agent_json("sup-1")],
        })))
        .expect(2)
        .mount(&server)
        .await;
    for route in ["claim", "release", "renew"] {
        Mock::given(method("POST"))
            .and(path(format!("/api/agents/tasks/cas-1~abcd/{route}")))
            .and(body_partial_json(json!({
                "agent_id": "sup-1",
                PROJECT_SCOPE_KEY: REPO,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "result": "claimed",
                "lock": lock_json("cas-1~abcd"),
            })))
            .expect(1)
            .mount(&server)
            .await;
    }

    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let mut coord = coordinator(&endpoint);
        let mut agent = Agent::new("sup-1".into(), "bright-lark-8".into());
        agent.role = AgentRole::Supervisor;
        coord.register(&agent).unwrap();
        assert_eq!(coord.list_agent_infos().unwrap().len(), 1);
        assert_eq!(coord.list_agents(None).unwrap().len(), 1);
        assert!(matches!(
            coord.claim("cas-1~abcd", 600, Some("reason")).unwrap(),
            ClaimResult::Success(_)
        ));
        coord.renew("cas-1~abcd", 600).unwrap();
        coord.release("cas-1~abcd").unwrap();
    })
    .await
    .unwrap();
    server.verify().await;
}

#[tokio::test]
async fn cas_f707_peer_claims_send_the_canonical_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/agents/tasks/{}/claim",
            crate::cloud::peer_claims::claim_key(REPO, "cas-9")
        )))
        .and(body_partial_json(json!({
            PROJECT_SCOPE_KEY: REPO,
            // The holder names this machine as discovery does (cas-f707).
            "reason": serde_json::to_string(&crate::cloud::peer_claims::ClaimHolder::new(
                crate::cloud::peer_claims::ClaimKind::Task,
                "alpha-sup",
                REPO,
            ))
            .unwrap(),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "result": "claimed",
            "lock": lock_json("cas-9"),
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().unwrap();
    let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, REPO).unwrap();
    let endpoint = server.uri();
    let config = CloudConfig {
        endpoint,
        token: Some("test-token".to_string()),
        ..Default::default()
    };
    config.save_to_cas_dir(&cas_root).unwrap();
    let outcome = tokio::task::spawn_blocking(move || {
        let claims = crate::cloud::peer_claims::PeerClaims::for_project(&cas_root)
            .expect("logged-in project with an identity");
        claims.acquire(
            "cas-9",
            crate::cloud::peer_claims::ClaimKind::Task,
            "sup-1",
            "alpha-sup",
            600,
            &|_| false,
        )
    })
    .await
    .unwrap();
    assert_eq!(outcome, crate::cloud::peer_claims::Acquire::Claimed);
    server.verify().await;
}

#[tokio::test]
async fn cas_f707_heartbeat_404_and_401_mean_register_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/gone/heartbeat"))
        .respond_with(ResponseTemplate::new(404).set_body_string("agent not found"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/agents/foreign/heartbeat"))
        .respond_with(ResponseTemplate::new(401).set_body_string("not yours"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/agents/busy/heartbeat"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let outcomes = tokio::task::spawn_blocking(move || {
        let coord = coordinator(&endpoint);
        ["gone", "foreign", "busy"].map(|id| coord.heartbeat_presence(id, None).unwrap_err())
    })
    .await
    .unwrap();
    assert!(matches!(
        outcomes[0],
        HeartbeatError::NotRegistered { status: 404, .. }
    ));
    assert!(matches!(
        outcomes[1],
        HeartbeatError::NotRegistered { status: 401, .. }
    ));
    assert!(matches!(outcomes[2], HeartbeatError::Failed(_)));
}

#[tokio::test]
async fn cas_4ac8_shutdown_and_release_retry_after_rate_limit() {
    for (route, success) in [
        ("/api/agents/sup-1/shutdown", json!({"released_locks": 2})),
        ("/api/agents/tasks/cas-one/release", json!({"status":"ok"})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
            .up_to_n_times(1)
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(success))
            .expect(1)
            .mount(&server)
            .await;
        let endpoint = server.uri();
        let started = Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            let client = coordinator(&endpoint).with_agent_id("sup-1");
            if route.ends_with("shutdown") {
                client.shutdown().map(|n| assert_eq!(n, 2))
            } else {
                client.release("cas-one")
            }
        })
        .await
        .unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "Retry-After was ignored"
        );
    }
}

#[tokio::test]
async fn cas_4ac8_presence_names_rate_limiting() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/register"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let record = tokio::task::spawn_blocking(move || {
        crate::cloud::presence::register_now(
            &coordinator(&endpoint),
            &root,
            &Agent::new("sup-1".into(), "sup".into()),
            chrono::Utc::now(),
        )
    })
    .await
    .unwrap();
    assert!(
        crate::cloud::presence::render_line(Some(&record), chrono::Utc::now())
            .contains("rate limited")
    );
}

#[tokio::test]
async fn cas_4ac8_cleanup_budget_does_not_retry_other_errors_or_early_retry_after() {
    for (code, header) in [
        (503, "0"),
        (429, "600"),
        (429, "Thu, 01 Jan 2099 00:00:00 GMT"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agents/sup-1/shutdown"))
            .respond_with(ResponseTemplate::new(code).insert_header("Retry-After", header))
            .expect(1)
            .mount(&server)
            .await;
        let endpoint = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            coordinator(&endpoint)
                .with_agent_id("sup-1")
                .with_timeout(Duration::from_millis(100))
                .shutdown()
        })
        .await
        .unwrap();
        let error = result.unwrap_err().to_string();
        assert!(error.contains(&format!("({code})")), "{error}");
        if code == 429 {
            assert!(error.contains("rate limited"), "{error}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
