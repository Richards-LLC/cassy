use super::*;
use chrono::Duration;
use serde_json::json;

const REPO: &str = "github.com/acme/widgets";

fn agent(id: &str, role: &str, session: &str, machine: &str, repo: &str, age: i64) -> AgentInfo {
    serde_json::from_value(json!({
        "id": id, "name": id, "agent_type": "worker", "status": "active",
        "pid": null, "session_id": null, "machine_id": machine,
        "last_heartbeat": (Utc::now() - Duration::seconds(age)).to_rfc3339(),
        "active_tasks": 0, "user_id": "test-user", "team_id": "test-team", "project_id": "test-project",
        "metadata": {"role": role, "factory_session": session, "canonical_id": repo,
                     "hostname": machine, "focus": "cas-epic", "branch": format!("factory/{id}")}
    }))
    .unwrap()
}

fn lease(id: &str, owner: &str, status: LeaseStatus, expires: i64) -> TaskLease {
    TaskLease {
        task_id: id.into(),
        agent_id: owner.into(),
        status,
        acquired_at: Utc::now() - Duration::minutes(10),
        renewed_at: Utc::now() - Duration::seconds(30),
        expires_at: Utc::now() + Duration::seconds(expires),
        renewal_count: 1,
        epoch: 1,
        claim_reason: None,
    }
}

#[test]
fn cas_559c_details_enforce_repo_machine_session_liveness_and_active_leases() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store(&root).unwrap();
    let agents = vec![
        agent("sup", "supervisor", "same-name", "beta", REPO, 1),
        agent(
            "worker",
            "worker",
            "same-name",
            "beta",
            "git@github.com:acme/widgets.git",
            1,
        ),
        agent("stale", "worker", "same-name", "beta", REPO, 301),
        agent("other-machine", "worker", "same-name", "gamma", REPO, 1),
        agent("other-session", "worker", "elsewhere", "beta", REPO, 1),
        agent(
            "other-repo",
            "worker",
            "same-name",
            "beta",
            "github.com/acme/gadgets",
            1,
        ),
    ];
    let locks = vec![
        lease("cas-running", "worker", LeaseStatus::Active, 600),
        lease("cas-held", "sup", LeaseStatus::Active, 600),
        lease("cas-stale", "stale", LeaseStatus::Active, 600),
        lease("cas-expired", "worker", LeaseStatus::Active, -1),
        lease("cas-released", "worker", LeaseStatus::Released, 600),
        lease("cas-foreign", "other-machine", LeaseStatus::Active, 600),
        lease("cas-foreign-repo", "other-repo", LeaseStatus::Active, 600),
    ];
    let details = assemble_peer_details(
        peers::repo_peers(&agents, REPO, &[], None, Utc::now()),
        &agents,
        &locks,
        &[],
        tasks.as_ref(),
        Some("test-user"),
        Utc::now(),
    );
    assert_eq!(details.len(), 1);
    let detail = &details[0];
    assert_eq!(
        detail
            .workers
            .iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>(),
        ["worker"]
    );
    assert_eq!(
        detail
            .claims
            .iter()
            .map(|c| c.task_id.as_str())
            .collect::<Vec<_>>(),
        ["cas-held", "cas-running", "cas-stale"]
    );
    assert_eq!(detail.workers[0].claims[0].task_id, "cas-running");
    assert_eq!(detail.workers[0].branch.as_deref(), Some("factory/worker"));
    assert!(
        detail
            .claims
            .iter()
            .all(|c| c.title.is_none() && c.status.is_none())
    );
    assert!(
        detail
            .claims
            .iter()
            .all(|c| c.age_secs >= 30 && c.age_secs < 60)
    );
}

#[test]
fn cas_559c_render_is_bounded_sanitized_and_json_preserves_values() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut agents = vec![agent("sup", "supervisor", "factory-b", "beta", REPO, 1)];
    agents[0]
        .metadata
        .insert("focus".into(), format!("\u{1b}[31m\n{}", "界".repeat(100)));
    let report = PeerReport {
        canonical_id: Some(REPO.into()),
        status: "ready".into(),
        detail: true,
        peers: assemble_peer_details(
            peers::repo_peers(&agents, REPO, &[], None, Utc::now()),
            &agents,
            &[],
            &[],
            tasks.as_ref(),
            Some("test-user"),
            Utc::now(),
        ),
        error: None,
    };
    for ascii in [true, false] {
        for width in [80, 120] {
            let text = render_peer_report(&report, width, ascii);
            assert!(
                text.starts_with(
                    "[OK] 1 peer supervisors | 1 live\nRepo github.com/acme/widgets\n"
                ),
                "{text}"
            );
            assert!(
                text.lines()
                    .all(|line| ratatui::text::Line::raw(line).width() <= width)
            );
            assert!(!text.contains('\u{1b}'));
            assert!(!ascii || text.is_ascii());
            assert!(text.contains("--json for untruncated values"));
            assert!(text.contains("Claims  0") && text.contains("Workers 0 live (read-only)"));
        }
    }
    let encoded = serde_json::to_value(&report).unwrap();
    assert_eq!(
        encoded["peers"][0]["supervisor"]["focus"],
        agents[0].metadata["focus"]
    );
}

#[tokio::test]
async fn cas_559c_list_locks_sends_canonical_id_and_surfaces_failure() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/agents/locks"))
        .and(query_param("canonical_id", REPO))
        .respond_with(ResponseTemplate::new(503).set_body_string("claims unavailable"))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let error = tokio::task::spawn_blocking(move || {
        super::super::CloudCoordinator::new(super::super::CloudConfig {
            endpoint,
            token: Some("fake-test-token".into()),
            ..Default::default()
        })
        .unwrap()
        .with_canonical_id(Some(REPO.into()))
        .list_locks()
        .unwrap_err()
    })
    .await
    .unwrap();
    assert!(
        error.to_string().contains("503") && error.to_string().contains("claims unavailable"),
        "{error}"
    );
}

#[test]
fn cas_559c_missing_account_identity_never_means_zero_claims() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let agents = vec![agent("sup", "supervisor", "factory-b", "beta", REPO, 1)];
    let peers = peers::repo_peers(&agents, REPO, &[], None, Utc::now());
    let report = PeerReport {
        canonical_id: Some(REPO.into()),
        detail: true,
        status: "ready".into(),
        error: None,
        peers: assemble_peer_details(peers, &agents, &[], &[], tasks.as_ref(), None, Utc::now()),
    };
    assert_eq!(report.peers[0].claims_visibility, ClaimsVisibility::Unknown);
    let text = render_peer_report(&report, 80, true);
    assert!(
        text.contains("Claims not visible (account unknown)"),
        "{text}"
    );
    assert!(!text.contains("Claims  0"), "{text}");
}
