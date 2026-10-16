//! GH #1203: a staging walk is one receipt for its listed deliveries.
use crate::support::*;
use cas::mcp::{CasCore, CasService};
use cas::mcp::tools::TaskCloseRequest;
use cas::store::{open_agent_store, open_task_store};
use cas::types::{Agent, AgentRole, Task, TaskStatus};
use cas_mcp::types::VerificationRequest;
use rmcp::handler::server::wrapper::Parameters;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Test").env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test").env("GIT_COMMITTER_EMAIL", "test@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().into()
}

struct Fx {
    _temp: tempfile::TempDir,
    core: CasCore,
    supervisor: CasService,
    repo: PathBuf,
    staging: String,
    artifact: String,
    ledger: PathBuf,
}

fn fixture(env: &mut TestEnvGuard) -> Fx {
    let (temp, core) = setup_cas(env);
    let repo = temp.path().to_path_buf();
    env.set("CAS_AGENT_ROLE", "supervisor");
    std::fs::write(repo.join(".cas/config.toml"),
        "[project]\ncanonical_id='batch-project'\n[verification]\nenabled=false\n[qa]\nmode='batch'\n").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "seed\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "seed"]);
    git(&repo, &["checkout", "-qb", "factory/test-agent"]);
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    for id in ["cas-bq01", "cas-bq02", "cas-bq03"] {
        let mut task = Task::new(id.into(), "Spacing".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("test-agent".into());
        task.demo_statement = "See the changed spacing on staging".into();
        tasks.add(&task).unwrap();
        std::fs::write(repo.join(format!("{id}.css")), "body{gap:8px}\n").unwrap();
        git(&repo, &["add", &format!("{id}.css")]);
        git(&repo, &["commit", "-qm", &format!("{id}: spacing")]);
        task = tasks.get(id).unwrap();
        task.deliverables.factory_branch_anchor = Some(git(&repo, &["rev-parse", "HEAD"]));
        tasks.update(&task).unwrap();
    }
    let staging = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "-f", "main", &staging]);
    let supervisor_id = "batch-supervisor";
    open_agent_store(&repo.join(".cas")).unwrap().register(&Agent::new_with_role(
        supervisor_id.into(), "batch-supervisor".into(), AgentRole::Supervisor)).unwrap();
    let supervisor = CasCore::with_daemon(repo.join(".cas"), None, None);
    supervisor.set_agent_id_for_testing(supervisor_id.into());
    let ledger = repo.join("LEDGER.md");
    std::fs::write(&ledger, "# Staging QA\ncas-bq01 PASS at 390/1440\ncas-bq02 PASS at 390/1440\n").unwrap();
    let store = cas_store::SqliteArtifactStore::open(&repo.join(".cas")).unwrap();
    let roots = cas::artifacts::paths::PublishRoots::new(&repo.join(".cas"), &repo.join("artifacts"), "cas-bq01");
    let context = cas::artifacts::PublishContext { store: &store, roots, task_id: "cas-bq01".into(), cloud: None };
    let artifact = cas::artifacts::publish(&context, &ledger).unwrap().artifact.id;
    Fx { _temp: temp, core, supervisor: CasService::new(supervisor, None), repo, staging, artifact, ledger }
}

impl Fx {
    fn request(&self) -> VerificationRequest {
        serde_json::from_value(serde_json::json!({
            "action":"batch_receipt", "task_ids":["cas-bq01","cas-bq02","cas-bq01"],
            "staging_sha":self.staging, "ledger_artifact_id":self.artifact,
            "ledger_path":self.ledger,
        })).unwrap()
    }
    async fn close(&self, id: &str) -> String {
        let task = open_task_store(&self.repo.join(".cas")).unwrap().get(id).unwrap();
        let req = TaskCloseRequest { id: id.into(), reason: Some("staging QA complete".into()),
            commit_receipt: task.deliverables.factory_branch_anchor,
            stranded_branch_override: None, supervisor_override: None,
            legacy_bypass_code_review: None, search_manifest: None };
        match self.core.cas_task_close(Parameters(req)).await {
            Ok(out) => extract_text(out), Err(err) => err.message.to_string(),
        }
    }
}

#[tokio::test]
async fn batch_qa_receipt_closes_listed_deliveries_cas_d697() {
    let mut env = TestEnvGuard::new();
    let fx = fixture(&mut env);
    let recorded = fx.supervisor.verification(Parameters(fx.request())).await
        .expect("one supervisor receipt must cover the staging walk");
    assert!(extract_text(recorded).contains("Batch QA receipt"));
    for id in ["cas-bq01", "cas-bq02"] {
        let result = fx.close(id).await;
        assert_eq!(open_task_store(&fx.repo.join(".cas")).unwrap().get(id).unwrap().status,
            TaskStatus::Closed, "{id}: {result}");
    }
    let unlisted = fx.close("cas-bq03").await;
    assert!(unlisted.contains("QA EVIDENCE"), "{unlisted}");
    assert_ne!(open_task_store(&fx.repo.join(".cas")).unwrap().get("cas-bq03").unwrap().status,
        TaskStatus::Closed);
    assert!(cas_store::list_qa_passes(&fx.repo.join(".cas"), "cas-bq01").unwrap().is_empty());
}
