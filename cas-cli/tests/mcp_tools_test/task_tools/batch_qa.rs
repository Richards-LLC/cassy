//! GH #1203: a staging walk is one receipt for its listed deliveries.
use crate::support::*;
use cas::mcp::tools::TaskCloseRequest;
use cas::mcp::{CasCore, CasService};
use cas::store::{open_agent_store, open_task_store};
use cas::types::{Agent, AgentRole, Task, TaskStatus};
use cas_mcp::types::VerificationRequest;
use rmcp::handler::server::wrapper::Parameters;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    open_agent_store(&repo.join(".cas"))
        .unwrap()
        .register(&Agent::new_with_role(
            supervisor_id.into(),
            "batch-supervisor".into(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    let supervisor = CasCore::with_daemon(repo.join(".cas"), None, None);
    supervisor.set_agent_id_for_testing(supervisor_id.into());
    let ledger = repo.join("LEDGER.md");
    std::fs::write(
        &ledger,
        "# Staging QA\ncas-bq01 PASS at 390/1440\ncas-bq02 PASS at 390/1440\n",
    )
    .unwrap();
    let store = cas_store::SqliteArtifactStore::open(&repo.join(".cas")).unwrap();
    let roots = cas::artifacts::paths::PublishRoots::new(
        &repo.join(".cas"),
        &repo.join("artifacts"),
        "cas-bq01",
    );
    let context = cas::artifacts::PublishContext {
        store: &store,
        roots,
        task_id: "cas-bq01".into(),
        cloud: None,
    };
    let artifact = cas::artifacts::publish(&context, &ledger)
        .unwrap()
        .artifact
        .id;
    Fx {
        _temp: temp,
        core,
        supervisor: CasService::new(supervisor, None),
        repo,
        staging,
        artifact,
        ledger,
    }
}

impl Fx {
    fn request(&self) -> VerificationRequest {
        serde_json::from_value(serde_json::json!({
            "action":"batch_receipt", "task_ids":["cas-bq01","cas-bq02","cas-bq01"],
            "staging_sha":self.staging, "ledger_artifact_id":self.artifact,
            "ledger_path":self.ledger,
        }))
        .unwrap()
    }
    async fn close(&self, id: &str) -> String {
        let task = open_task_store(&self.repo.join(".cas"))
            .unwrap()
            .get(id)
            .unwrap();
        let req = TaskCloseRequest {
            id: id.into(),
            reason: Some("staging QA complete".into()),
            commit_receipt: task.deliverables.factory_branch_anchor,
            stranded_branch_override: None,
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
        };
        match self.core.cas_task_close(Parameters(req)).await {
            Ok(out) => extract_text(out),
            Err(err) => err.message.to_string(),
        }
    }
}

#[tokio::test]
async fn batch_qa_receipt_closes_listed_deliveries_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    let recorded = fx
        .supervisor
        .verification(Parameters(fx.request()))
        .await
        .expect("one supervisor receipt must cover the staging walk");
    assert!(extract_text(recorded).contains("Batch QA receipt"));
    let status: VerificationRequest = serde_json::from_value(serde_json::json!({
        "action":"qa_status", "task_id":"cas-bq01"
    }))
    .unwrap();
    let status = extract_text(
        fx.supervisor
            .verification(Parameters(status))
            .await
            .unwrap(),
    );
    assert!(
        status.contains("Batch QA PASS") && status.contains(&fx.artifact),
        "{status}"
    );
    for id in ["cas-bq01", "cas-bq02"] {
        let result = fx.close(id).await;
        assert_eq!(
            open_task_store(&fx.repo.join(".cas"))
                .unwrap()
                .get(id)
                .unwrap()
                .status,
            TaskStatus::Closed,
            "{id}: {result}"
        );
    }
    let unlisted = fx.close("cas-bq03").await;
    assert!(unlisted.contains("no `qa-bundle:` note"), "{unlisted}");
    assert_ne!(
        open_task_store(&fx.repo.join(".cas"))
            .unwrap()
            .get("cas-bq03")
            .unwrap()
            .status,
        TaskStatus::Closed
    );
    assert!(
        cas_store::list_qa_passes(&fx.repo.join(".cas"), "cas-bq01")
            .unwrap()
            .is_empty()
    );
}

impl Fx {
    fn unmerge(&self) {
        let tasks = open_task_store(&self.repo.join(".cas")).unwrap();
        let anchor = tasks
            .get("cas-bq01")
            .unwrap()
            .deliverables
            .factory_branch_anchor
            .unwrap();
        let seed = git(&self.repo, &["rev-parse", &format!("{anchor}^")]);
        git(&self.repo, &["branch", "-f", "main", &seed]);
    }
    fn reconfigure(&self, qa: &str) {
        std::fs::write(self.repo.join(".cas/config.toml"), format!(
            "[project]\ncanonical_id='batch-project'\n[verification]\nenabled=false\n[qa]\n{qa}\n")).unwrap();
    }
    async fn record(&self) -> String {
        extract_text(
            self.supervisor
                .verification(Parameters(self.request()))
                .await
                .unwrap(),
        )
    }
    async fn reject(&self, patch: serde_json::Value, reason: &str) {
        let mut request = serde_json::to_value(self.request()).unwrap();
        request
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let err = self
            .supervisor
            .verification(Parameters(serde_json::from_value(request).unwrap()))
            .await
            .expect_err("invalid batch receipt must be rejected");
        assert!(err.message.contains(reason), "{err:?}");
        assert!(
            cas_store::list_qa_batch_receipts(&self.repo.join(".cas"), "batch-project")
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn batch_qa_deferred_park_requires_receipt_after_integration_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let park = fx.close("cas-bq01").await;
    let root = fx.repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{park}"
    );
    assert!(cas_store::has_qa_batch_deferral(&root, "cas-bq01").unwrap());
    assert!(
        cas_store::list_qa_passes(&root, "cas-bq01")
            .unwrap()
            .is_empty()
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    // The debt survives a later change to the user-facing classification.
    let mut task = tasks.get("cas-bq01").unwrap();
    task.demo_statement.clear();
    tasks.update(&task).unwrap();
    fx.reconfigure("mode='batch'\nuser_facing_paths=[]");
    let refused = fx.close("cas-bq01").await;
    assert!(
        refused.contains("deferred evidence at its batch park"),
        "{refused}"
    );
    assert_ne!(tasks.get("cas-bq01").unwrap().status, TaskStatus::Closed);
    fx.record().await;
    let closed = fx.close("cas-bq01").await;
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{closed}"
    );
}

#[tokio::test]
async fn batch_qa_default_gate_is_unchanged_and_never_defers_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    fx.reconfigure("");
    let unset = fx.close("cas-bq01").await;
    assert!(unset.contains("no `qa-bundle:` note"), "{unset}");
    assert!(!cas_store::has_qa_batch_deferral(&fx.repo.join(".cas"), "cas-bq01").unwrap());
    fx.reconfigure("mode='per_task'");
    let explicit = fx.close("cas-bq01").await;
    // pin: opting out of batch deferral must preserve the exact legacy refusal.
    assert_eq!(unset, explicit);
    assert_eq!(
        open_task_store(&fx.repo.join(".cas"))
            .unwrap()
            .get("cas-bq01")
            .unwrap()
            .status,
        TaskStatus::InProgress
    );
    fx.reject(serde_json::json!({}), "requires qa.mode=batch")
        .await;
    let mut config = cas::config::Config::default();
    assert_eq!(config.get("qa.mode").as_deref(), Some("per_task"));
    config.set("qa.mode", "batch").unwrap();
    assert_eq!(config.get("qa.mode").as_deref(), Some("batch"));
    assert!(config.set("qa.mode", "typo").is_err());
    config.set("qa.mode", "per_task").unwrap();
    assert!(
        !serde_json::to_value(config.qa())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("mode")
    );
}

#[tokio::test]
async fn batch_qa_receipt_rejects_invalid_bindings_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.reject(serde_json::json!({"staging_sha":"main"}), "full 40-hex")
        .await;
    fx.reject(
        serde_json::json!({"staging_sha":"f".repeat(40)}),
        "does not resolve",
    )
    .await;
    fx.reject(serde_json::json!({"task_ids":[]}), "1 to 500")
        .await;
    fx.reject(
        serde_json::json!({"task_ids":["cas-missing"]}),
        "task cas-missing",
    )
    .await;
    fx.reject(
        serde_json::json!({"ledger_artifact_id":"art-missing"}),
        "not published in this project",
    )
    .await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get("cas-bq02").unwrap();
    let original = task.clone();
    task.origin_project = Some("another-project".into());
    tasks.update(&task).unwrap();
    fx.reject(serde_json::json!({}), "another project").await;
    tasks.update(&original).unwrap();
    let anchor = tasks
        .get("cas-bq01")
        .unwrap()
        .deliverables
        .factory_branch_anchor
        .unwrap();
    fx.reject(serde_json::json!({"staging_sha":anchor}), "not an ancestor")
        .await;
    fx.unmerge();
    fx.reject(serde_json::json!({}), "configured target main")
        .await;
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    std::fs::write(&fx.ledger, "changed after publish\n").unwrap();
    fx.reject(serde_json::json!({}), "ledger bytes differ")
        .await;
}

#[tokio::test]
async fn batch_qa_authority_and_no_self_review_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    // Environment supervisor mode never grants a Standard/Worker registry authority.
    let worker = CasService::new(fx.core.clone(), None);
    let err = worker
        .verification(Parameters(fx.request()))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("live registered supervisor"),
        "{err:?}"
    );
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    for token in ["batch-supervisor", "batch-supervisor-alias"] {
        let mut task = tasks.get("cas-bq01").unwrap();
        task.assignee = Some(token.into());
        tasks.update(&task).unwrap();
        if token.ends_with("alias") {
            let agents = open_agent_store(&fx.repo.join(".cas")).unwrap();
            let mut supervisor = agents.get("batch-supervisor").unwrap();
            supervisor.name = token.into();
            agents.register(&supervisor).unwrap();
        }
        fx.reject(serde_json::json!({}), "no self-review").await;
    }
}

#[tokio::test]
async fn batch_qa_changed_ledger_and_advanced_anchor_cannot_reuse_receipt_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.record().await;
    let original = std::fs::read(&fx.ledger).unwrap();
    std::fs::write(&fx.ledger, "tampered ledger\n").unwrap();
    assert!(fx.close("cas-bq01").await.contains("no `qa-bundle:` note"));
    std::fs::write(&fx.ledger, original).unwrap();
    std::fs::write(fx.repo.join("cas-bq01.css"), "body{gap:16px}\n").unwrap();
    git(&fx.repo, &["add", "cas-bq01.css"]);
    git(&fx.repo, &["commit", "-qm", "cas-bq01: later change"]);
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["branch", "-f", "main", &head]);
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get("cas-bq01").unwrap();
    task.deliverables.factory_branch_anchor = Some(head);
    tasks.update(&task).unwrap();
    let refused = fx.close("cas-bq01").await;
    assert!(refused.contains("no `qa-bundle:` note"), "{refused}");
    assert_ne!(tasks.get("cas-bq01").unwrap().status, TaskStatus::Closed);
    let mut receipts =
        cas_store::list_qa_batch_receipts(&fx.repo.join(".cas"), "batch-project").unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].task_heads.len(),
        2,
        "duplicate task IDs collapse"
    );
    receipts[0].staging_sha = "0".repeat(40);
    assert!(
        cas_store::record_qa_batch_receipt(&fx.repo.join(".cas"), &receipts[0]).is_err(),
        "receipt ID is immutable"
    );
    assert!(
        cas_store::list_qa_batch_receipts(&fx.repo.join(".cas"), "another-project")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn batch_qa_explicit_request_remains_available_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let park = fx.close("cas-bq03").await;
    assert_eq!(
        open_task_store(&fx.repo.join(".cas"))
            .unwrap()
            .get("cas-bq03")
            .unwrap()
            .status,
        TaskStatus::AwaitingMerge,
        "{park}"
    );
    let req: VerificationRequest = serde_json::from_value(serde_json::json!({
        "action":"qa_request", "task_id":"cas-bq03", "summary":"Explicitly walk this change before merge"
    })).unwrap();
    let requested = fx.supervisor.verification(Parameters(req)).await.unwrap();
    let passes = cas_store::list_qa_passes(&fx.repo.join(".cas"), "cas-bq03").unwrap();
    assert_eq!(passes.len(), 1, "{}", extract_text(requested));
    assert_eq!(passes[0].bound_head, fx.staging);
    assert!(
        cas::qa_pass::merge_gate(
            &open_task_store(&fx.repo.join(".cas"))
                .unwrap()
                .get("cas-bq03")
                .unwrap(),
            &cas::config::Config::load(&fx.repo.join(".cas"))
                .unwrap()
                .qa(),
            &passes,
            &fx.staging
        )
        .is_err()
    );
}

#[tokio::test]
async fn batch_qa_debt_survives_disabled_gate_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let park = fx.close("cas-bq01").await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{park}"
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    fx.reconfigure("mode='batch'\nevidence_gate=false");
    let refused = fx.close("cas-bq01").await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{refused}"
    );
    assert!(
        refused.contains("deferred evidence at its batch park"),
        "{refused}"
    );
}

#[tokio::test]
async fn batch_qa_receipt_preserves_skip_marker_gate_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let mut fx = fixture(&mut env);
    fx.unmerge();
    let test = fx.repo.join("hub-web/e2e/spacing.spec.ts");
    std::fs::create_dir_all(test.parent().unwrap()).unwrap();
    std::fs::write(&test, "test.only('spacing', async () => {});\n").unwrap();
    git(&fx.repo, &["add", "hub-web/e2e/spacing.spec.ts"]);
    git(&fx.repo, &["commit", "-qm", "cas-bq01: spacing test"]);
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get("cas-bq01").unwrap();
    task.deliverables.factory_branch_anchor = Some(git(&fx.repo, &["rev-parse", "HEAD"]));
    tasks.update(&task).unwrap();
    let before = fx.close("cas-bq01").await;
    assert!(before.contains("adds test skip/focus markers"), "{before}");
    assert!(!cas_store::has_qa_batch_deferral(&fx.repo.join(".cas"), "cas-bq01").unwrap());
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(
        &fx.repo,
        &[
            "merge",
            "--no-ff",
            "-qm",
            "staging batch",
            "factory/test-agent",
        ],
    );
    fx.staging = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);
    fx.record().await;
    let after = fx.close("cas-bq01").await;
    assert!(after.contains("adds test skip/focus markers"), "{after}");
    assert_ne!(tasks.get("cas-bq01").unwrap().status, TaskStatus::Closed);
}
