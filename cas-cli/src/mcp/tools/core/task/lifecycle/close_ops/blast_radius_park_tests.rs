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
        "pub mod gear;\npub const EMBEDDED: &str = include_str!(\"embedded.md\");\npub const RELEASE_RUBRIC: &str = include_str!(\"../../../docs/RELEASE_SLACK_RUBRIC.md\");\n",
    )
    .unwrap();
    std::fs::write(repo.join("crates/widget/src/embedded.md"), "embedded prompt v1\n").unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/RELEASE_SLACK_RUBRIC.md"), "release rubric v1\n").unwrap();
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

/// Declaration-only risk validation resolves real mounts without relying on
/// receipt-derived scope or accepting lookalike filenames.
#[test]
fn declared_parent_covers_only_real_sibling_test_mounts_cas_ba56() {
    let repo = tempfile::tempdir().unwrap();
    let src = repo.path().join("cas-cli/src/nested");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("gear_tests.rs"), "#[test] fn turns() {}\n").unwrap();
    let changed = vec![
        "cas-cli/src/nested/gear.rs".into(),
        "cas-cli/src/nested/gear_tests.rs".into(),
    ];
    let mut task = Task::new("cas-ba56-declaration".into(), "parent proof".into());
    task.risk = vec![TaskRisk::BlastRadius];
    for target in [
        "cas-cli nested::gear",
        "gear",
        "cas nested::gear::tests",
        "cas-cli/src/nested/gear.rs",
    ] {
        task.proof_targets = vec![target.into()];
        for (body, covered) in [
            (
                "#[cfg(test)]\n#[path = \"gear_tests.rs\"]\nmod tests;\n",
                true,
            ),
            ("#[path=\"gear_tests.rs\"] pub(crate) mod tests;\n", true),
            ("#[path = \"gear_tests.rs\"] mod gear_tests;\n", true),
            ("// #[path = \"gear_tests.rs\"] mod tests;\n", false),
            (
                "const EXAMPLE: &str = r#\"#[path = \"gear_tests.rs\"] mod tests;\"#;\n",
                false,
            ),
            ("#[path = \"other_tests.rs\"] mod tests;\n", false),
            ("mod tests {}\n", false),
            ("mod gear_tests;\n", false),
            ("#[path = \"gear_tests.rs\"] mod", false),
        ] {
            std::fs::write(src.join("gear.rs"), body).unwrap();
            // Exercise the live declaration gate without receipt widening or build
            // evidence; factory workers defer those build proofs to assembly.
            let result = validate_risk_close_proofs_with_base_and_target_and_cache(
                &task,
                &changed,
                repo.path(),
                repo.path(),
                None,
                None,
                BuildProofs::DeferredToAssembly,
                &mut ScopedProofTargetCache::default(),
            );
            assert_eq!(result.is_ok(), covered, "{target}, {body:?}: {result:?}");
            if let Err(error) = result {
                assert!(error.contains("nested::gear_tests"), "{error}");
            }
        }
    }
    // A mod.rs parent can mount the same sibling, even when only the tests
    // changed. No changed-file heuristic may stand in for the declaration.
    std::fs::remove_file(src.join("gear.rs")).unwrap();
    std::fs::create_dir(src.join("gear")).unwrap();
    std::fs::write(
        src.join("gear/mod.rs"),
        "#[path = \"../gear_tests.rs\"] mod tests;\n",
    )
    .unwrap();
    task.proof_targets = vec!["cas-cli nested::gear".into()];
    validate_risk_close_proofs_with_base_and_target_and_cache(
        &task,
        &changed[1..],
        repo.path(),
        repo.path(),
        None,
        None,
        BuildProofs::DeferredToAssembly,
        &mut ScopedProofTargetCache::default(),
    )
    .expect("mounted tests remain covered without a changed parent");

    // cas-087d: a single close may change several mounted siblings whose
    // filenames describe their scope rather than repeat their parent's name.
    let named = repo.path().join("cas-cli/src/named");
    std::fs::create_dir(&named).unwrap();
    let mut changed = Vec::new();
    task.proof_targets.clear();
    for (parent, tests, target) in [
        ("peers", "peers_scope_tests", "named::peers"),
        ("peer_mailbox", "peer_mailbox_user_scope_tests", "named::peer_mailbox"),
        // cas-623f: match a tested child through the actual adjacent parent,
        // not just the physical directory's already-covered ancestor path.
        ("close_ops", "close_ops/blast_radius_park_tests", "named::close_ops::tests"),
    ] {
        let test_file = named.join(format!("{tests}.rs"));
        std::fs::create_dir_all(test_file.parent().unwrap()).unwrap();
        std::fs::write(test_file, "#[test] fn scoped() {}\n").unwrap();
        std::fs::write(
            named.join(format!("{parent}.rs")),
            format!("#[cfg(test)]\n#[path = \"{tests}.rs\"] mod tests;\n"),
        ).unwrap();
        changed.push(format!("cas-cli/src/named/{tests}.rs"));
        task.proof_targets.push(format!("cas-cli {target}"));
    }
    validate_risk_close_proofs_with_base_and_target_and_cache(
        &task, &changed, repo.path(), repo.path(), None, None,
        BuildProofs::DeferredToAssembly, &mut ScopedProofTargetCache::default(),
    ).expect("actual path mounts cover differently named sibling test files");

    std::fs::write(
        named.join("peers.rs"),
        "// #[path = \"peers_scope_tests.rs\"] mod tests;\n",
    ).unwrap();
    assert_eq!(
        uncovered_blast_radius_modules(repo.path(), &changed, &task.proof_targets),
        ["named::peers_scope_tests"],
        "a comment cannot claim a differently named sibling",
    );

    // Two real mounting parents must not silently grant either parent's alias.
    std::fs::write(
        named.join("close_ops/alternate.rs"),
        "#[path = \"blast_radius_park_tests.rs\"] mod tests;\n",
    ).unwrap();
    task.proof_targets.push("cas-cli named::close_ops::alternate::tests".into());
    let error = validate_risk_close_proofs_with_base_and_target_and_cache(
        &task, &changed[2..], repo.path(), repo.path(), None, None,
        BuildProofs::DeferredToAssembly, &mut ScopedProofTargetCache::default(),
    ).expect_err("competing mounts require explicit physical proof scope");
    assert!(error.contains("named/close_ops.rs") && error.contains("named/close_ops/alternate.rs"),
        "uncovered diagnostic must name both mounting parents: {error}");
    task.proof_targets.push("cas-cli named::close_ops::blast_radius_park_tests".into());
    validate_risk_close_proofs_with_base_and_target_and_cache(
        &task, &changed[2..], repo.path(), repo.path(), None, None,
        BuildProofs::DeferredToAssembly, &mut ScopedProofTargetCache::default(),
    ).expect("an explicit physical module target resolves ambiguous parent aliases");
}

/// A parent's capped tests cover a mounted sibling and exercised child through
/// real worker park, merge and supervisor close; the audit names physical files.
#[tokio::test]
async fn parent_receipt_covers_mounted_tests_and_child_cas_ba56() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    std::fs::create_dir_all(f.worker_path.join("crates/widget/src/gear")).unwrap();
    for (path, body) in [
        ("gear.rs", "pub fn turn() {}\nmod blocked;\n#[cfg(test)]\n#[path = \"gear_tests.rs\"]\nmod tests;\n"),
        ("gear_tests.rs", "#[test] fn turns() { super::turn(); super::blocked::blocked(); }\n"),
        ("gear/blocked.rs", "pub fn blocked() {}\n"),
    ] {
        std::fs::write(f.worker_path.join("crates/widget/src").join(path), body).unwrap();
    }
    git(&f.worker_path, &["add", "."]);
    git(&f.worker_path, &["commit", "-q", "-m", "test(cas-br01): parent selection covers mounted tests and child"]);
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let updated = call(
        &f.worker,
        serde_json::json!({
            "action": "update", "id": ID, "proof_targets": ["unrelated"],
        }),
    )
    .await;
    assert_eq!(f.task().proof_targets, ["unrelated"], "{updated}");
    let head = git(&f.worker_path, &["rev-parse", "HEAD"]);
    // A receipt in another crate must not make the mounted file disappear
    // from the focused-proof obligation.
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &head,
        "other-crate",
        "test(gear::tests)",
    );
    let refused = call(&f.worker, serde_json::json!({
        "action": "close", "id": ID, "reason": "parent tests cover delivery",
    })).await;
    assert!(refused.contains("FOCUSED PROOF REQUIRED"), "{refused}");
    assert_eq!(f.task().status, TaskStatus::InProgress);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &head,
        "widget",
        "test(gear::tests)",
    );
    let base = git(&f.worker_path, &["rev-parse", "origin/main"]);
    crate::factory_worker_check::record_stability_for_tests(
        &f.cas_dir(), &f.worker_path, &head, &base, "widget", "test(gear::tests)",
        "crates/widget/src/gear_tests.rs::turns",
    );
    let parked = call(&f.worker, serde_json::json!({
        "action": "close", "id": ID, "reason": "parent tests cover delivery",
    })).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(f.task().status, TaskStatus::AwaitingMerge);
    git(f.dir.path(), &["merge", "--no-ff", "-m", "merge delivery", LANE]);
    git(f.dir.path(), &["push", "-q", "origin", "main"]);
    let closed = call(&f.supervisor, serde_json::json!({
        "action": "close", "id": ID, "reason": "reviewed parent test proof",
    })).await;
    assert!(closed.contains("Closed task"), "{closed}");
    assert_eq!(f.task().status, TaskStatus::Closed);
    let note = f.task().notes;
    assert!(note.lines().any(|line| line.contains("PROOF SCOPE derived") && line.contains("widget::gear_tests")), "{note}");
    assert_eq!(
        f.task().proof_targets,
        ["unrelated"],
        "no administrative scope widening"
    );
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
    // cas-0dac: a supervisor ran this selection in a scratch checkout while
    // the worker's build lane was unavailable. Record it through the public
    // factory action, then let the real park gate consume the normal receipt.
    let scratch = tempfile::tempdir().unwrap();
    let tested_repo = scratch.path().join("tested");
    git(f.dir.path(), &["worktree", "add", "-q", "--detach", tested_repo.to_str().unwrap(), &tested_tip]);
    let log = scratch.path().join("supervisor-test.log");
    std::fs::write(&log, "     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n").unwrap();
    let proof = serde_json::json!({
        "head": tested_tip, "repo": tested_repo,
        "cargo_args": ["nextest", "run", "-p", "widget", "--lib", "-E", "test(gear)"],
        "log_path": log, "exit_code": 0,
    });
    let request = |proof: &serde_json::Value| serde_json::from_value(serde_json::json!({
        "action": "record_test_receipt", "task_id": ID, "payload": proof.to_string(),
    })).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    assert!(f.worker.factory(Parameters(request(&proof))).await.is_err(), "a worker cannot attest supervisor evidence via an environment hint");
    for (field, value) in [
        ("head", serde_json::json!(git(&f.worker_path, &["rev-parse", "HEAD^"]))),
        ("exit_code", serde_json::json!(100)),
        ("cargo_args", serde_json::json!(["nextest", "run", "-p", "widget", "--lib", "-E", "all()"])),
    ] {
        let mut invalid = proof.clone();
        invalid[field] = value;
        assert!(f.supervisor.factory(Parameters(request(&invalid))).await.is_err(), "invalid {field} must not supply a test receipt");
    }
    for failed_log in [
        "     Summary [ 0.01s] 0 tests run: 0 passed, 1 skipped\n",
        "     Summary [ 0.01s] 2 tests run: 1 passed, 1 failed\n",
        "no harness summary\n",
    ] {
        std::fs::write(&log, failed_log).unwrap();
        assert!(f.supervisor.factory(Parameters(request(&proof))).await.is_err(), "failed or empty selection evidence must not count");
    }
    std::fs::write(&log, "     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n").unwrap();
    std::fs::write(tested_repo.join("dirty.rs"), "// not tested\n").unwrap();
    assert!(f.supervisor.factory(Parameters(request(&proof))).await.is_err(), "dirty supervisor checkout must not count");
    std::fs::remove_file(tested_repo.join("dirty.rs")).unwrap();
    git(&tested_repo, &["checkout", "-q", "HEAD^"]);
    assert!(f.supervisor.factory(Parameters(request(&proof))).await.is_err(), "another tested commit cannot supply this tip's receipt");
    git(&tested_repo, &["checkout", "-q", &tested_tip]);
    let mut wrong_task = request(&proof);
    wrong_task.task_id = Some("cas-no-task".into());
    assert!(f.supervisor.factory(Parameters(wrong_task)).await.is_err(), "evidence must name an assigned task");
    std::fs::write(f.worker_path.join("dirty.rs"), "// worker changed\n").unwrap();
    assert!(f.supervisor.factory(Parameters(request(&proof))).await.is_err(), "dirty worker checkout must not count");
    std::fs::remove_file(f.worker_path.join("dirty.rs")).unwrap();
    for caller in ["not-registered", "shutdown-supervisor"] {
        if caller == "shutdown-supervisor" {
            let mut agent = Agent::new_with_role(caller.into(), caller.into(), AgentRole::Supervisor);
            agent.status = cas_types::AgentStatus::Shutdown;
            open_agent_store(&f.cas_dir()).unwrap().register(&agent).unwrap();
        }
        let core = CasCore::with_daemon(f.cas_dir(), None, None);
        core.set_agent_id_for_testing(caller.into());
        let service = CasService::new(core, None);
        assert!(service.factory(Parameters(request(&proof))).await.is_err(), "{caller} has no live supervisor authority");
    }
    let recorded = f.supervisor.factory(Parameters(request(&proof))).await;
    assert!(recorded.is_ok(), "registered supervisor evidence must reach the worker receipt: {recorded:?}");
    let receipt = crate::factory_worker_check::passing_test_receipts(&f.cas_dir(), &f.worker_path, &tested_tip).join("\n");
    // pin: supervisor-recorded distinguishes an attestation from a worker-executed run.
    assert!(receipt.contains("supervisor-recorded") && receipt.contains("br-supervisor-session"), "the gate must preserve execution provenance: {receipt}");
    let parked = call(&f.worker, close.clone()).await;
    assert!(parked.contains("MERGE REQUIRED"), "supervisor receipt must pass focused proof at park: {parked}");
    assert_eq!(f.task().status, TaskStatus::AwaitingMerge);
    assert!(f.task().notes.contains("br-supervisor-session"), "recording audits its registered issuer");
    let reopened = call(&f.supervisor, serde_json::json!({
        "action": "request_changes", "id": ID, "reason": "continue proof input regression",
    })).await;
    assert!(!reopened.contains("rejected"), "{reopened}");
    let resumed = call(&f.worker, serde_json::json!({"action": "start", "id": ID})).await;
    assert_eq!(f.task().status, TaskStatus::InProgress, "{resumed}");
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

    let external_embedded_path = "docs/RELEASE_SLACK_RUBRIC.md";
    std::fs::write(
        f.worker_path.join(external_embedded_path),
        "release rubric v2\n",
    )
    .unwrap();
    git(&f.worker_path, &["add", external_embedded_path]);
    git(
        &f.worker_path,
        &[
            "commit",
            "-q",
            "-m",
            "docs(cas-br01): change an external compiled-in rubric",
        ],
    );
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let stale_external_embedded = call(&f.worker, close.clone()).await;
    assert!(
        stale_external_embedded.contains("FOCUSED PROOF REQUIRED"),
        "receipt at {tested_tip} must not cover an outside-crate compiled-in Markdown file: {stale_external_embedded}"
    );
    assert_eq!(
        f.task().status,
        TaskStatus::InProgress,
        "{stale_external_embedded}"
    );
    // cas-70cb: another task can remove the include after this delivery was
    // measured. That newer HEAD must not let the old receipt cover its asset.
    let untested_delivery = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(&f.worker_path, &["checkout", "-q", "-b", "factory/without-include"]);
    std::fs::write(f.worker_path.join("crates/widget/src/lib.rs"), "pub mod gear;\n").unwrap();
    git(&f.worker_path, &["add", "crates/widget/src/lib.rs"]);
    git(&f.worker_path, &["commit", "-q", "-m", "next task removes include"]);
    assert!(
        crate::factory_worker_check::passing_test_receipts(&f.cas_dir(), &f.worker_path, &untested_delivery).is_empty(),
        "removing an include on another HEAD must not authorize the delivered asset change"
    );
    git(&f.worker_path, &["checkout", "-q", LANE]);
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(),
        &f.worker_path,
        &tested_tip,
        "widget",
        "test(gear)",
    );

    let doc_path = "docs/design/cli/update-managed-files.brief.md";
    // An older receipt whose range includes this obvious source input must
    // be rejected without searching Rust source for its filename.
    crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(), &f.worker_path, &tested_tip, "widget", "test(before_script)",
    );
    std::fs::write(f.worker_path.join("operator.py"), "print('next')\n").unwrap();
    git(&f.worker_path, &["add", "operator.py"]);
    git(&f.worker_path, &["commit", "-q", "-m", "update operator script"]);
    let tested_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    for filter in ["test(gear)", "test(adjacent)"] {
        crate::factory_worker_check::record_passing_test_for_tests(
            &f.cas_dir(), &f.worker_path, &tested_tip, "widget", filter,
        );
    }
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

    // The worker now starts another task whose Rust source names this brief.
    // The supervisor still closes the parked delivery's immutable tree.
    git(&f.worker_path, &["checkout", "-q", "-b", "factory/next-task"]);
    std::fs::write(
        f.worker_path.join(source_path),
        format!("pub fn turn() {{}}\nconst NEXT_TASK: &str = include_str!(\"../../../{doc_path}\");\n"),
    ).unwrap();
    git(&f.worker_path, &["add", source_path]);
    git(&f.worker_path, &["commit", "-q", "-m", "next task embeds brief"]);
    let receipts = crate::factory_worker_check::passing_test_receipts(&f.cas_dir(), &f.worker_path, &delivered_tip);
    assert!(receipts.iter().any(|receipt| receipt.contains(&format!("rebound from {tested_tip} to {delivered_tip}"))), "the parked tree still permits doc rebind after HEAD changes: {receipts:?}");
    // The executable pre-close hook requires the task's own lane, so restore
    // it after exercising the parked delivery's receipt reader on another HEAD.
    git(&f.worker_path, &["checkout", "-q", LANE]);

    git(&f.dir.path(), &["merge", "--ff-only", LANE]);
    git(&f.dir.path(), &["push", "-q", "origin", "main"]);
    let trace = f.dir.path().join("close-git-trace.jsonl");
    env.set("GIT_TRACE2_EVENT", &trace);
    let closed = call(&f.supervisor, close).await;
    env.remove("GIT_TRACE2_EVENT");
    assert!(closed.contains("Closed task"), "{closed}");
    let task = f.task();
    assert_eq!(task.status, TaskStatus::Closed);
    assert!(!task.notes.contains("test(before_script)"), "the script invalidates its older receipt: {}", task.notes);
    assert!(
        task.notes
            .contains(&format!("rebound from {tested_tip} to {delivered_tip}")),
        "the close must record the receipt rebind: {}",
        task.notes
    );
    // Count real Git commands rather than inspecting cache internals. Both
    // receipt filters and the close's package/audit readers share this lookup.
    let trace = std::fs::read_to_string(trace).unwrap();
    let git_calls: Vec<Vec<String>> = trace.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["event"] == "start")
        .filter_map(|event| Some(event["argv"].as_array()?.iter().filter_map(|arg| arg.as_str().map(str::to_owned)).collect::<Vec<_>>()))
        .collect();
    let brief = Path::new(doc_path).file_name().unwrap().to_str().unwrap();
    let inventories = git_calls.iter().filter(|args| {
        args.get(1).is_some_and(|arg| arg == "ls-tree")
            && args.iter().any(|arg| arg == "--full-tree")
            && args.iter().any(|arg| arg == &delivered_tip)
    }).count();
    assert!(inventories <= 1, "at most one delivered-tree inventory per close: {git_calls:?}");
    let cat_batches = git_calls.iter().filter(|args| {
        args.get(1).is_some_and(|arg| arg == "cat-file")
            && args.iter().any(|arg| arg == "--batch")
    }).count();
    assert!(cat_batches <= 1, "at most one blob batch per close: {git_calls:?}");
    assert!(git_calls.iter().all(|args| {
        args.get(1).is_none_or(|arg| arg != "grep") || !args.iter().any(|arg| arg == brief)
    }), "the include lookup should use the batched delivered tree: {git_calls:?}");
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

/// cas-e9d6: the 14:01 divergence. Cassy's post-merge close of a parked
/// blast-radius delivery judges the worker's receipts at the delivered tip,
/// exactly as the worker's own close does. A gate that truly fails on the
/// verdict is the supervisor's: no worker retry, the worker hold is restored
/// and no close-authority note is written until a close commits.
#[tokio::test]
async fn cassy_merge_close_judges_the_workers_receipt_scope_cas_e9d6() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    std::fs::write(
        f.cas_dir().join("config.toml"),
        format!("[project]\ncanonical_id=\"{PROJECT}\"\n"),
    )
    .unwrap();
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    let record = || crate::factory_worker_check::record_passing_test_for_tests(
        &f.cas_dir(), &f.worker_path, &tip, "widget", "test(gear)",
    );
    record();
    let parked = call(&f.worker, serde_json::json!({"action": "close", "id": ID, "reason": "gear turns"})).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");

    let repo = f.dir.path();
    git(repo, &["merge", "-q", "--no-ff", LANE, "-m", "merge lane"]);
    git(repo, &["push", "-q", "origin", "main"]);
    let merge = git(repo, &["rev-parse", "HEAD"]);
    let tasks = open_task_store(&f.cas_dir()).unwrap();
    tasks.record_merge_admission(ID, &merge, cas_types::MergeCiAdmission::Green).unwrap();

    env.set("CAS_AGENT_ROLE", "supervisor");
    let cassy = CasCore::with_daemon(f.cas_dir(), None, None);
    cassy.set_agent_id_for_testing("br-supervisor-session".into());
    // The worker's receipts cover `widget`, so Cassy's close reaches the
    // verification gate instead of refusing the proof scope.
    let first = cassy.close_green_merged_delivery(ID).await.receipt(ID);
    assert!(first.contains("VERIFICATION REQUIRED"), "{first}");
    let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), ID).unwrap().unwrap();

    // The worker blocks on that dispatch; then the receipts disappear, so a
    // proof gate truly fails when the verdict arrives.
    env.set("CAS_AGENT_ROLE", "worker");
    call(&f.worker, serde_json::json!({"action": "update", "id": ID, "status": "blocked"})).await;
    assert_eq!(f.task().status, TaskStatus::Blocked);
    std::fs::remove_dir_all(f.cas_dir().join("worker-checks")).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let verdict = f
        .supervisor
        .verification(Parameters(serde_json::from_value(serde_json::json!({
            "action": "add", "task_id": ID, "dispatch_id": dispatch.id,
            "status": "approved", "summary": "gear turns", "confidence": 1.0,
        })).unwrap()))
        .await
        .expect("verdict");
    let verdict = verdict.content.iter().filter_map(|c| match &c.raw {
        rmcp::model::RawContent::Text(text) => Some(text.text.clone()),
        _ => None,
    }).collect::<Vec<_>>().join("\n");
    assert!(verdict.contains("narrower than its delivery diff"), "the gate is reported: {verdict}");
    assert!(verdict.contains("No worker close retry"), "{verdict}");
    let held = f.task();
    assert_eq!(held.status, TaskStatus::Blocked, "the worker hold is restored");
    assert!(crate::mcp::tools::core::task::block_hold::read(tasks.as_ref(), ID).unwrap().is_some());
    assert!(!held.notes.contains("Cassy close authority"), "{}", held.notes);
    let worker_prompts = crate::store::open_prompt_queue_store(&f.cas_dir()).unwrap()
        .peek_all(50).unwrap().into_iter().filter(|p| p.target == WORKER)
        .map(|p| p.prompt).collect::<Vec<_>>();
    assert!(!worker_prompts.iter().any(|p| p.contains("Retry task close")), "{worker_prompts:?}");

    // With the worker's receipts back, the recorded verdict closes it.
    record();
    let verification = cas_store::get_verification_for_dispatch(&f.cas_dir(), &dispatch.id).unwrap().unwrap();
    let closed = cassy.close_green_merged_delivery_after_verdict(ID, Some(&verification.id)).await;
    assert!(
        matches!(closed, super::super::merge_close::CassyMergeClose::Closed { .. }),
        "{}",
        closed.receipt(ID)
    );
    let closed = f.task();
    assert_eq!(closed.status, TaskStatus::Closed);
    assert!(closed.notes.contains("Cassy close authority"), "{}", closed.notes);
}
