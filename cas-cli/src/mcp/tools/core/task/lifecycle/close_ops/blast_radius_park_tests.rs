//! cas-d6eb: a blast-radius delivery learns at park, while its worker can
//! still widen proof_targets, which delivered modules its declaration and
//! test receipts miss; and a supervisor widening that only names modules the
//! worker's receipts already cover keeps the parked delivery instead of
//! reopening it for a fresh verification round.
use super::*;
use crate::mcp::CasService;
use crate::store::{open_agent_store, open_store, open_task_store, open_worktree_store};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, TaskRisk, WorkTarget};
use std::path::{Path, PathBuf};

const PROJECT: &str = "cas-d6eb-fixture";
const WORKER: &str = "br-worker";
const LANE: &str = "factory/br-worker";
const ID: &str = "cas-br01";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "CAS Test")
        .env("GIT_AUTHOR_EMAIL", "cas@example.test")
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

async fn call(service: &CasService, request: serde_json::Value) -> String {
    let request = serde_json::from_value(request).unwrap();
    match service.task(Parameters(request)).await {
        Ok(result) => result
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(error) => error.message.to_string(),
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    _origin: tempfile::TempDir,
    worker_path: PathBuf,
    worker: CasService,
    supervisor: CasService,
}

impl Fixture {
    fn cas_dir(&self) -> PathBuf {
        self.dir.path().join(".cas")
    }

    fn task(&self) -> Task {
        open_task_store(&self.cas_dir()).unwrap().get(ID).unwrap()
    }
}

/// A workspace with crate `widget`; the worker's pushed, unmerged lane
/// changes `widget::gear`; its task declares only an unrelated target.
fn fixture(env: &mut TestEnvGuard) -> Fixture {
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
    std::fs::create_dir_all(repo.join("crates/widget/src")).unwrap();
    std::fs::write(repo.join("crates/widget/Cargo.toml"), "[package]\nname = \"widget\"\n").unwrap();
    std::fs::write(
        repo.join("crates/widget/src/lib.rs"),
        "pub mod gear;\npub const EMBEDDED: &str = include_str!(\"embedded.md\");\n",
    )
    .unwrap();
    std::fs::write(repo.join("crates/widget/src/embedded.md"), "embedded prompt v1\n").unwrap();
    std::fs::write(repo.join("crates/widget/src/gear.rs"), "pub fn turn() {}\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    git(repo, &["push", "-q", "origin", "main"]);
    git(repo, &["checkout", "-q", "-b", LANE]);
    std::fs::write(repo.join("crates/widget/src/gear.rs"), "pub fn turn() { let _ = 1; }\n").unwrap();
    git(repo, &["commit", "-q", "-am", &format!("fix({ID}): turn the gear")]);
    git(repo, &["push", "-q", "origin", LANE]);
    git(repo, &["checkout", "-q", "main"]);

    let cas_dir = repo.join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_FACTORY_MODE", "1");
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    std::fs::write(
        cas_dir.join("config.toml"),
        format!("[project]\ncanonical_id=\"{PROJECT}\"\n[verification]\nenabled=false\n"),
    )
    .unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_worktree_store(&cas_dir).unwrap().init().unwrap();
    open_task_store(&cas_dir).unwrap().init().unwrap();
    let worker_path = cas_dir.join("worktrees").join(WORKER);
    std::fs::create_dir_all(worker_path.parent().unwrap()).unwrap();
    git(repo, &["worktree", "add", "-q", worker_path.to_str().unwrap(), LANE]);
    std::fs::create_dir_all(worker_path.join(".cas")).unwrap();
    std::fs::write(worker_path.join(".cas/config.toml"), format!("[project]\ncanonical_id=\"{PROJECT}\"\n")).unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    agents.register(&Agent::new_with_role("br-worker-session".into(), WORKER.into(), AgentRole::Worker)).unwrap();
    agents.register(&Agent::new_with_role("br-supervisor-session".into(), "supervisor".into(), AgentRole::Supervisor)).unwrap();
    let worker = CasCore::with_daemon(cas_dir.clone(), None, None);
    worker.set_agent_id_for_testing("br-worker-session".into());
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("br-supervisor-session".into());

    let mut task = Task::new(ID.into(), "turn the gear".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(WORKER.into());
    task.risk = vec![TaskRisk::BlastRadius];
    task.proof_targets = vec!["unrelated".into()];
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: format!("project:{PROJECT}"),
        target_branch: "main".into(),
    });
    open_task_store(&cas_dir).unwrap().add(&task).unwrap();
    agents.try_claim(ID, "br-worker-session", 1800, Some("assigned")).unwrap();
    Fixture {
        dir,
        _origin: origin,
        worker_path,
        worker: CasService::new(worker, None),
        supervisor: CasService::new(supervisor, None),
    }
}

/// cas-eac2 / cas-95e0: a focused receipt is required after each proof-input
/// change, but can be rebound across a docs-only commit without rerunning.
#[tokio::test]
async fn park_requires_a_focused_receipt_in_the_changed_crate_cas_eac2() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let close = serde_json::json!({"action": "close", "id": ID, "reason": "gear turns"});

    let refused = call(&f.worker, close.clone()).await;
    assert!(refused.contains("FOCUSED PROOF REQUIRED"), "{refused}");
    assert!(refused.contains("receipt at"), "{refused}");
    assert!(refused.contains(": widget."), "{refused}");
    assert_eq!(f.task().status, TaskStatus::InProgress, "the proof is still the worker's to run");

    call(
        &f.worker,
        serde_json::json!({"action": "update", "id": ID, "proof_targets": "unrelated,gear"}),
    )
    .await;
    let widened = call(&f.worker, close.clone()).await;
    assert!(widened.contains("FOCUSED PROOF REQUIRED"), "{widened}");

    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );
    let source_path = "crates/widget/src/gear.rs";
    std::fs::write(f.worker_path.join(source_path), "pub fn turn() { let _ = 2; }\n").unwrap();
    git(&f.worker_path, &["add", source_path]);
    git(&f.worker_path, &["commit", "-q", "-m", "fix(cas-br01): update gear behavior"]);
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale = call(&f.worker, close.clone()).await;
    assert!(
        stale.contains("FOCUSED PROOF REQUIRED"),
        "receipt at {tested_tip} must not cover Rust source change: {stale}"
    );
    assert_eq!(f.task().status, TaskStatus::InProgress, "{stale}");

    std::fs::write(
        f.worker_path.join(source_path),
        "pub fn turn() { let _ = 1; }\n",
    )
    .unwrap();
    git(&f.worker_path, &["add", source_path]);
    git(
        &f.worker_path,
        &["commit", "-q", "-m", "revert(cas-br01): restore gear source"],
    );
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale_after_revert = call(&f.worker, close.clone()).await;
    assert!(
        stale_after_revert.contains("FOCUSED PROOF REQUIRED"),
        "a proof input changed and was reverted after {tested_tip}: {stale_after_revert}"
    );
    assert_eq!(
        f.task().status,
        TaskStatus::InProgress,
        "{stale_after_revert}"
    );
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );

    let test_path = "crates/widget/tests/gear.rs";
    std::fs::create_dir_all(f.worker_path.join("crates/widget/tests")).unwrap();
    std::fs::write(f.worker_path.join(test_path), "#[test] fn gear_turns() {}\n").unwrap();
    git(&f.worker_path, &["add", test_path]);
    git(
        &f.worker_path,
        &["commit", "-q", "-m", "test(cas-br01): add gear coverage"],
    );
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale = call(&f.worker, close.clone()).await;
    assert!(
        stale.contains("FOCUSED PROOF REQUIRED"),
        "receipt at {tested_tip} must not cover test change: {stale}"
    );
    assert_eq!(f.task().status, TaskStatus::InProgress, "{stale}");
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );

    let manifest_path = "crates/widget/Cargo.toml";
    let manifest = std::fs::read_to_string(f.worker_path.join(manifest_path)).unwrap();
    std::fs::write(
        f.worker_path.join(manifest_path),
        format!("{manifest}\n# build input changed\n"),
    )
    .unwrap();
    git(&f.worker_path, &["add", manifest_path]);
    git(
        &f.worker_path,
        &["commit", "-q", "-m", "build(cas-br01): update crate manifest"],
    );
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale = call(&f.worker, close.clone()).await;
    assert!(
        stale.contains("FOCUSED PROOF REQUIRED"),
        "receipt at {tested_tip} must not cover Cargo change: {stale}"
    );
    assert_eq!(f.task().status, TaskStatus::InProgress, "{stale}");
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );

    let embedded_path = "crates/widget/src/embedded.md";
    std::fs::write(f.worker_path.join(embedded_path), "embedded prompt v2\n").unwrap();
    git(&f.worker_path, &["add", embedded_path]);
    git(
        &f.worker_path,
        &["commit", "-q", "-m", "docs(cas-br01): change compiled-in prompt"],
    );
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale_embedded = call(&f.worker, close.clone()).await;
    assert!(
        stale_embedded.contains("FOCUSED PROOF REQUIRED"),
        "receipt at {tested_tip} must not cover a compiled-in non-Rust file: {stale_embedded}"
    );
    assert_eq!(f.task().status, TaskStatus::InProgress, "{stale_embedded}");
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );

    let doc_path = "docs/design/cli/update-managed-files.brief.md";
    std::fs::create_dir_all(f.worker_path.join("docs/design/cli")).unwrap();
    std::fs::write(f.worker_path.join(doc_path), "Updated operator guidance.\n").unwrap();
    git(&f.worker_path, &["add", doc_path]);
    git(&f.worker_path, &["commit", "-q", "-m", "docs(cas-br01): update managed files brief"]);
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let delivered_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    assert_ne!(tested_tip, delivered_tip);
    let parked = call(&f.worker, close.clone()).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(f.task().status, TaskStatus::AwaitingMerge);

    git(&f.dir.path(), &["merge", "--ff-only", LANE]);
    git(&f.dir.path(), &["push", "-q", "origin", "main"]);
    let closed = call(&f.supervisor, close).await;
    assert!(closed.contains("Closed task"), "{closed}");
    let task = f.task();
    assert_eq!(task.status, TaskStatus::Closed);
    assert!(
        task.notes
            .contains(&format!("rebound from {tested_tip} to {delivered_tip}")),
        "the close must record the receipt rebind: {}",
        task.notes
    );
}

/// AC2: a worker receipt in `widget` at the parked tip already covers
/// `widget::gear`; widening the declaration to name it changes nothing the
/// close accepts, so the parked delivery is kept. A widening the receipts do
/// not cover still takes the invalidating scope-fix path.
#[tokio::test]
async fn widening_covered_by_worker_receipts_keeps_the_parked_delivery_cas_d6eb() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(), &f.worker_path, &tip, "widget", "test(gear)",
    );
    // The receipt covers widget, so the delivery parks without a widening.
    let parked = call(&f.worker, serde_json::json!({"action": "close", "id": ID, "reason": "gear turns"})).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    let before = f.task();
    assert_eq!(before.status, TaskStatus::AwaitingMerge);

    let kept = call(
        &f.supervisor,
        serde_json::json!({
            "action": "update", "id": ID, "proof_scope_fix": true,
            "proof_targets": "unrelated,widget::gear", "reason": "name the delivered module",
        }),
    )
    .await;
    let after = f.task();
    assert!(kept.contains("verification and awaiting_merge status are kept"), "{kept}");
    assert_eq!(after.status, TaskStatus::AwaitingMerge);
    assert_eq!(after.proof_targets, ["unrelated", "widget::gear"]);
    assert_eq!(after.deliverables.factory_branch_anchor, before.deliverables.factory_branch_anchor);
    assert!(after.notes.contains("reviewed proof cycle is kept"), "{}", after.notes);

    let reset = call(
        &f.supervisor,
        serde_json::json!({
            "action": "update", "id": ID, "proof_scope_fix": true,
            "proof_targets": "unrelated,widget::gear,sprocket::chain", "reason": "untested crate",
        }),
    )
    .await;
    // It reaches the ordinary invalidating correction, which refuses an
    // unmerged delivery, instead of being kept.
    assert!(!reset.contains("are kept"), "an untested crate never keeps the cycle: {reset}");
    assert!(reset.contains("PROOF-SCOPE FIX REJECTED"), "{reset}");
    assert_eq!(f.task().proof_targets, ["unrelated", "widget::gear"]);
}
