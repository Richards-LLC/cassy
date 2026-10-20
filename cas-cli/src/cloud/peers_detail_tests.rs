use super::*;
use chrono::Duration;
use serde_json::json;

const REPO: &str = "github.com/acme/widgets";

#[test]
fn cas_cd42_board_exposes_claim_owned_epic_target_roots_and_unknown_prs() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let mut epic = crate::types::Task::new("cas-epic".into(), "Epic lane".into());
    epic.task_type = crate::types::TaskType::Epic;
    epic.branch = Some("epic/lane".into());
    tasks.add(&epic).unwrap();
    let mut owner = agent("sup", "supervisor", "lane", "alpha", REPO, 1);
    owner
        .metadata
        .insert("clone_path".into(), "/alpha/worktree".into());
    owner
        .metadata
        .insert("scratch_root".into(), "/alpha/scratch".into());
    let agents = vec![
        owner,
        agent("remote", "supervisor", "lane", "beta", REPO, 400),
    ];
    let locks = vec![lease("cas-epic", "sup", LeaseStatus::Active, 600)];
    let details = assemble_peer_details(
        peers::repo_peers(&agents, REPO, &[], None, Utc::now()),
        &agents,
        &locks,
        &[],
        tasks.as_ref(),
        Some("test-user"),
        Utc::now(),
    );
    let data = serde_json::to_value(&details).unwrap();
    assert_eq!(data[0]["worktree_root"], "/alpha/worktree");
    assert_eq!(data[0]["scratch_root"], "/alpha/scratch");
    assert_eq!(data[0]["epics"][0]["task_id"], "cas-epic");
    assert_eq!(data[0]["epics"][0]["target_branch"], "epic/lane");
    assert_eq!(data[0]["epics"][0]["owner"], "sup");
    assert_eq!(data[0]["prs_status"]["state"], "unavailable");
    assert!(data[0]["prs"].as_array().unwrap().is_empty());
    assert_eq!(data[1]["observations_status"]["state"], "stale");
    assert!(data[1]["observations_status"]["observed_at"].is_string());
}

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
    let mut lapsed = lease("cas-lapsed", "stale", LeaseStatus::Active, 600);
    lapsed.renewed_at = Utc::now() - chrono::Duration::seconds(301);
    let locks = vec![
        lapsed,
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
    let mut report = PeerReport {
        #[cfg(feature = "mcp-proxy")]
        stakeholder_contacts: serde_json::json!({"state":"shared","contacts":[{"workspace_id":"T123","user_id":"U123","last_contact_at":"2026-10-20T16:23:00Z","last_poster":"peer-on-prowl","permalink":"https://example.slack.com/root","next_allowed_at":"2026-10-20T17:23:00Z","owner":{"registration_id":"new-owner"}}]}),
        hidden_peers: 0,
        overlaps: Vec::new(),
        limitations: Vec::new(),
        observed_at: Utc::now(),
        cloud_presence: None,
        activation: None,
        cloud_presence_message: String::new(),
        mailbox_status: String::new(),
        epic_splits: Vec::new(),
        epic_splits_status: ObservationStatus::at("ready", Some(Utc::now()), None),
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
    // cas-2db0 (QA F01): the logged-out reason wraps instead of being cut.
    report.peers[0].supervisor.harness_auth_failed = Some("Login expired · Please run /login".into());
    for ascii in [true, false] {
        for width in [80, 120] {
            let text = render_peer_report(&report, width, ascii);
            assert!(
                text.starts_with("[OK] 1 peer supervisor | 1 live\nRepo github.com/acme/widgets\n"),
                "{text}"
            );
            let middot = if ascii { "?" } else { "·" };
            assert!(
                text.split_whitespace().collect::<Vec<_>>().join(" ").contains(&format!(
                    "Harness logged out: \"Login expired {middot} Please run /login\"; messages to it are refused"
                )),
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
            #[cfg(feature = "mcp-proxy")]
            assert!(text.contains("Stakeholder contacts [shared]") && text.contains("Next allowed 2026-10-20T17:23:00Z") && text.contains("Owner new-owner"));
        }
    }
    let encoded = serde_json::to_value(&report).unwrap();
    #[cfg(feature = "mcp-proxy")]
    assert_eq!(encoded["stakeholder_contacts"]["contacts"][0]["last_poster"], "peer-on-prowl");
    assert_eq!(
        encoded["peers"][0]["supervisor"]["focus"],
        agents[0].metadata["focus"]
    );
}

#[tokio::test]
async fn cas_559c_list_locks_sends_project_and_surfaces_failure() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/agents/locks"))
        .and(query_param("project", REPO))
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
    let mut agents = agents;
    agents[0].team_id = None; // No team authorization signal.
    let peers = peers::repo_peers(&agents, REPO, &[], None, Utc::now());
    let report = PeerReport {
        #[cfg(feature = "mcp-proxy")]
        stakeholder_contacts: serde_json::json!({"state":"unconfigured"}),
        hidden_peers: 0,
        overlaps: Vec::new(),
        limitations: Vec::new(),
        observed_at: Utc::now(),
        cloud_presence: None,
        activation: None,
        cloud_presence_message: String::new(),
        mailbox_status: String::new(),
        epic_splits: Vec::new(),
        epic_splits_status: ObservationStatus::at("ready", Some(Utc::now()), None),
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

#[tokio::test]
async fn cas_559c_paging_echoes_sort_cursor_with_project_on_every_page() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("project", REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "agents": [], "next_cursor": "created-at", "next_cursor_id": "agent-b",
            "next_sort_cursor": "opaque-sort-key"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("project", REPO))
        .and(query_param("cursor", "created-at"))
        .and(query_param("cursor_id", "agent-b"))
        .and(query_param("sort_cursor", "opaque-sort-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "agents": [agent("remote", "supervisor", "session", "beta", REPO, 1)],
            "next_cursor": null, "next_cursor_id": null, "next_sort_cursor": null
        })))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let agents = tokio::task::spawn_blocking(move || {
        super::super::CloudCoordinator::new(super::super::CloudConfig {
            endpoint,
            token: Some("fake-test-token".into()),
            ..Default::default()
        })
        .unwrap()
        .with_canonical_id(Some(REPO.into()))
        .list_agent_infos()
        .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(
        agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["remote"]
    );
}

#[test]
fn cas_a81c_claim_copy_uses_scoped_plain_ids_owner_names_and_missing_title_reason() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    tasks
        .add(&crate::types::Task::new(
            "cas-local".into(),
            "Synced task title".into(),
        ))
        .unwrap();
    let owner_id = "22b34bda-18a6-4be1-a107-b90ad96f6fa7";
    let mut owner = agent(owner_id, "worker", "factory-b", "beta", REPO, 1);
    owner.name = "qa-worker".into();
    owner
        .metadata
        .insert("branch".into(), "factory/qa-worker".into());
    let agents = vec![
        agent("sup", "supervisor", "factory-b", "beta", REPO, 1),
        owner,
    ];
    let local_key = super::super::peer_claims::claim_key(REPO, "cas-local");
    let remote_key = super::super::peer_claims::claim_key(REPO, "cas-remote");
    let foreign_key = super::super::peer_claims::claim_key("github.com/acme/other", "cas-foreign");
    let locks = vec![
        lease(&local_key, owner_id, LeaseStatus::Active, 600),
        lease(&remote_key, owner_id, LeaseStatus::Active, 600),
        lease(&foreign_key, owner_id, LeaseStatus::Active, 600),
    ];
    let report = PeerReport {
        #[cfg(feature = "mcp-proxy")]
        stakeholder_contacts: serde_json::json!({"state":"unconfigured"}),
        hidden_peers: 0,
        overlaps: Vec::new(),
        limitations: Vec::new(),
        observed_at: Utc::now(),
        cloud_presence: None,
        activation: None,
        cloud_presence_message: String::new(),
        mailbox_status: String::new(),
        epic_splits: Vec::new(),
        epic_splits_status: ObservationStatus::at("ready", Some(Utc::now()), None),
        canonical_id: Some(REPO.into()),
        detail: true,
        status: "ready".into(),
        error: None,
        peers: assemble_peer_details(
            peers::repo_peers(&agents, REPO, &[], None, Utc::now()),
            &agents,
            &locks,
            &[REPO.into()],
            tasks.as_ref(),
            Some("test-user"),
            Utc::now(),
        ),
    };
    let text = render_peer_report(&report, 80, true);
    assert!(
        text.starts_with("[OK] 1 peer supervisor | 1 live\n"),
        "{text}"
    );
    assert!(
        text.contains("Synced task title")
            && text.contains("cas-local")
            && text.contains("cas-remote"),
        "{text}"
    );
    assert!(
        text.contains("Owner qa-worker") && text.contains("title not synced"),
        "{text}"
    );
    assert!(
        !text.contains('~') && !text.contains(owner_id) && !text.contains("cas-foreign"),
        "{text}"
    );
    let data = serde_json::to_value(&report).unwrap();
    let claims = data["peers"][0]["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0]["agent_id"], owner_id);
    assert_eq!(claims[0]["task_id"], "cas-local");
    assert_eq!(claims[0]["lock_key"], local_key);
}

#[test]
fn cas_cd42_two_machine_inventory_preserves_private_stale_and_claim_authority() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let now = Utc::now();
    let observed = now - Duration::seconds(400);
    let mut epic = crate::types::Task::new("cas-epic".into(), "Shared epic".into());
    epic.task_type = crate::types::TaskType::Epic;
    epic.branch = Some("epic/target".into());
    tasks.add(&epic).unwrap();
    let mut parked = crate::types::Task::new("cas-parked".into(), "Parked work".into());
    parked.status = crate::types::TaskStatus::AwaitingMerge;
    parked.assignee = Some("worker-a".into());
    parked.deliverables.parked_branch = Some("factory/worker-a".into());
    parked.deliverables.factory_branch_anchor = Some("a".repeat(40));
    parked.deliverables.work_target = Some(crate::types::WorkTarget {
        repo_selector: "project:widgets".into(),
        target_branch: "epic/target".into(),
    });
    tasks.add(&parked).unwrap();
    let mut unlinked = parked.clone();
    unlinked.id = "cas-unlinked".into();
    tasks.add(&unlinked).unwrap();
    tasks
        .add_dependency(&crate::types::Dependency::new(
            parked.id.clone(),
            epic.id.clone(),
            crate::types::DependencyType::ParentChild,
        ))
        .unwrap();
    let prs = vec![
        PeerPr {
            url: Some("https://github.com/acme/widgets/pull/42".into()),
            head_sha: "b".repeat(40),
            branch: "factory/worker-a".into(),
            number: 42,
            state: "OPEN".into(),
            in_merge_queue: true,
        },
        PeerPr {
            url: Some("https://github.com/acme/widgets/pull/43".into()),
            head_sha: "c".repeat(40),
            branch: "epic/target".into(),
            number: 43,
            state: "OPEN".into(),
            in_merge_queue: false,
        },
    ];
    cas_store::insert_epic_split(
        &root,
        &cas_store::EpicSplit {
            id: "split-lanes".into(),
            epic_id: "cas-epic".into(),
            local_agent_id: "sup-a".into(),
            peer_agent_id: "sup-b".into(),
            peer_name: "sup-b".into(),
            peer_machine: "beta".into(),
            direction: cas_store::SplitDirection::Outgoing,
            state: cas_store::SplitState::Accepted,
            mine: vec!["cas-parked".into()],
            theirs: vec!["cas-unlinked".into()],
            reason: None,
            message_id: None,
            expires_at: now + Duration::hours(1),
            created_at: now,
            decided_at: Some(now),
            decided_by: None,
            cleanup_pending: false,
            expiry_notice_pending: false,
        },
    )
    .unwrap();
    save_pr_snapshot(&root, prs, observed);
    let mut agents = vec![
        agent("sup-a", "supervisor", "same", "alpha", REPO, 1),
        agent("sup-b", "supervisor", "same", "beta", REPO, 400),
        agent("worker-a", "worker", "same", "alpha", REPO, 1),
        agent("worker-b", "worker", "same", "beta", REPO, 1),
        agent("private", "supervisor", "private", "gamma", REPO, 1),
    ];
    let mut local = crate::types::Agent::new("sup-a".into(), "sup-a".into());
    local.role = crate::types::AgentRole::Supervisor;
    local.metadata = agents[0].metadata.clone();
    local
        .metadata
        .insert("clone_path".into(), "/alpha/worktree".into());
    let published = presence_agent(&root, &local);
    // Exercise the production presence metadata path, not a second serializer.
    let coordinator = super::super::CloudCoordinator::new(super::super::CloudConfig {
        token: Some("test-token".into()),
        ..Default::default()
    })
    .unwrap();
    let metadata = super::super::presence::heartbeat_metadata(
        &coordinator,
        &published,
        Some("cas-epic"),
        Some("epic/target"),
    );
    for (key, value) in metadata {
        agents[0]
            .metadata
            .insert(key, value.as_str().unwrap().into());
    }
    agents[4].user_id = Some("other-user".into());
    agents[4].team_id = None; // Personal accounts still expose no cross-account claims.
    agents[4].metadata.insert(
        work::PR_META.into(),
        published.metadata[work::PR_META].clone(),
    );
    let locks = vec![
        lease("cas-epic", "sup-a", LeaseStatus::Active, 600),
        lease("cas-secret", "private", LeaseStatus::Active, 600),
    ];
    let details = assemble_peer_details(
        peers::repo_peers(&agents, REPO, &[], None, now),
        &agents,
        &locks,
        &[],
        tasks.as_ref(),
        Some("test-user"),
        now,
    );
    let a = details.iter().find(|d| d.owner == "sup-a").unwrap();
    assert_eq!(a.work.epics[0].owner.as_deref(), Some("sup-a"));
    assert_eq!(a.work.splits_status.state, "ready");
    assert_eq!(a.work.splits[0].id, "split-lanes");
    assert_eq!(
        a.work.splits[0].target_branch.as_deref(),
        Some("epic/target")
    );
    assert_eq!(a.work.splits[0].mine, ["cas-parked"]);
    assert!(
        a.work
            .tasks
            .iter()
            .any(|t| t.task_id == "cas-unlinked" && t.owner.is_none()),
        "parking releases claims, but a unique recorded assignment remains an observation"
    );
    assert_eq!(a.work.tasks[0].assignee.as_deref(), Some("worker-a"));
    assert!(
        a.work.tasks[0].owner.is_none(),
        "assignment and PR activity must not fabricate ownership"
    );
    assert_eq!(a.work.prs_status.state, "stale");
    assert_eq!(
        a.work.prs_status.observed_at,
        Some(observed),
        "heartbeat must not refresh the PR observation time"
    );
    assert_eq!(
        a.work.tasks[0].pr.as_ref().unwrap().head_sha,
        "b".repeat(40)
    );
    for kind in [
        "awaiting_merge",
        "parked_delivery",
        "open_epic_pr",
        "merge_queue",
    ] {
        assert!(
            a.work.waiting_merges.iter().any(|w| w.kind == kind),
            "missing {kind}"
        );
    }
    assert_eq!(
        a.workers
            .iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>(),
        ["worker-a"]
    );
    let parked_head = a
        .work
        .waiting_merges
        .iter()
        .find(|w| w.kind == "parked_delivery")
        .unwrap();
    assert_eq!(
        parked_head.head_sha.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    let queue_head = a
        .work
        .waiting_merges
        .iter()
        .find(|w| w.kind == "merge_queue")
        .unwrap();
    assert_eq!(
        queue_head.head_sha.as_deref(),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    assert_eq!(queue_head.observed_at, Some(observed));
    let b = details.iter().find(|d| d.owner == "sup-b").unwrap();
    assert_eq!(b.work.observations_status.state, "stale");
    assert_eq!(b.work.prs_status.state, "unavailable");
    assert_eq!(b.workers[0].machine, "beta");
    let private = details.iter().find(|d| d.owner == "private").unwrap();
    assert_eq!(private.work.claims_status.state, "private");
    assert_eq!(private.work.tasks_status.state, "private");
    assert!(private.claims.is_empty() && private.work.prs.is_empty());
    assert!(
        !serde_json::to_string(private)
            .unwrap()
            .contains("cas-secret")
    );
    assert!(
        crate::store::open_agent_store(&root)
            .unwrap()
            .list(None)
            .unwrap()
            .is_empty(),
        "remote inventory must not register local agents"
    );
    let mut report = PeerReport::new(&root, None, true);
    report.peers = details;
    report.canonical_id = Some(REPO.into());
    report.cloud_presence_message.clear();
    report.mailbox_status.clear();
    let text = render_peer_report(&report, 80, true);
    for expected in [
        "Owner sup-a",
        "private work",
        "PRs stale at",
        "PRs unavailable at",
        "/alpha/worktree",
        "Head bbbbb",
        "open_epic_pr",
        "merge_queue",
        "Read-only:",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("Slack") && !text.contains("cas-secret"));
}

#[test]
fn cas_cd42_pr_cache_is_bounded_and_corrupt_metadata_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let now = Utc::now();
    let prs = (0..101)
        .map(|number| PeerPr {
            url: Some("x".repeat(1000)),
            head_sha: "a".repeat(40),
            branch: format!("factory/{number}"),
            number,
            state: "OPEN".into(),
            in_merge_queue: false,
        })
        .collect();
    save_pr_snapshot(&root, prs, now);
    let bytes = std::fs::read(root.join("peer-pr-observations.json")).unwrap();
    assert!(bytes.len() <= 32 * 1024);
    let snapshot: work::PrSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert!(snapshot.truncated && !snapshot.prs.is_empty() && snapshot.prs.len() < 100);
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    for bad in ["{broken".to_string(), "x".repeat(32 * 1024 + 1)] {
        let mut supervisor = agent("sup", "supervisor", "lane", "alpha", REPO, 1);
        supervisor.metadata.insert(work::PR_META.into(), bad);
        let agents = vec![supervisor];
        let details = assemble_peer_details(
            peers::repo_peers(&agents, REPO, &[], None, now),
            &agents,
            &[],
            &[],
            tasks.as_ref(),
            Some("test-user"),
            now,
        );
        assert_eq!(details[0].work.prs_status.state, "unavailable");
        assert!(details[0].work.prs.is_empty());
    }
}

#[tokio::test]
async fn cas_cd42_unavailable_cloud_sections_keep_observation_time_and_no_registry_writes() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let activation = json!({
        "schema": "cassy.coordination/v1", "code": "completed",
        "bindings": [
            {"binding": {"project_id": REPO, "channel_id": "C08MANAGED"}, "strict_activation": "evidenced"},
            {"binding": {"project_id": REPO, "channel_id": "C08LEGACY"}, "strict_activation": "not_requested"}
        ],
        "receipt": {"observed_at": "2026-10-17T12:00:00Z", "next_action": "none"}
    });
    Mock::given(method("GET"))
        .and(path("/api/coordination/bindings/activation"))
        .and(query_param("registration_id", "viewer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(activation.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let supervisor = agent("remote", "supervisor", "remote-lane", "beta", REPO, 1);
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"agents": [supervisor, agent("remote-worker", "worker", "remote-lane", "beta", REPO, 1)]})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents/locks"))
        .respond_with(ResponseTemplate::new(503).set_body_string("claims offline"))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let (report, text) = tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        std::fs::write(
            root.join("config.toml"),
            format!("[project]\ncanonical_id = {REPO:?}\n"),
        )
        .unwrap();
        super::super::CloudConfig {
            endpoint,
            token: Some("fixture-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        let mut report = discover_peer_report(&root, Some("viewer"), true, None).unwrap();
        report.activation = super::super::activation::observe(&root, Some("viewer"));
        assert!(
            crate::store::open_agent_store(&root)
                .unwrap()
                .list(None)
                .unwrap()
                .is_empty()
        );
        let text = render_peer_report(&report, 80, true);
        (serde_json::to_value(report).unwrap(), text)
    })
    .await
    .unwrap();
    assert_eq!(report["status"], "partial");
    assert_eq!(report["activation"], activation, "activation keeps Cloud's receipt verbatim");
    assert!(text.contains("Slack activation: evidenced 1, not_requested 1"), "{text}");
    assert_eq!(report["peers"][0]["claims_status"]["state"], "unavailable");
    assert!(report["peers"][0]["claims_status"]["observed_at"].is_string());
    assert!(text.starts_with("[WARN] 1 peer supervisor"));
    assert!(text.contains("Claims unavailable at") && text.contains("claims offline"));
    assert!(!text.contains("Claims  0"));
    assert!(
        text.contains("Tasks unknown") && !text.contains("Tasks none"),
        "{text}"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method.as_str() == "GET")
    );
}

/// cas-9f53: a worker `peers detail=true` lists under a peer supervisor is
/// resolvable by name or name@machine, and names that supervisor as relay.
#[test]
fn cas_9f53_a_listed_peer_worker_resolves_to_its_owning_supervisor() {
    let agents = vec![
        agent("rapid-panther-80", "supervisor", "steady-lynx", "soundwave", REPO, 5),
        agent("bright-lynx-31", "worker", "steady-lynx", "soundwave", REPO, 5),
        agent("gone-worker", "worker", "steady-lynx", "soundwave", REPO, 3_600),
        agent("other-session", "worker", "other-lynx", "soundwave", REPO, 5),
    ];
    let supervisors = peers::repo_peers(&agents, REPO, &[], None, Utc::now());
    let workers = live_peer_workers(&supervisors, &agents, &[], Utc::now());
    assert_eq!(
        workers.iter().map(|worker| worker.name.as_str()).collect::<Vec<_>>(),
        ["bright-lynx-31"],
        "the same live session members peers lists"
    );
    let worker = &workers[0];
    assert!(worker.matches("bright-lynx-31") && worker.matches("Bright-Lynx-31@soundwave"));
    assert!(!worker.matches("bright-lynx-31@prowl"));
    assert_eq!(worker.supervisor.name, "rapid-panther-80");
    assert_eq!(worker.supervisor.session.as_deref(), Some("steady-lynx"));
}

/// cas-47a2: reset retires a dead worker's branch binding; once the new
/// worker starts, the peer view names the new worker's branch, not the dead
/// one as a parked delivery, and the old branch survives only as history.
#[tokio::test]
async fn cas_47a2_reset_then_new_worker_start_shows_the_new_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let mut task = crate::types::Task::new("cas-reset".into(), "Reassigned work".into());
    task.status = crate::types::TaskStatus::InProgress;
    task.assignee = Some("dead-worker".into());
    // The dead worker's commit-time binding (attribution hook).
    task.deliverables.parked_branch = Some("factory/dead-worker-cas-reset".into());
    task.deliverables.factory_branch_anchor = Some("a".repeat(40));
    tasks.add(&task).unwrap();

    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    let reset_task = |task_id: &str| {
        core.cas_task_reset(rmcp::handler::server::wrapper::Parameters(
            crate::mcp::tools::TaskReleaseRequest { task_id: task_id.into(), force: None },
        ))
    };
    reset_task("cas-reset").await.unwrap();
    let reset = tasks.get("cas-reset").unwrap();
    assert_eq!(reset.deliverables.parked_branch, None);
    assert_eq!(reset.deliverables.factory_branch_anchor, None);
    assert_eq!(reset.deliverables.handoff_branches, ["factory/dead-worker-cas-reset"]);
    assert_eq!(reset.deliverables.historical_factory_branch_anchors, ["a".repeat(40)]);

    // The new worker starts: assigned, in progress, live lease on its branch.
    let mut started = reset;
    started.status = crate::types::TaskStatus::InProgress;
    started.assignee = Some("new-worker".into());
    tasks.update(&started).unwrap();
    let mut worker = agent("new-worker", "worker", "lane", "alpha", REPO, 1);
    worker
        .metadata
        .insert("branch".into(), "factory/new-worker-cas-reset".into());
    let agents = vec![agent("sup", "supervisor", "lane", "alpha", REPO, 1), worker];
    let locks = vec![lease("cas-reset", "new-worker", LeaseStatus::Active, 600)];
    let details = assemble_peer_details(
        peers::repo_peers(&agents, REPO, &[], None, Utc::now()),
        &agents,
        &locks,
        &[],
        tasks.as_ref(),
        Some("test-user"),
        Utc::now(),
    );
    let work = &details[0].work;
    let row = work.tasks.iter().find(|t| t.task_id == "cas-reset").unwrap();
    assert_eq!(row.branch.as_deref(), Some("factory/new-worker-cas-reset"));
    assert_eq!(row.head_sha, None);
    assert!(
        work.waiting_merges.iter().all(|w| w.kind != "parked_delivery"),
        "{:?}",
        work.waiting_merges
    );

    // cas-b28c: the stranded-branch gate still measures the retired lane.
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&["checkout", "-q", "-b", "factory/dead-worker-cas-reset"]);
    git(&["commit", "-q", "--allow-empty", "-m", "dead worker's stranded work"]);
    git(&["checkout", "-q", "main"]);
    let gate = crate::mcp::tools::core::task::lifecycle::close_ops::collect_epic_branch_statuses(
        &[tasks.get("cas-reset").unwrap()],
        "main",
        repo.path(),
    );
    assert_eq!(gate[0].factory_branch.as_deref(), Some("factory/dead-worker-cas-reset"));
    assert_eq!(gate[0].unmerged_count, 1, "{:?}", gate[0]);

    // cas-2b10: an old-style shared lane retired by reset is still measured,
    // and commits claiming another task strand nothing here (cas-dbbf).
    let mut shared = crate::types::Task::new("cas-shared".into(), "Shared-lane work".into());
    shared.status = crate::types::TaskStatus::InProgress;
    shared.assignee = Some("dead-worker".into());
    shared.deliverables.parked_branch = Some("factory/dead-worker".into());
    tasks.add(&shared).unwrap();
    reset_task("cas-shared").await.unwrap();
    git(&["checkout", "-q", "-b", "factory/dead-worker", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "feat: later lane work (cas-beef)"]);
    let measure = || {
        crate::mcp::tools::core::task::lifecycle::close_ops::collect_epic_branch_statuses(
            &[tasks.get("cas-shared").unwrap()],
            "main",
            repo.path(),
        )
        .remove(0)
    };
    let row = measure();
    assert_eq!(row.factory_branch.as_deref(), Some("factory/dead-worker"), "{row:?}");
    assert_eq!(row.unmerged_count, 0, "{row:?}");
    git(&["commit", "-q", "--allow-empty", "-m", "dead worker's own shared-lane work"]);
    assert_eq!(measure().unmerged_count, 2, "own work blocks epic close");
}
