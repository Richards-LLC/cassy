//! cas-d58d: same-repo peers are labelled with their owning account, and a
//! per-project `peers.same_repo_scope = own` keeps only the caller's own.
use super::*;
use crate::config::SameRepoScope;
use crate::test_support::TestEnvGuard;
use chrono::Duration;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const REPO: &str = "github.com/acme/widgets";
const ME: &str = "user-me-0001";
const TEAMMATE: &str = "user-bob-0002";

fn row(id: &str, name: &str, user: &str, owner: Option<&str>, machine: &str) -> AgentInfo {
    let mut metadata: HashMap<String, String> = [
        (META_ROLE, "supervisor"),
        (META_CANONICAL_ID, REPO),
        (META_FACTORY_SESSION, "factory-x"),
        (META_HOSTNAME, machine),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if let Some(owner) = owner {
        metadata.insert(META_OWNER.to_string(), owner.to_string());
    }
    AgentInfo {
        id: id.to_string(),
        name: name.to_string(),
        agent_type: "primary".to_string(),
        status: "active".to_string(),
        pid: None,
        session_id: None,
        machine_id: Some(format!("mid-{machine}")),
        last_heartbeat: (Utc::now() - Duration::seconds(10)).to_rfc3339(),
        active_tasks: 0,
        metadata,
        user_id: Some(user.to_string()),
        team_id: Some("team-1".to_string()),
        project_id: None,
    }
}

/// The caller (`me-self`), another of the caller's supervisors, and a
/// teammate's supervisor of the same repo on a shared team project.
fn agents() -> Vec<AgentInfo> {
    vec![
        row("me-self", "calm-owl-1", ME, Some("me@example.com"), "soundwave"),
        row("me-other", "swift-fox-2", ME, Some("me@example.com"), "prowl"),
        row("bob-sup", "bold-bear-3", TEAMMATE, Some("bob@example.com"), "bobbox"),
    ]
}

fn marked() -> Vec<Peer> {
    let agents = agents();
    let mut peers = repo_peers(&agents, REPO, &[], Some("me-self"), Utc::now());
    let viewer = viewer_user_id(&agents, Some("me-self"), || None);
    mark_own(&mut peers, viewer.as_deref());
    peers
}

#[test]
fn cas_d58d_every_peer_names_its_owning_account() {
    let peers = marked();
    let labels: Vec<(String, String)> = peers
        .iter()
        .map(|peer| (peer.name.clone(), owner_label(peer)))
        .collect();
    assert!(labels.contains(&("swift-fox-2".into(), "you".into())), "{labels:?}");
    assert!(
        labels.contains(&("bold-bear-3".into(), "teammate bob@example.com".into())),
        "{labels:?}"
    );
    let text = render_peers(REPO, &peers);
    assert!(text.contains("    owner you\n"), "{text}");
    assert!(text.contains("    owner teammate bob@example.com\n"), "{text}");
    for line in text.lines() {
        assert!(line.chars().count() <= 80, "wider than 80 columns: {line}");
    }
    // A registration without the owner email still names an account.
    let mut unnamed = peers.iter().find(|p| p.name == "bold-bear-3").unwrap().clone();
    unnamed.owner = None;
    assert_eq!(owner_label(&unnamed), "teammate user user-bob");
    unnamed.own = None;
    unnamed.user_id = None;
    assert_eq!(owner_label(&unnamed), "unknown account");
}

#[test]
fn cas_d58d_viewer_is_the_callers_own_row_else_the_single_own_account() {
    let agents = agents();
    assert_eq!(viewer_user_id(&agents, Some("me-self"), || None).as_deref(), Some(ME));
    // Not registered: the token's own agent list decides, never a project row.
    let own = vec![row("me-other", "swift-fox-2", ME, None, "prowl")];
    assert_eq!(viewer_user_id(&agents, Some("cli"), || Some(own)).as_deref(), Some(ME));
    assert_eq!(viewer_user_id(&agents, None, || Some(agents.clone())), None);
    assert_eq!(viewer_user_id(&agents, None, || None), None);
}

#[test]
fn cas_d58d_team_scope_delivers_to_a_teammate_and_own_scope_refuses() {
    // team (default): the teammate is listed and a plain-name send resolves.
    let (team, hidden) = apply_same_repo_scope(marked(), SameRepoScope::Team);
    assert!(hidden.is_empty());
    match resolve_same_repo_target(&team, &hidden, "bold-bear-3").unwrap() {
        SameRepoTarget::Peer(peer) => assert_eq!(owner_label(peer), "teammate bob@example.com"),
        other => panic!("team scope must deliver to the teammate: {other:?}"),
    }

    // own: only the caller's supervisors; the teammate is refused by name.
    let (own, hidden) = apply_same_repo_scope(marked(), SameRepoScope::Own);
    assert_eq!(own.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["swift-fox-2"]);
    assert_eq!(hidden.len(), 1);
    for target in ["bold-bear-3", "bold-bear-3@bobbox"] {
        match resolve_same_repo_target(&own, &hidden, target).unwrap() {
            SameRepoTarget::Refused(reason) => {
                assert!(reason.contains("teammate bob@example.com"), "{reason}");
                assert!(reason.contains("peers.same_repo_scope is own"), "{reason}");
                assert!(reason.contains("Nothing was sent"), "{reason}");
            }
            other => panic!("own scope must refuse {target}: {other:?}"),
        }
    }
    assert!(matches!(
        resolve_same_repo_target(&own, &hidden, "swift-fox-2").unwrap(),
        SameRepoTarget::Peer(_)
    ));
    assert_eq!(
        resolve_same_repo_target(&own, &hidden, "nobody").unwrap(),
        SameRepoTarget::NotFound
    );
    assert!(render_hidden_note(hidden.len()).contains("1 supervisor of other accounts hidden"));

    // An account that cannot be told is never admitted under own.
    let mut unknown = marked();
    mark_own(&mut unknown, None);
    let (own, hidden) = apply_same_repo_scope(unknown, SameRepoScope::Own);
    assert!(own.is_empty());
    assert_eq!(hidden.len(), 2);
}

/// `GET /api/agents`: with `project` the shared team project's agents of
/// every user; without it the token's own agents only.
struct FakeCloud;

impl Respond for FakeCloud {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let project = request.url.query_pairs().any(|(key, _)| key == "project");
        let agents: Vec<AgentInfo> = agents()
            .into_iter()
            .filter(|agent| project || agent.user_id.as_deref() == Some(ME))
            .collect();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "agents": agents }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cas_d58d_discovery_labels_peers_and_honours_the_project_scope() {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(FakeCloud).mount(&server).await;
    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("CAS_USER_CLOUD_JSON", home.join("no-user-cloud.json"));
    let cas_root = crate::store::init_cas_dir(&home).unwrap();
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, REPO).unwrap();
    let mut cloud = crate::cloud::CloudConfig::default();
    cloud.endpoint = server.uri();
    cloud.token = Some("synthetic-test-token".to_string());
    cloud.save_to_cas_dir(&cas_root).unwrap();

    let discover = |cas_root: std::path::PathBuf, self_id: &'static str| {
        tokio::task::spawn_blocking(move || discover_peers(&cas_root, Some(self_id)).unwrap())
    };
    let summary = |discovery: PeerDiscovery| match discovery {
        PeerDiscovery::Found { peers, hidden, .. } => (
            peers.iter().map(|p| (p.name.clone(), owner_label(p))).collect::<Vec<_>>(),
            hidden.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        ),
        other => panic!("expected peers, got {other:?}"),
    };

    // Default team scope: both supervisors, each labelled.
    assert_eq!(same_repo_scope(&cas_root), SameRepoScope::Team);
    let (listed, hidden) = summary(discover(cas_root.clone(), "me-self").await.unwrap());
    assert!(listed.contains(&("swift-fox-2".into(), "you".into())), "{listed:?}");
    assert!(listed.contains(&("bold-bear-3".into(), "teammate bob@example.com".into())), "{listed:?}");
    assert!(hidden.is_empty());

    // An unregistered caller (a plain terminal) still tells the accounts
    // apart from the token's own agent list.
    let (listed, _) = summary(discover(cas_root.clone(), "cli-terminal").await.unwrap());
    assert!(listed.contains(&("swift-fox-2".into(), "you".into())), "{listed:?}");
    assert!(listed.contains(&("calm-owl-1".into(), "you".into())), "{listed:?}");

    // own: the teammate is hidden, never listed.
    let mut config = crate::config::Config::load(&cas_root).unwrap();
    config.set("peers.same_repo_scope", "own").unwrap();
    config.save(&cas_root).unwrap();
    assert_eq!(same_repo_scope(&cas_root), SameRepoScope::Own);
    let (listed, hidden) = summary(discover(cas_root.clone(), "me-self").await.unwrap());
    assert_eq!(listed, [("swift-fox-2".to_string(), "you".to_string())]);
    assert_eq!(hidden, ["bold-bear-3"]);
    let section = tokio::task::spawn_blocking({
        let cas_root = cas_root.clone();
        move || peers_discovery_section(&cas_root, Some("me-self"))
    })
    .await
    .unwrap();
    assert!(section.contains("1 supervisor of other accounts hidden"), "{section}");
    assert!(!section.contains("bold-bear-3"), "{section}");
}

#[test]
fn cas_d58d_same_repo_scope_key_is_registered_operator_only_and_validated() {
    let mut config = crate::config::Config::default();
    assert_eq!(config.get("peers.same_repo_scope").as_deref(), Some("team"));
    config.set("peers.same_repo_scope", "own").unwrap();
    assert_eq!(config.get("peers.same_repo_scope").as_deref(), Some("own"));
    assert!(config.set("peers.same_repo_scope", "everyone").is_err());
    assert!(
        config
            .list()
            .iter()
            .any(|(key, value)| key == "peers.same_repo_scope" && value == "own")
    );
    let registry = crate::config::registry();
    let meta = registry.get("peers.same_repo_scope").expect("registered");
    assert_eq!(meta.default, "team");
    assert!(crate::config::operator_policy::is_operator_only_config_key("peers.same_repo_scope"));
}
