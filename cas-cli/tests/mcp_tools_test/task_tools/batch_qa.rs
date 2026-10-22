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

#[test]
fn override_debt_documentation_cas_5129a() {
    let guide = include_str!("../../../../docs/qa/batch-qa.md");
    assert!(guide.contains("supervisor_override=true does not clear batch QA debt"));
    assert!(!guide.contains(
        "Normal supervisor override and explicit QA-request authority remain available."
    ));
}

async fn squash_deferred_delivery_cas_5129a(receipt: bool) {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let root = fx.repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    if receipt {
        // The pre-squash integrated staging remains on origin/main, while
        // local main is subsequently squash-merged. Both are authoritative.
        git(
            &fx.repo,
            &["update-ref", "refs/remotes/origin/main", &fx.staging],
        );
        fx.record().await;
    } else {
        let request = serde_json::from_value(serde_json::json!({
            "action":"qa_request", "task_id":"cas-bq01", "summary":"Review before squash"
        }))
        .unwrap();
        fx.supervisor
            .verification(Parameters(request))
            .await
            .unwrap();
        cas_store::claim_qa_pass(
            &root,
            "cas-bq01",
            "independent-reviewer",
            chrono::Utc::now(),
        )
        .unwrap();
        cas_store::resolve_qa_pass(
            &root,
            "cas-bq01",
            "independent-reviewer",
            cas::types::QaVerdict::Approved,
            "Reviewed delivery before squash",
            None,
            fx.ledger.to_str().unwrap(),
            chrono::Utc::now(),
        )
        .unwrap();
    }
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["merge", "--squash", &fx.staging]);
    git(
        &fx.repo,
        &["commit", "-qm", "cas-bq01: squash integration batch"],
    );
    let squash = git(&fx.repo, &["rev-parse", "HEAD"]);
    let anchor = tasks
        .get("cas-bq01")
        .unwrap()
        .deliverables
        .factory_branch_anchor
        .unwrap();
    assert_ne!(squash, anchor);
    let ancestry = Command::new("git")
        .args(["merge-base", "--is-ancestor", &anchor, &squash])
        .current_dir(&fx.repo)
        .status()
        .unwrap();
    assert_eq!(
        ancestry.code(),
        Some(1),
        "squash must break commit ancestry"
    );
    git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);
    let request = serde_json::from_value::<TaskCloseRequest>(serde_json::json!({
        "id":"cas-bq01", "commit_receipt":squash, "reason":"Reviewed squash delivery"
    }))
    .unwrap();
    let closed = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{closed}"
    );
    assert!(
        tasks
            .get("cas-bq01")
            .unwrap()
            .notes
            .contains("QA evidence scoped to delivery anchor")
    );
    // A real but unrelated target commit must never be mapped to the anchor.
    let request = serde_json::from_value::<TaskCloseRequest>(serde_json::json!({
        "id":"cas-bq02", "commit_receipt":git(&fx.repo, &["rev-parse", "main^1"]),
        "reason":"Unrelated target receipt must stay refused"
    }))
    .unwrap();
    let unrelated = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
    assert_ne!(
        tasks.get("cas-bq02").unwrap().status,
        TaskStatus::Closed,
        "{unrelated}"
    );
    assert!(
        unrelated.contains("QA evidence ledger is missing: no ledger at"),
        "unrelated receipt must not inherit the reviewed task's QA: {unrelated}"
    );
}

#[tokio::test]
async fn squash_close_with_passed_qa_cas_5129a() {
    squash_deferred_delivery_cas_5129a(false).await;
}

#[tokio::test]
async fn squash_close_with_batch_receipt_cas_5129a() {
    squash_deferred_delivery_cas_5129a(true).await;
}

#[tokio::test]
async fn unreviewed_squash_cannot_clear_debt_cas_5129a() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let root = fx.repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["merge", "--squash", &fx.staging]);
    git(
        &fx.repo,
        &[
            "commit",
            "-qm",
            "unrelated squash containing the delivery's files",
        ],
    );
    let squash = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);
    let request = serde_json::from_value::<TaskCloseRequest>(serde_json::json!({
        "id":"cas-bq01", "commit_receipt":squash, "reason":"Unreviewed squash"
    }))
    .unwrap();
    let refused = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
    assert!(
        refused.contains("deferred evidence at its batch park"),
        "{refused}"
    );
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge
    );
    assert!(
        cas_store::list_qa_passes(&root, "cas-bq01")
            .unwrap()
            .is_empty()
    );
    assert!(
        cas_store::list_qa_batch_receipts(&root, "batch-project")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cas_d697_round2_historical_anchor_actor_cannot_record() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    let old = git(&fx.repo, &["rev-parse", "HEAD"]);
    let output = Command::new("git")
        .args(["commit", "--allow-empty", "-qm", "prior implementation"])
        .current_dir(&fx.repo)
        .env("GIT_AUTHOR_NAME", "batch-supervisor")
        .env("GIT_AUTHOR_EMAIL", "prior@example.test")
        .env("GIT_COMMITTER_NAME", "batch-supervisor")
        .env("GIT_COMMITTER_EMAIL", "prior@example.test")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let historical = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["reset", "--hard", &old]);
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get("cas-bq01").unwrap();
    task.deliverables
        .historical_factory_branch_anchors
        .push(historical);
    tasks.update(&task).unwrap();
    fx.reject(serde_json::json!({}), "no self-review").await;
}

#[tokio::test]
async fn cas_d697_round2_reassigned_implementer_cannot_record() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    let root = fx.repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let mut task = tasks.get("cas-bq01").unwrap();
    task.assignee = Some("batch-supervisor".into());
    tasks.update(&task).unwrap();
    agents
        .try_claim(
            &task.id,
            "batch-supervisor",
            600,
            Some("original implementation"),
        )
        .unwrap();
    agents.release_lease(&task.id, "batch-supervisor").unwrap();
    task.assignee = Some("test-agent".into());
    tasks.update(&task).unwrap();
    fx.reject(serde_json::json!({}), "no self-review").await;
}

#[tokio::test]
async fn cas_d697_round2_prior_delivery_actor_cannot_record() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    let root = fx.repo.join(".cas");
    // An older receipt must remain binding after another worker re-delivers.
    for worker in ["batch-supervisor", "test-agent"] {
        let input = cas::types::WorkerCompletionReceiptInput {
            task_id: "cas-bq01".into(),
            worker_agent_id: worker.into(),
            repo_selector: "project".into(),
            source_branch: format!("factory/{worker}-cas-bq01"),
            commit_sha: fx.staging.clone(),
            merge_base_sha: fx.staging.clone(),
            target_branch: "main".into(),
            target_sha: fx.staging.clone(),
            proof_reference: "target/test.log".into(),
            scope_summary: "Delivery".into(),
            artifact_path: None,
        };
        let receipt =
            cas_store::build_worker_completion_receipt(&input, worker, chrono::Utc::now());
        cas_store::create_worker_delivery(
            &root,
            &receipt,
            cas::types::WorkerDeliveryState::AwaitingMerge,
            worker,
        )
        .unwrap();
    }
    fx.reject(serde_json::json!({}), "no self-review").await;
}

#[tokio::test]
async fn cas_d697_round2_handoff_owner_cannot_record() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get("cas-bq01").unwrap();
    task.deliverables
        .handoff_branches
        .push("origin/factory/batch-supervisor-cas-bq01".into());
    tasks.update(&task).unwrap();
    fx.reject(serde_json::json!({}), "no self-review").await;
}

#[tokio::test]
async fn cas_d697_round2_stale_origin_cannot_defer_integrated_delivery() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let seed = git(&fx.repo, &["rev-parse", "main"]);
    git(&fx.repo, &["update-ref", "refs/remotes/origin/main", &seed]);
    let parked = fx.close("cas-bq01").await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    let refused = fx.close("cas-bq01").await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{refused}"
    );
    assert!(refused.contains("deferred evidence"), "{refused}");
    let direct = fx.close("cas-bq03").await;
    assert_ne!(
        tasks.get("cas-bq03").unwrap().status,
        TaskStatus::Closed,
        "{direct}"
    );
    assert!(!cas_store::has_qa_batch_deferral(&fx.repo.join(".cas"), "cas-bq03").unwrap());
    fx.record().await;
    let closed = fx.close("cas-bq01").await;
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{closed}"
    );
}

#[tokio::test]
async fn cas_d697_round2_final_close_backstop_covers_unassigned_path() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    let mut task = tasks.get("cas-bq01").unwrap();
    task.assignee = None;
    tasks.update(&task).unwrap();
    fx.reconfigure("mode='per_task'\nevidence_gate=false\nindependent_pass=false");
    let refused = fx.close("cas-bq01").await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{refused}"
    );
    assert!(refused.contains("deferred evidence"), "{refused}");
    let request = serde_json::from_value(serde_json::json!({
        "action":"close", "id":"cas-bq01", "supervisor_override":true,
        "reason":"authorized close override cannot erase deferred evidence"
    }))
    .unwrap();
    let overridden = extract_text(fx.supervisor.task(Parameters(request)).await.unwrap());
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{overridden}"
    );
    assert!(overridden.contains("deferred evidence"), "{overridden}");
}

#[tokio::test]
async fn cas_d697_round2_mode_switch_retains_batch_debt() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    fx.reconfigure("mode='per_task'\nevidence_gate=false\nindependent_pass=false");
    let refused = fx.close("cas-bq01").await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{refused}"
    );
    assert!(refused.contains("deferred evidence"), "{refused}");
    fx.reconfigure("mode='batch'");
    fx.record().await;
    fx.reconfigure("mode='per_task'\nevidence_gate=false\nindependent_pass=false");
    let closed = fx.close("cas-bq01").await;
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{closed}"
    );
}

#[tokio::test]
async fn cas_d697_round2_passed_explicit_qa_satisfies_deferred_close() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let root = fx.repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    let request = serde_json::from_value(serde_json::json!({
        "action":"qa_request", "task_id":"cas-bq01", "summary":"Walk this delivery explicitly"
    }))
    .unwrap();
    fx.supervisor
        .verification(Parameters(request))
        .await
        .unwrap();
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    let pending = fx.close("cas-bq01").await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{pending}"
    );
    // Seed the trusted persisted reviewer verdict at the store seam. Existing
    // qa_record tests separately cover its full evidence-bundle validation.
    cas_store::claim_qa_pass(
        &root,
        "cas-bq01",
        "independent-reviewer",
        chrono::Utc::now(),
    )
    .unwrap();
    cas_store::resolve_qa_pass(
        &root,
        "cas-bq01",
        "independent-reviewer",
        cas::types::QaVerdict::Approved,
        "Explicit delivery walk passed",
        None,
        fx.ledger.to_str().unwrap(),
        chrono::Utc::now(),
    )
    .unwrap();
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    let closed = fx.close("cas-bq01").await;
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{closed}"
    );
    assert!(
        cas_store::list_qa_batch_receipts(&root, "batch-project")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cas_d697_round2_direct_update_cannot_drop_debt() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    let parked = fx.close("cas-bq01").await;
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::AwaitingMerge,
        "{parked}"
    );
    git(&fx.repo, &["branch", "-f", "main", &fx.staging]);
    let request = || {
        serde_json::from_value(serde_json::json!({
            "action":"update", "id":"cas-bq01", "status":"closed"
        }))
        .unwrap()
    };
    let result = fx.supervisor.task(Parameters(request())).await;
    assert_ne!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{result:?}"
    );
    fx.record().await;
    let result = fx.supervisor.task(Parameters(request())).await;
    assert_eq!(
        tasks.get("cas-bq01").unwrap().status,
        TaskStatus::Closed,
        "{result:?}"
    );
}

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
            no_delivery: None,
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
    // cas-e04d: the release-gate pack ledger is this ledger. A receipt binds
    // the sealed pack.json for staging_sha; changed pack bytes void it too.
    let cas_root = fx.repo.join(".cas");
    let dir = cas::qa_release::pack::pack_dir(&cas_root, &fx.staging).unwrap();
    let mut pack = cas::qa_release::pack::Pack::new("batch-project", &fx.staging, Vec::new());
    let pack_json = pack.seal(&dir).unwrap();
    let sealed = std::fs::read(&pack_json).unwrap();
    std::fs::write(&pack_json, b"{}").unwrap();
    fx.reject(serde_json::json!({}), "release-gate pack for staging_sha").await;
    std::fs::write(&pack_json, &sealed).unwrap();
    let recorded = fx.record().await;
    assert!(recorded.contains("pack.json (OWED)"), "{recorded}");
    std::fs::write(&pack_json, b"{}").unwrap();
    assert!(fx.close("cas-bq01").await.contains("no `qa-bundle:` note"));
    std::fs::write(&pack_json, sealed).unwrap();
    let bound = fx.close("cas-bq02").await;
    let tasks = open_task_store(&cas_root).unwrap();
    assert_eq!(tasks.get("cas-bq02").unwrap().status, TaskStatus::Closed, "{bound}");
    let original = std::fs::read(&fx.ledger).unwrap();
    std::fs::write(&fx.ledger, "tampered ledger\n").unwrap();
    assert!(fx.close("cas-bq01").await.contains("no `qa-bundle:` note"));
    std::fs::write(&fx.ledger, original).unwrap();
    std::fs::write(fx.repo.join("cas-bq01.css"), "body{gap:16px}\n").unwrap();
    git(&fx.repo, &["add", "cas-bq01.css"]);
    git(&fx.repo, &["commit", "-qm", "cas-bq01: later change"]);
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["branch", "-f", "main", &head]);
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
    let bound = receipts[0].pack.as_ref().expect("receipt binds the pack");
    assert_eq!((bound.gate.as_str(), bound.path.as_str()), ("OWED", pack_json.to_str().unwrap()));
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

#[tokio::test]
async fn batch_qa_merge_receipt_binds_own_reviewed_anchor_cas_d697() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(&mut env);
    fx.unmerge();
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(
        &fx.repo,
        &[
            "merge",
            "--no-ff",
            "-qm",
            "integrate reviewed staging",
            "factory/test-agent",
        ],
    );
    let merged = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);
    fx.record().await;
    let req = TaskCloseRequest {
        no_delivery: None,
        id: "cas-bq01".into(),
        reason: Some("integrated reviewed batch".into()),
        commit_receipt: Some(merged),
        stranded_branch_override: None,
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
    };
    let closed = extract_text(fx.core.cas_task_close(Parameters(req)).await.unwrap());
    assert_eq!(
        open_task_store(&fx.repo.join(".cas"))
            .unwrap()
            .get("cas-bq01")
            .unwrap()
            .status,
        TaskStatus::Closed,
        "{closed}"
    );
    // A sibling's earlier anchor cannot stand in for the later task's delivery.
    let sibling = open_task_store(&fx.repo.join(".cas"))
        .unwrap()
        .get("cas-bq01")
        .unwrap()
        .deliverables
        .factory_branch_anchor;
    let req = TaskCloseRequest {
        no_delivery: None,
        id: "cas-bq02".into(),
        reason: Some("wrong sibling receipt".into()),
        commit_receipt: sibling,
        stranded_branch_override: None,
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
    };
    let refused = extract_text(fx.core.cas_task_close(Parameters(req)).await.unwrap());
    assert_ne!(
        open_task_store(&fx.repo.join(".cas"))
            .unwrap()
            .get("cas-bq02")
            .unwrap()
            .status,
        TaskStatus::Closed,
        "{refused}"
    );
}
