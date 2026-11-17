use super::*;
use crate::test_support::TestEnvGuard;
use tempfile::TempDir;

const ISSUE_CLAIM_PROJECT: &str = "github.com/richards-llc/cassy";

fn source_claim_lock(request: &wiremock::Request, agent_id: &str) -> serde_json::Value {
    let source_key = request
        .url
        .path()
        .trim_start_matches("/api/agents/tasks/")
        .split('/')
        .next()
        .expect("claim path contains its source key");
    let mut lock = serde_json::to_value(cas_types::TaskLease::new(
        source_key.to_string(),
        agent_id.to_string(),
        600,
    ))
    .expect("serialize source claim lock");
    lock["id"] = serde_json::json!(format!("source-lock-{source_key}"));
    lock
}

async fn source_claim_fixture() -> (
    wiremock::MockServer,
    TempDir,
    CasCore,
    crate::types::Agent,
) {
    use std::process::Command;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let temp = TempDir::new().expect("temporary project");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .output()
            .expect("git runs for issue-repo fixture");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/richards-llc/cassy.git",
    ]);
    let cas_root = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create project CAS directory");
    let core = CasCore::with_daemon(cas_root.clone(), None, None);
    let agent = crate::types::Agent::new_with_role(
        "issue-source-test-agent".into(),
        "issue-source-test".into(),
        crate::types::AgentRole::Supervisor,
    );
    core.open_agent_store()
        .expect("agent store")
        .register(&agent)
        .expect("register source owner");
    core.set_agent_id_for_testing(agent.id.clone());
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, ISSUE_CLAIM_PROJECT)
        .expect("pin test project identity");
    crate::cloud::CloudConfig {
        endpoint: server.uri(),
        token: Some("source-claim-test-token".into()),
        ..Default::default()
    }
    .save_to_cas_dir(&cas_root)
    .expect("save isolated test cloud config");

    let claim_agent = agent.id.clone();
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/agents/tasks/.+/claim$"))
        .respond_with(move |request: &wiremock::Request| {
            let lock = source_claim_lock(request, &claim_agent);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "result": "claimed",
                "lock": lock,
            }))
        })
        .mount(&server)
        .await;
    let renew_agent = agent.id.clone();
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/agents/tasks/.+/renew$"))
        .respond_with(move |request: &wiremock::Request| {
            let lock = source_claim_lock(request, &renew_agent);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "lock": lock }))
        })
        .mount(&server)
        .await;
    (server, temp, core, agent)
}

fn create_request(title: &str, description: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        title: title.to_string(),
        description: Some(description.to_string()),
        priority: 2,
        task_type: "bug".to_string(),
        risk: Some("none".to_string()),
        door: None,
        review_dispatch_id: None,
        proof_targets: None,
        supervisor_override: None,
        reason: None,
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
        depth: None,
    }
}

fn source_claim_path(source: &str) -> String {
    let source_key = crate::cloud::peer_claims::github_source_key(source).unwrap();
    let task_key = crate::cloud::peer_claims::claim_key(ISSUE_CLAIM_PROJECT, &source_key);
    format!("/api/agents/tasks/{task_key}/claim")
}

#[tokio::test]
async fn task_create_near_issue_without_reference_does_not_claim_it() {
    let _env = TestEnvGuard::temp_home();
    let (server, _temp, core, agent) = source_claim_fixture().await;
    let title = "Release repair after freeze lift";
    core.cas_task_create(Parameters(create_request(
        title,
        "Prepare the release repair that follows the freeze lift on the active train.",
    )))
    .await
    .expect("create unrelated task");

    let created = core
        .open_task_store()
        .expect("task store")
        .list(None)
        .expect("list tasks")
        .into_iter()
        .find(|task| task.title == title)
        .expect("created task");
    assert_eq!(created.external_ref, None);
    assert!(server.received_requests().await.unwrap().is_empty());
    let persisted_agent = core.open_agent_store().unwrap().get(&agent.id).unwrap();
    assert!(
        persisted_agent
            .metadata
            .get(crate::cloud::peer_claims::META_SOURCE_CLAIMS)
            .is_none()
    );
}

#[tokio::test]
async fn task_create_description_citation_does_not_claim_issue() {
    let _env = TestEnvGuard::temp_home();
    let (server, _temp, core, agent) = source_claim_fixture().await;
    let title = "Review release repair follow-up";
    core.cas_task_create(Parameters(create_request(
        title,
        "This report cites GH #2027 as context while investigating the freeze.",
    )))
    .await
    .expect("create context-only task");

    let created = core
        .open_task_store()
        .expect("task store")
        .list(None)
        .expect("list tasks")
        .into_iter()
        .find(|task| task.title == title)
        .expect("created task");
    assert_eq!(created.external_ref, None);
    assert!(server.received_requests().await.unwrap().is_empty());
    let persisted_agent = core.open_agent_store().unwrap().get(&agent.id).unwrap();
    assert!(
        persisted_agent
            .metadata
            .get(crate::cloud::peer_claims::META_SOURCE_CLAIMS)
            .is_none()
    );
}

#[tokio::test]
async fn task_create_claims_only_explicit_github_bindings() {
    let _env = TestEnvGuard::temp_home();
    let (server, _temp, core, agent) = source_claim_fixture().await;
    let incidental_title = "Review GH #2027 while investigating the freeze";
    core.cas_task_create(Parameters(create_request(
        incidental_title,
        "The release repair is related to the freeze, but this task does not close it.",
    )))
    .await
    .expect("create citation-only task");

    let explicit_ref = "https://github.com/richards-llc/cassy/issues/2032";
    let mut explicit = create_request("Own the source binding", "");
    explicit.external_ref = Some(explicit_ref.into());
    core.cas_task_create(Parameters(explicit))
        .await
        .expect("create explicit-ref task");

    let fixes_issue = "https://github.com/richards-llc/cassy/issues/2033";
    core.cas_task_create(Parameters(create_request(
        "Repair the issue binding behavior",
        "Fixes #2033 when the task reaches the release lane.",
    )))
    .await
    .expect("create closing-reference task");

    let closes_issue = "https://github.com/richards-llc/cassy/issues/2034";
    core.cas_task_create(Parameters(create_request(
        "Repair another issue binding behavior",
        "Closes #2034 when the task reaches the release lane.",
    )))
    .await
    .expect("create closes-reference task");

    for title in [
        "Richards-LLC/cassy#2035: owner/repo title binding",
        "#2036: default-repo title binding",
        "Title suffix binding (GH #2037)",
    ] {
        core.cas_task_create(Parameters(create_request(title, "")))
            .await
            .expect("create issue-bound intake task");
    }

    let tasks = core
        .open_task_store()
        .expect("task store")
        .list(None)
        .expect("list tasks");
    assert_eq!(
        tasks
            .iter()
            .find(|task| task.title == incidental_title)
            .expect("citation task")
            .external_ref,
        None
    );
    assert_eq!(
        tasks
            .iter()
            .find(|task| task.title == "Own the source binding")
            .expect("explicit task")
            .external_ref
            .as_deref(),
        Some(explicit_ref)
    );
    assert_eq!(
        tasks
            .iter()
            .find(|task| task.title == "Repair the issue binding behavior")
            .expect("closing task")
            .external_ref
            .as_deref(),
        Some(fixes_issue)
    );
    assert_eq!(
        tasks
            .iter()
            .find(|task| task.title == "Repair another issue binding behavior")
            .expect("closes task")
            .external_ref
            .as_deref(),
        Some(closes_issue)
    );

    let requests = server.received_requests().await.unwrap();
    let claims: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/claim"))
        .map(|request| request.url.path().to_string())
        .collect();
    let title_bindings = [2035, 2036, 2037].map(|number| {
        format!("https://github.com/richards-llc/cassy/issues/{number}")
    });
    let expected_claims = [explicit_ref, fixes_issue, closes_issue]
        .into_iter()
        .map(source_claim_path)
        .chain(title_bindings.iter().map(|source| source_claim_path(source)))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        claims.into_iter().collect::<std::collections::BTreeSet<_>>(),
        expected_claims,
        "only explicit external_ref, issue-owning title forms, and Fixes/Closes #N should reach the source claim path"
    );

    let persisted_agent = core.open_agent_store().unwrap().get(&agent.id).unwrap();
    let mut source_claims: Vec<String> = serde_json::from_str(
        persisted_agent
            .metadata
            .get(crate::cloud::peer_claims::META_SOURCE_CLAIMS)
            .expect("accepted bindings recorded as owned source claims"),
    )
    .expect("source claim metadata");
    source_claims.sort();
    let mut expected_sources = [explicit_ref, fixes_issue, closes_issue]
        .into_iter()
        .map(|source| crate::cloud::peer_claims::github_source_key(source).unwrap())
        .chain(
            title_bindings
                .iter()
                .map(|source| crate::cloud::peer_claims::github_source_key(source).unwrap()),
        )
        .collect::<Vec<_>>();
    expected_sources.sort();
    assert_eq!(source_claims, expected_sources);
}
