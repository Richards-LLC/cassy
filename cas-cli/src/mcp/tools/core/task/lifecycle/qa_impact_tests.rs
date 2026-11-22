use super::*;
use crate::mcp::tools::service::CasService;
use rmcp::handler::server::wrapper::Parameters;

fn git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git").arg("-C").arg(repo).args(args)
        .env("GIT_AUTHOR_NAME", "Fixture").env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Fixture").env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output().unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn registry(repo: &Path, id: &str) {
    std::fs::create_dir_all(repo.join("docs/qa")).unwrap();
    std::fs::write(repo.join("docs/qa/journeys.yaml"), format!(
        "version: 1\nupdated: '2026-11-21'\nmain_journeys: [{id}]\nsurfaces: ['app/**']\ninvariants:\n  ARRIVED: {{statement: Arrived, check: Read state}}\njourneys:\n- id: {id}\n  status: live\n  entry: {{url: /feature, trigger: Open}}\n  steps: [Arrive]\n  owners: ['app/**']\n  invariants: [ARRIVED]\n"
    )).unwrap();
}

fn fixture(env: &mut crate::test_support::TestEnvGuard) -> (tempfile::TempDir, CasService, Task, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-qb", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-qm", "base without metadata"]);
    let root = repo.join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='worker-metadata-fixture'\n[verification]\nenabled=false\n").unwrap();
    let worker = root.join("worktrees/reader");
    std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
    git(repo, &["worktree", "add", "-qb", "factory/reader", worker.to_str().unwrap()]);
    registry(&worker, "WORKER-J");
    std::fs::write(worker.join("docs/PRODUCT_OVERVIEW.md"), "# Worker product\n").unwrap();
    std::fs::create_dir_all(worker.join("app")).unwrap();
    std::fs::write(worker.join("app/feature.rs"), "pub fn feature() {}\n").unwrap();
    git(&worker, &["add", "."]);
    git(&worker, &["commit", "-qm", "worker metadata"]);
    // Start creates its lane from the declared target, whose metadata is
    // newer than the primary checkout's currently checked-out main.
    git(&worker, &["branch", "epic/metadata"]);
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &worker);
    env.set("CAS_AGENT_NAME", "reader");
    env.set("CAS_AGENT_ROLE", "worker");
    let core = CasCore::with_daemon(root, None, None);
    let mut agent = cas_types::Agent::new_with_role("reader-session".into(), "reader".into(), cas_types::AgentRole::Worker);
    agent.metadata.insert("clone_path".into(), worker.display().to_string());
    agent.pid = Some(std::process::id());
    core.open_agent_store().unwrap().register(&agent).unwrap();
    core.set_agent_id_for_testing(agent.id.clone());
    let mut task = Task::new("cas-metadata".into(), "Inspect app/feature.rs".into());
    task.assignee = Some(agent.name);
    task.origin_project = Some("worker-metadata-fixture".into());
    task.acceptance_criteria = "The feature path uses its own registry".into();
    task.notes = "Repro: caller supplied fixture".into();
    task.labels = vec!["from-finding".into(), "severity:broken".into(), "reach:secondary".into()];
    task.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:worker-metadata-fixture".into(), target_branch: "epic/metadata".into(),
    });
    core.open_task_store().unwrap().add(&task).unwrap();
    (dir, CasService::new(core, #[cfg(feature = "mcp-proxy")] None), task, worker)
}

#[tokio::test]
async fn cas_598f_show_and_start_read_assigned_checkout_metadata() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (dir, service, task, worker) = fixture(&mut env);
    for primary_has_registry in [false, true] {
        if primary_has_registry { registry(dir.path(), "PRIMARY-J"); }
        let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"show", "id":task.id, "full":true,
        })).unwrap())).await.unwrap();
        let data = answer.structured_content.expect("current task metadata, independent of saved notes");
        assert_eq!(data["qa_impact"]["registry"]["present"], true);
        assert!(data["qa_impact"]["journeys"].as_array().unwrap().iter().any(|j| j["id"] == "WORKER-J"));
        assert!(!data["qa_impact"]["journeys"].as_array().unwrap().iter().any(|j| j["id"] == "PRIMARY-J"));
    }
    let started = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"start", "id":task.id, "brief":false,
    })).unwrap())).await.unwrap();
    assert_eq!(started.is_error, Some(false));
    let started = serde_json::to_string(&started).unwrap();
    assert!(started.contains("WORKER-J"), "{started}");
    // A genuinely missing worker registry must not borrow the primary's.
    std::fs::remove_file(worker.join("docs/qa/journeys.yaml")).unwrap();
    let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"show", "id":task.id, "full":true,
    })).unwrap())).await.unwrap();
    assert_eq!(answer.structured_content.unwrap()["qa_impact"]["registry"]["present"], false);
    // Even a registered worker cannot substitute a different repository.
    let foreign = tempfile::tempdir().unwrap();
    git(foreign.path(), &["init", "-qb", "main"]);
    registry(foreign.path(), "PRIMARY-J");
    let agents = crate::store::open_agent_store(&dir.path().join(".cas")).unwrap();
    let mut agent = agents.get("reader-session").unwrap();
    agent.metadata.insert("clone_path".into(), foreign.path().display().to_string());
    agents.update(&agent).unwrap();
    let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"show", "id":task.id, "full":true,
    })).unwrap())).await.unwrap();
    let data = answer.structured_content.unwrap();
    assert!(data["qa_impact"].is_null());
    assert!(data["qa_impact_error"].is_string());
}

#[tokio::test]
async fn cas_598f_finding_labels_validate_the_assigned_registry() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (dir, _worker_service, task, _worker) = fixture(&mut env);
    registry(dir.path(), "PRIMARY-J");
    // Priority changes remain supervisor-only. The supervisor's checkout
    // also must not replace the task's assigned worker registry.
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_AGENT_NAME", "supervisor");
    env.set("CAS_CLONE_PATH", dir.path());
    let core = CasCore::with_daemon(dir.path().join(".cas"), None, None);
    let mut supervisor = cas_types::Agent::new_with_role("supervisor-session".into(), "supervisor".into(), cas_types::AgentRole::Supervisor);
    supervisor.pid = Some(std::process::id());
    core.open_agent_store().unwrap().register(&supervisor).unwrap();
    core.set_agent_id_for_testing(supervisor.id);
    let service = CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
    let updated = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"update", "id":task.id, "labels":"severity:broken,reach:main,journey:WORKER-J",
    })).unwrap())).await.unwrap();
    assert_eq!(updated.is_error, Some(false));
    let store = crate::store::open_task_store(&dir.path().join(".cas")).unwrap();
    assert_eq!(store.get(&task.id).unwrap().priority.0, 1);
    let before = store.get(&task.id).unwrap();
    let error = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"update", "id":task.id, "labels":"reach:main,journey:PRIMARY-J",
    })).unwrap())).await.unwrap_err();
    assert_eq!(error.data.unwrap()["rule_id"], "finding_main_journey_required");
    assert_eq!(store.get(&task.id).unwrap().labels, before.labels);
    assert_eq!(store.get(&task.id).unwrap().priority, before.priority);
}
