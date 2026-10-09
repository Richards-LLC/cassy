//! cas-c6e1: recovery must bind close, QA and the tracked merge guard to one
//! current delivery. This is the installed-520cd74 failure shape; cas-a44a's
//! record_park fix already exists on the epic, so test its real consumers.
use super::*;
use crate::store::{
    open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
    open_worktree_store,
};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, TaskRisk, WorkTarget};
use std::path::Path;

const TASK: &str = "cas-c6e1-fixture";
const WORKER: &str = "recovered-worker";
const BRANCH: &str = "factory/recovered-worker";
const OLD_BRANCH: &str = "factory/dead-worker";

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
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct Recovery {
    dir: tempfile::TempDir,
    core: CasCore,
    worker_path: std::path::PathBuf,
    old: String,
    predecessor: String,
    final_tip: String,
}

fn fixture(env: &mut TestEnvGuard) -> Recovery {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    git(repo, &["checkout", "-q", "-b", OLD_BRANCH]);
    std::fs::write(repo.join("footer.css"), ".footer { gap: 4px; }\n").unwrap();
    git(repo, &["add", "footer.css"]);
    git(
        repo,
        &[
            "commit",
            "-q",
            "-m",
            "fix(cas-c6e1-fixture): original delivery",
        ],
    );
    let old = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "-b", BRANCH]);
    std::fs::write(repo.join("footer.css"), ".footer { gap: 8px; }\n").unwrap();
    git(repo, &["add", "footer.css"]);
    git(
        repo,
        &[
            "commit",
            "-q",
            "-m",
            "fix(cas-c6e1-fixture): recovered delivery",
        ],
    );
    let predecessor = git(repo, &["rev-parse", "HEAD"]);
    // The last commit merges unrelated, already-integrated epic content;
    // the delivered web tree stays identical to the predecessor, but QA
    // must still bind to the explicit final receipt, never that predecessor.
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("epic.txt"), "current epic\n").unwrap();
    git(repo, &["add", "epic.txt"]);
    git(
        repo,
        &["commit", "-q", "-m", "chore(cas-other): epic update"],
    );
    git(repo, &["checkout", "-q", BRANCH]);
    git(
        repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "main",
            "-m",
            "merge current epic for cas-c6e1-fixture",
        ],
    );
    let final_tip = git(repo, &["rev-parse", "HEAD"]);
    // Remote refs provide a pushed delivery without any network dependency.
    git(
        repo,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{BRANCH}"),
            &final_tip,
        ],
    );
    git(repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    let cas_dir = repo.join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_FACTORY_MODE", "1");
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    std::fs::write(cas_dir.join("config.toml"),
        "[project]\ncanonical_id=\"cas-c6e1-fixture-project\"\n[verification]\nenabled=false\n[qa]\nevidence_gate=false\nindependent_pass=true\n").unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_rule_store(&cas_dir).unwrap().init().unwrap();
    open_skill_store(&cas_dir).unwrap().init().unwrap();
    open_worktree_store(&cas_dir).unwrap().init().unwrap();
    // A real linked worker worktree is required for close's execution-root
    // validation; using the main checkout would test a different failure.
    git(repo, &["checkout", "-q", "main"]);
    let worker_path = cas_dir.join("worktrees").join(WORKER);
    std::fs::create_dir_all(worker_path.parent().unwrap()).unwrap();
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            worker_path.to_str().unwrap(),
            BRANCH,
        ],
    );
    std::fs::create_dir_all(worker_path.join(".cas")).unwrap();
    std::fs::write(
        worker_path.join(".cas/config.toml"),
        "[project]\ncanonical_id=\"cas-c6e1-fixture-project\"\n",
    )
    .unwrap();
    let tasks = open_task_store(&cas_dir).unwrap();
    tasks.init().unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    agents
        .register(&Agent::new_with_role(
            "recovery-session".into(),
            WORKER.into(),
            AgentRole::Worker,
        ))
        .unwrap();
    agents
        .register(&Agent::new_with_role(
            "supervisor-session".into(),
            "supervisor".into(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    let mut task = Task::new(TASK.into(), "Recover machine footer delivery".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(WORKER.into());
    task.risk = vec![TaskRisk::None];
    task.demo_statement = "Read the machine footer with its status".into();
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "project:cas-c6e1-fixture-project".into(),
        target_branch: "main".into(),
    });
    // Exactly the incident: stale old-worker branch and new worker's
    // commit-hook predecessor anchor coexist before close with final receipt.
    task.deliverables.parked_branch = Some(OLD_BRANCH.into());
    task.deliverables.factory_branch_anchor = Some(predecessor.clone());
    task.deliverables
        .historical_factory_branch_anchors
        .push(old.clone());
    tasks.add(&task).unwrap();
    agents
        .try_claim(
            TASK,
            "recovery-session",
            1800,
            Some("authenticated recovery"),
        )
        .unwrap();
    let core = CasCore::with_daemon(cas_dir, None, None);
    core.set_agent_id_for_testing("recovery-session".into());
    Recovery {
        dir,
        core,
        worker_path,
        old,
        predecessor,
        final_tip,
    }
}

fn text(result: CallToolResult) -> String {
    result
        .content
        .into_iter()
        .filter_map(|content| match content.raw {
            rmcp::model::RawContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn close_output(fixture: &Recovery) -> String {
    text(
        fixture
            .core
            .cas_task_close(Parameters(TaskCloseRequest {
                id: TASK.into(),
                reason: Some("Recovered final pushed tip".into()),
                commit_receipt: Some(fixture.final_tip.clone()),
                supervisor_override: None,
                stranded_branch_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
            }))
            .await
            .unwrap(),
    )
}

async fn close(fixture: &Recovery) -> Task {
    let output = close_output(fixture).await;
    assert!(output.contains("MERGE REQUIRED"), "{output}");
    let parked = open_task_store(&fixture.dir.path().join(".cas"))
        .unwrap()
        .get(TASK)
        .unwrap();
    assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
    parked
}

#[tokio::test]
async fn recovery_close_parks_final_receipt_and_dispatches_that_tip_cas_c6e1() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let parked = close(&f).await;
    assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
    assert_eq!(
        parked.deliverables.factory_branch_anchor.as_deref(),
        Some(f.final_tip.as_str())
    );
    assert!(
        parked
            .deliverables
            .handoff_branches
            .iter()
            .any(|b| b == OLD_BRANCH)
    );
    assert!(
        parked
            .deliverables
            .historical_factory_branch_anchors
            .contains(&f.old)
    );
    assert!(
        parked
            .deliverables
            .historical_factory_branch_anchors
            .contains(&f.predecessor)
    );
    let rounds = cas_store::list_qa_passes(&f.dir.path().join(".cas"), TASK).unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].bound_head, f.final_tip);
    assert_eq!(rounds[0].branch, BRANCH);
}

#[tokio::test]
async fn recovery_tracked_merge_rejects_old_qa_and_selects_current_tip_cas_c6e1() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    close(&f).await;
    let cas_dir = f.dir.path().join(".cas");
    // Test-only review records exercise the guard, not a production waiver.
    cas_store::waive_qa_pass(
        &cas_dir,
        TASK,
        "supervisor-session",
        WORKER,
        OLD_BRANCH,
        &f.old,
        "reviewed old delivery only",
        chrono::Utc::now(),
    )
    .unwrap();
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let error = supervisor
        .worktree_merge(WORKER, false, Some(TASK), false, Some(false), false, None)
        .await
        .expect_err("tracked merge must require QA for current recovered tip");
    assert!(
        error.message.contains("INDEPENDENT QA REQUIRED"),
        "{error:?}"
    );
    assert!(
        error.message.contains(&format!(
            "no passed or waived QA round covers @{};",
            &f.final_tip[..8]
        )),
        "must select the current delivery; historical QA may appear in the audit detail: {error:?}"
    );
    cas_store::waive_qa_pass(
        &cas_dir,
        TASK,
        "supervisor-session",
        WORKER,
        BRANCH,
        &f.final_tip,
        "reviewed exact recovered delivery",
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(
        supervisor
            .independent_qa_merge_refusal(TASK, WORKER, f.dir.path())
            .is_none()
    );
    // A later branch amendment cannot borrow the recovered tip's waiver.
    std::fs::write(f.worker_path.join("footer.css"), ".footer { gap: 12px; }\n").unwrap();
    git(&f.worker_path, &["add", "footer.css"]);
    git(
        &f.worker_path,
        &[
            "commit",
            "-q",
            "-m",
            "fix(cas-c6e1-fixture): unreviewed amendment",
        ],
    );
    assert!(
        supervisor
            .independent_qa_merge_refusal(TASK, WORKER, f.dir.path())
            .is_some()
    );
}

#[tokio::test]
async fn recovery_close_rejects_foreign_checkout_before_rebinding_cas_c6e1() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    git(
        &f.worker_path,
        &["checkout", "-q", "-b", "factory/another-worker"],
    );
    let output = close_output(&f).await;
    assert!(
        output.contains("PRE-CLOSE HOOK CONTEXT REJECTED"),
        "{output}"
    );
    let cas_dir = f.dir.path().join(".cas");
    let task = open_task_store(&cas_dir).unwrap().get(TASK).unwrap();
    assert_eq!(task.status, TaskStatus::InProgress, "{output}");
    assert_eq!(task.deliverables.parked_branch.as_deref(), Some(OLD_BRANCH));
    assert_eq!(
        task.deliverables.factory_branch_anchor.as_deref(),
        Some(f.predecessor.as_str())
    );
    assert!(task.deliverables.handoff_branches.is_empty());
    assert!(
        cas_store::list_qa_passes(&cas_dir, TASK)
            .unwrap()
            .is_empty()
    );
}

/// The supported audited recovery also exists in installed 520cd74: retire
/// both stale active fields, keep their audit trail, then start a new cycle.
#[tokio::test]
async fn recovery_request_changes_start_close_binds_final_tip_cas_c6e1() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let cas_dir = f.dir.path().join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut parked = tasks.get(TASK).unwrap();
    parked.status = TaskStatus::AwaitingMerge;
    tasks.update(&parked).unwrap();
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "supervisor");
    let verdict = text(supervisor.cas_task_request_changes(Parameters(
        crate::mcp::tools::TaskRequestChangesRequest {
            id: TASK.into(),
            reason: "Preserve prior source work; correct stale branch/QA binding by re-delivering the reviewed final tip, with no source amendment".into(),
        }
    )).await.unwrap());
    let reopened = tasks.get(TASK).unwrap();
    assert_eq!(reopened.status, TaskStatus::Open, "{verdict}");
    assert_eq!(reopened.assignee.as_deref(), Some(WORKER));
    assert!(reopened.deliverables.parked_branch.is_none());
    assert!(reopened.deliverables.factory_branch_anchor.is_none());
    assert!(
        reopened
            .deliverables
            .historical_factory_branch_anchors
            .contains(&f.predecessor)
    );
    assert!(reopened.notes.contains(OLD_BRANCH), "{}", reopened.notes);
    env.set("CAS_AGENT_ROLE", "worker");
    let started = text(
        f.core
            .cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
                id: TASK.into(),
                brief: Some(true),
            }))
            .await
            .unwrap(),
    );
    assert_eq!(
        tasks.get(TASK).unwrap().status,
        TaskStatus::InProgress,
        "{started}"
    );
    let recovered = close(&f).await;
    assert_eq!(
        recovered.deliverables.parked_branch.as_deref(),
        Some(BRANCH)
    );
    assert_eq!(
        recovered.deliverables.factory_branch_anchor.as_deref(),
        Some(f.final_tip.as_str())
    );
    let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].bound_head, f.final_tip);
    assert_eq!(rounds[0].branch, BRANCH);
}

/// cas-3b81 (GH #1142): a close's cost does not grow with the task's note
/// history. The timed-out incident tasks were old, with long histories; this
/// close carries a ~2 MB history of receipts, SHAs and proof lines, takes the
/// same commit_receipt gate path as a fresh task, and stays far inside the
/// 55s tool budget.
#[tokio::test]
async fn long_note_history_close_stays_well_under_the_tool_budget_cas_3b81() {
    let mut short_env = TestEnvGuard::temp_home();
    let short = fixture(&mut short_env);
    let started = std::time::Instant::now();
    let short_output = close_output(&short).await;
    let short_elapsed = started.elapsed();
    drop(short);
    drop(short_env);

    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let tasks = open_task_store(&f.dir.path().join(".cas")).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    let history = (0..6_000)
        .map(|n| match n % 4 {
            0 => format!(
                "[2026-09-{:02} {:02}:{:02}] 📝 PROGRESS round {n}: pushed {} to {BRANCH}; merged {} into main; QA receipt /tmp/cas-qa/round-{n}/receipt.json",
                1 + n % 28,
                n % 24,
                n % 60,
                f.predecessor,
                f.old,
            ),
            1 => format!(
                "[2026-09-{:02} {:02}:{:02}] SCOPED_PROOF: command=scripts/run-scoped-tests.sh --proof -p cas --lib result=PASS base={} head={}",
                1 + n % 28,
                n % 24,
                n % 60,
                f.old,
                f.final_tip,
            ),
            2 => format!(
                "[2026-09-{:02} {:02}:{:02}] 🧪 LOADED_PROOF cargo test -j16 x3 loops result=PASS on {}",
                1 + n % 28,
                n % 24,
                n % 60,
                f.predecessor,
            ),
            _ => format!(
                "[2026-09-{:02} {:02}:{:02}] ✅ DECISION round {n} reviewed footer.css at {}; Closed: no, still parked on {OLD_BRANCH}",
                1 + n % 28,
                n % 24,
                n % 60,
                f.final_tip,
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    task.notes = format!("{history}\n\n{}", task.notes);
    assert!(task.notes.len() > 1_500_000, "{}", task.notes.len());
    tasks.update(&task).unwrap();

    let started = std::time::Instant::now();
    let output = close_output(&f).await;
    let elapsed = started.elapsed();
    eprintln!(
        "cas-3b81 close timing: short notes {short_elapsed:?}, {} byte notes {elapsed:?}",
        task.notes.len()
    );
    assert!(output.contains("MERGE REQUIRED"), "{output}");
    assert!(short_output.contains("MERGE REQUIRED"), "{short_output}");
    assert!(
        elapsed < std::time::Duration::from_secs(15),
        "long-note close took {elapsed:?}; the tool budget is 55s"
    );
    assert!(
        elapsed <= short_elapsed * 3 + std::time::Duration::from_secs(3),
        "close cost grew with note history: short {short_elapsed:?}, long {elapsed:?}"
    );
}
