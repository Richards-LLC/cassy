//! cas-e477: peer supervisor discovery model.
use super::*;
use crate::types::AgentRole;
use chrono::Duration;

const REPO: &str = "github.com/acme/widgets";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-11T03:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn info(
    id: &str,
    status: &str,
    age_secs: i64,
    metadata: &[(&str, &str)],
    machine_id: Option<&str>,
) -> AgentInfo {
    AgentInfo {
        id: id.to_string(),
        name: format!("name-{id}"),
        agent_type: "primary".to_string(),
        status: status.to_string(),
        pid: None,
        session_id: None,
        machine_id: machine_id.map(str::to_string),
        last_heartbeat: (now() - Duration::seconds(age_secs)).to_rfc3339(),
        active_tasks: 0,
        metadata: metadata
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn supervisor(id: &str, repo: &str, age_secs: i64) -> AgentInfo {
    info(
        id,
        "active",
        age_secs,
        &[
            (META_ROLE, "supervisor"),
            (META_CANONICAL_ID, repo),
            (META_FOCUS, "cas-571d"),
            (META_FACTORY_SESSION, "factory-a"),
            (META_HOSTNAME, "box-a"),
        ],
        Some("machine-a"),
    )
}

#[test]
fn identity_metadata_carries_role_repo_focus_session_and_host() {
    let mut agent = Agent::new("sup-1".into(), "bright-lark-8".into());
    agent.role = AgentRole::Supervisor;
    agent.factory_session = Some("factory-a".into());
    agent
        .metadata
        .insert("clone_path".into(), "/src/widgets".into());

    let meta = identity_metadata(&agent, Some(REPO), Some("cas-571d"), Some("box-a"));
    assert_eq!(meta.get(META_ROLE).map(String::as_str), Some("supervisor"));
    assert_eq!(meta.get(META_CANONICAL_ID).map(String::as_str), Some(REPO));
    assert_eq!(meta.get(META_FOCUS).map(String::as_str), Some("cas-571d"));
    assert_eq!(
        meta.get(META_FACTORY_SESSION).map(String::as_str),
        Some("factory-a")
    );
    assert_eq!(meta.get(META_HOSTNAME).map(String::as_str), Some("box-a"));
    assert_eq!(
        meta.get("clone_path").map(String::as_str),
        Some("/src/widgets"),
        "the agent's own metadata is kept"
    );

    let bare = identity_metadata(&agent, None, Some("  "), None);
    assert!(!bare.contains_key(META_CANONICAL_ID));
    assert!(!bare.contains_key(META_FOCUS), "blank focus is omitted");
    assert!(!bare.contains_key(META_HOSTNAME));
}

#[test]
fn repo_peers_lists_same_repo_supervisors_except_self_with_liveness() {
    let agents = vec![
        supervisor("me", REPO, 10),
        supervisor("live-peer", "git@github.com:acme/widgets.git", 20),
        supervisor("stale-peer", REPO, PEER_LIVE_SECS + 60),
        supervisor("other-repo", "github.com/acme/gadgets", 5),
        supervisor("ancient", REPO, PEER_LISTING_WINDOW_SECS + 1),
        info("shut", "shutdown", 5, &[(META_ROLE, "supervisor"), (META_CANONICAL_ID, REPO)], None),
        info("worker", "active", 5, &[(META_ROLE, "worker"), (META_CANONICAL_ID, REPO)], None),
        info("legacy", "active", 5, &[], None),
    ];

    let peers = repo_peers(&agents, REPO, &[], Some("me"), now());
    let ids: Vec<&str> = peers.iter().map(|p| p.agent_id.as_str()).collect();
    assert_eq!(ids, ["live-peer", "stale-peer"], "live first, then stale");

    let live = &peers[0];
    assert!(live.live);
    assert_eq!(live.heartbeat_age_secs, 20);
    assert_eq!(live.machine, "box-a");
    assert_eq!(live.machine_id.as_deref(), Some("machine-a"));
    assert_eq!(live.session.as_deref(), Some("factory-a"));
    assert_eq!(live.focus.as_deref(), Some("cas-571d"));

    let stale = &peers[1];
    assert!(!stale.live, "a heartbeat older than PEER_LIVE_SECS is stale");
    assert_eq!(stale.heartbeat_age_secs, PEER_LIVE_SECS + 60);
}

#[test]
fn repo_peers_honours_the_registered_alias_class() {
    let agents = vec![supervisor("renamed", "github.com/acme/widgets-old", 5)];
    assert!(repo_peers(&agents, REPO, &[], None, now()).is_empty());
    let class = vec![REPO.to_string(), "github.com/acme/widgets-old".to_string()];
    assert_eq!(repo_peers(&agents, REPO, &class, None, now()).len(), 1);
}

#[test]
fn repo_peers_falls_back_to_machine_id_and_unknown() {
    let mut no_host = supervisor("p", REPO, 5);
    no_host.metadata.remove(META_HOSTNAME);
    let mut nothing = supervisor("q", REPO, 6);
    nothing.metadata.remove(META_HOSTNAME);
    nothing.machine_id = None;
    let peers = repo_peers(&[no_host, nothing], REPO, &[], None, now());
    assert_eq!(peers[0].machine, "machine-a");
    assert_eq!(peers[1].machine, "unknown");
}

#[test]
fn render_peers_names_machine_session_focus_age_and_liveness() {
    let agents = vec![
        supervisor("live-peer", REPO, 20),
        supervisor("stale-peer", REPO, 900),
    ];
    let peers = repo_peers(&agents, REPO, &[], None, now());
    let text = render_peers(REPO, &peers);
    assert!(text.contains(REPO), "{text}");
    assert!(text.contains("name-live-peer"), "{text}");
    assert!(text.contains("box-a"), "{text}");
    assert!(text.contains("factory-a"), "{text}");
    assert!(text.contains("cas-571d"), "{text}");
    assert!(text.contains("live"), "{text}");
    assert!(text.contains("stale"), "{text}");
    assert!(text.contains("20s"), "{text}");
    assert!(text.contains("15m"), "{text}");

    let none = render_peers(REPO, &[]);
    assert!(none.contains("no other supervisors"), "{none}");
}
