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
    fixture_with_surface(env, "footer.css", false)
}

fn fixture_with_surface(env: &mut TestEnvGuard, surface: &str, evidence_gate: bool) -> Recovery {
    let (original_body, recovered_body) = match surface {
        "footer.css" => (".footer { gap: 4px; }\n", ".footer { gap: 8px; }\n"),
        path if path.ends_with(".ts") => (
            "logger.debug(\"render started\");\n",
            "logger.debug(\"render completed\");\n",
        ),
        _ => (
            "println!(\"status: waiting\");\n",
            "println!(\"status: ready\");\n",
        ),
    };
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    git(repo, &["checkout", "-q", "-b", OLD_BRANCH]);
    std::fs::create_dir_all(repo.join(surface).parent().unwrap()).unwrap();
    std::fs::write(repo.join(surface), original_body).unwrap();
    git(repo, &["add", surface]);
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
    std::fs::write(repo.join(surface), recovered_body).unwrap();
    git(repo, &["add", surface]);
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
        format!("[project]\ncanonical_id=\"cas-c6e1-fixture-project\"\n[verification]\nenabled=false\n[qa]\nmode='per_task'\nevidence_gate={evidence_gate}\nindependent_pass=true\n")).unwrap();
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
                no_delivery: None,
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

/// GH #1382: a start boundary can contain UI inherited from a reused lane.
/// Only the source range delivered after that boundary owes UI evidence.
#[tokio::test]
async fn close_classifies_delivery_after_start_boundary_gh1382() {
    let mut env = TestEnvGuard::temp_home();
    for surface in ["api.ts", "delivered.vue"] {
        let mut f = fixture_with_surface(&mut env, "note.txt", true);
        let repo = f.dir.path();
        git(&f.worker_path, &["reset", "--hard", "main"]);
        std::fs::write(f.worker_path.join("inherited.vue"), "<template>Earlier UI</template>\n").unwrap();
        git(&f.worker_path, &["add", "inherited.vue"]);
        git(&f.worker_path, &["commit", "-qm", "Earlier lane work"]);
        let start = git(&f.worker_path, &["rev-parse", "HEAD"]);
        std::fs::write(f.worker_path.join(surface), "export const result = 'delivered';\n").unwrap();
        git(&f.worker_path, &["add", surface]);
        git(&f.worker_path, &["commit", "-qm", &format!("fix({TASK}): deliver source change")]);
        f.final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        git(repo, &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &f.final_tip]);
        let cas_dir = repo.join(".cas");
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.demo_statement.clear();
        task.deliverables.parked_branch = None;
        task.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
        task.deliverables.historical_factory_branch_anchors.clear();
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(),
            branch: BRANCH.into(),
            base_sha: start,
        });
        tasks.update(&task).unwrap();

        let output = close_output(&f).await;
        if surface.ends_with(".vue") {
            assert!(output.contains("QA evidence bundle"), "delivered UI must owe evidence: {output}");
            assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::InProgress, "{output}");
        } else {
            assert!(output.contains("MERGE REQUIRED"), "API-only delivery must park: {output}");
            assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge, "{output}");
            assert!(cas_store::list_qa_passes(&cas_dir, TASK).unwrap().is_empty());
            let state = tasks.get_execution_state(TASK).unwrap().unwrap();
            let files: Vec<_> = state["qa_impact"]["unowned"].as_array().unwrap().iter()
                .filter_map(|entry| entry["file"].as_str()).collect();
            assert!(files.contains(&surface), "API change must appear in delivered impact: {state}");
            assert!(!files.contains(&"inherited.vue"), "inherited UI must not select a review journey: {state}");
            git(repo, &["merge", "-q", "--no-ff", BRANCH, "-m", "Integrate API delivery"]);
            git(repo, &["update-ref", "refs/remotes/origin/main", "main"]);
            let output = close_output(&f).await;
            assert!(output.contains("Closed task:"), "API-only delivery must also close after integration: {output}");
            assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Closed, "{output}");
            assert!(cas_store::list_qa_passes(&cas_dir, TASK).unwrap().is_empty());
        }
    }
}

/// cas-2bec: spawn assigns a plain lane. The commit hook records the RED
/// commit, but only the pushed GREEN tip has a passing fast-rows receipt.
#[tokio::test]
async fn shared_lane_red_green_close_checks_pushed_tip_cas_2bec() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    git(&f.worker_path, &["reset", "--hard", "main"]);
    std::fs::create_dir_all(f.worker_path.join("scripts")).unwrap();
    std::fs::write(
        f.worker_path.join("scripts/fast-rows-pre-push.sh"),
        "#!/bin/sh\n",
    )
    .unwrap();
    std::fs::write(f.worker_path.join("regression.txt"), "RED regression\n").unwrap();
    git(&f.worker_path, &["add", "scripts", "regression.txt"]);
    git(
        &f.worker_path,
        &["commit", "-qm", &format!("test({TASK}): RED regression")],
    );
    let red = git(&f.worker_path, &["rev-parse", "HEAD"]);
    std::fs::write(f.worker_path.join("fix.txt"), "GREEN fix\n").unwrap();
    git(&f.worker_path, &["add", "fix.txt"]);
    git(
        &f.worker_path,
        &[
            "commit",
            "-qm",
            &format!("fix({TASK}): GREEN implementation"),
        ],
    );
    let green = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(
        repo,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{BRANCH}"),
            &green,
        ],
    );

    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let receipts = Path::new(&common).join("cas/fast-rows");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(
        receipts.join(format!("{green}.pass")),
        format!("fast-rows: PASS {green} fixture\n"),
    )
    .unwrap();
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.demo_statement.clear();
    task.deliverables.parked_branch = None;
    task.deliverables.factory_branch_anchor = Some(red.clone());
    task.deliverables.historical_factory_branch_anchors.clear();
    tasks.update(&task).unwrap();

    let output = text(
        f.core
            .cas_task_close(Parameters(TaskCloseRequest {
                id: TASK.into(),
                reason: Some(format!("PASS {green}: RED then GREEN on spawn lane")),
                commit_receipt: None,
                no_delivery: None,
                supervisor_override: None,
                stranded_branch_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
            }))
            .await
            .unwrap(),
    );
    assert!(output.contains("MERGE REQUIRED"), "{output}");
    let parked = tasks.get(TASK).unwrap();
    assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
    assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
    assert_eq!(
        parked.deliverables.factory_branch_anchor.as_deref(),
        Some(green.as_str()),
        "{output}"
    );
    assert!(
        parked.notes.contains(&red),
        "the original RED boundary stays audited"
    );
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

/// GH #1389: the first final close must use the pushed receipt, even when a
/// commit hook saved its predecessor before the worker synchronized the target.
#[tokio::test]
async fn first_close_explicit_final_receipt_binds_qa_and_merge_request_gh1389() {
    let mut env = TestEnvGuard::temp_home();
    let mut f = fixture(&mut env);
    // The final target-sync merge also carries the worker's last source
    // correction. Prior-head QA is therefore stale, not source-equivalent.
    std::fs::write(f.worker_path.join("footer.css"), ".footer { gap: 12px; }\n").unwrap();
    git(&f.worker_path, &["add", "footer.css"]);
    git(&f.worker_path, &["commit", "--amend", "-q", "--no-edit"]);
    f.final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(f.dir.path(), &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &f.final_tip]);
    let cas_dir = f.dir.path().join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.deliverables.parked_branch = None;
    task.deliverables.historical_factory_branch_anchors.clear();
    tasks.update(&task).unwrap();

    let unrelated = git(f.dir.path(), &["rev-parse", "main"]);
    for (label, receipt) in [("prior head", &f.predecessor), ("unrelated target", &unrelated)] {
        let output = text(f.core.cas_task_close(Parameters(TaskCloseRequest {
            id: TASK.into(),
            reason: Some(format!("{label} receipt must refuse")),
            commit_receipt: Some(receipt.clone()),
            no_delivery: None,
            supervisor_override: None,
            stranded_branch_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
        })).await.unwrap());
        assert!(output.contains("REJECTED") || output.contains("REFUSED"), "{label}: {output}");
        let unchanged = tasks.get(TASK).unwrap();
        assert_eq!(unchanged.status, TaskStatus::InProgress, "{label}: {output}");
        assert_eq!(unchanged.deliverables.factory_branch_anchor.as_deref(), Some(f.predecessor.as_str()));
        assert!(cas_store::list_qa_passes(&cas_dir, TASK).unwrap().is_empty());
    }

    let parked = close(&f).await;
    assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
    assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(f.final_tip.as_str()));
    let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].bound_head, f.final_tip);

    let service = crate::mcp::CasService::new(f.core, None);
    let request = serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "task_id": TASK,
        "summary": "Merge final delivery", "message": "Merge the final pushed receipt",
        "merge_request": true,
    })).unwrap();
    let output = text(service.coordination(Parameters(request)).await.unwrap());
    assert!(!output.contains("RE-PARK"), "{output}");
    let rows = crate::store::open_prompt_queue_store(&cas_dir).unwrap().poll_all(50).unwrap();
    assert!(rows.iter().any(|row| {
        crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
            .is_some_and(|envelope| envelope.task_id == TASK && envelope.branch_tip == f.final_tip)
    }), "{output}");

    cas_store::waive_qa_pass(
        &cas_dir, TASK, "supervisor-session", WORKER, BRANCH,
        &f.predecessor, "reviewed predecessor only", chrono::Utc::now(),
    ).unwrap();
    let supervisor = CasCore::with_daemon(cas_dir, None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let refusal = supervisor.independent_qa_merge_refusal(TASK, WORKER, f.dir.path());
    assert!(refusal.is_some(), "prior-head QA must not approve the final receipt");
}

/// GH #1355: a pushed target-sync merge is the new park boundary, even
/// when the saved start base arrived through that merge rather than B.
#[tokio::test]
async fn repark_target_sync_advances_anchor_qa_and_merge_request_gh1355() {
    let mut env = TestEnvGuard::temp_home();
    for (target_first, receipt, rewrite) in [(false, true, false), (true, false, false), (false, true, true)] {
        let f = fixture(&mut env);
        let repo = f.dir.path();
        let cas_dir = repo.join(".cas");
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.parked_branch = Some(BRANCH.into());
        let target_base = git(repo, &["rev-parse", "main"]);
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(),
            branch: BRANCH.into(),
            base_sha: target_base.clone(),
        });
        tasks.update(&task).unwrap();
        cas_store::open_qa_pass(&cas_dir, &cas_store::NewQaPass {
            task_id: TASK,
            implementer_agent_id: "recovery-session",
            branch: BRANCH,
            bound_head: &f.predecessor,
            deadline_at: chrono::Utc::now() + chrono::Duration::minutes(30),
            max_rounds: 3,
        }, chrono::Utc::now()).unwrap();
        if target_first || rewrite {
            git(&f.worker_path, &["reset", "--hard", "main"]);
            if rewrite {
                std::fs::write(f.worker_path.join("footer.css"), ".footer { gap: 99px; }\n").unwrap();
                git(&f.worker_path, &["add", "footer.css"]);
                git(&f.worker_path, &["commit", "-qm", "fix(cas-c6e1-fixture): rewritten delivery"]);
            } else {
                git(&f.worker_path, &["merge", "--no-ff", &f.predecessor, "-m", "merge target sync for cas-c6e1-fixture"]);
            }
        }
        let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        assert!(git_commit_is_ancestor(repo, &target_base, &tip));
        assert_eq!(git_commit_is_ancestor(repo, &f.predecessor, &tip), !rewrite);
        let origin = cas_dir.join("origin.git");
        git(repo, &["init", "--bare", origin.to_str().unwrap()]);
        git(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(repo, &["push", "-q", "origin", "main", BRANCH]);
        let result = f.core.cas_task_close(Parameters(TaskCloseRequest {
            id: TASK.into(),
            reason: Some("Re-park current pushed merge tip".into()),
            commit_receipt: receipt.then(|| tip.clone()),
            no_delivery: None,
            supervisor_override: None,
            stranded_branch_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
        })).await.unwrap();
        let output = text(result);
        let parked = tasks.get(TASK).unwrap();
        if rewrite {
            assert!(output.contains("rewrit") && !output.contains("base is no longer an ancestor"), "{output}");
            assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(f.predecessor.as_str()));
            continue;
        }
        assert!(output.contains("MERGE REQUIRED"), "{output}");
        assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()), "{output}");
        let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds.iter().find(|round| round.bound_head == f.predecessor).unwrap().state,
            cas_types::QaPassState::Superseded);
        assert_eq!(rounds[0].bound_head, tip);
        let service = crate::mcp::CasService::new(f.core, None);
        let request = serde_json::from_value(serde_json::json!({
            "action": "message", "target": "supervisor", "task_id": TASK,
            "summary": "Merge current delivery", "message": "Merge the re-parked tip",
            "merge_request": true,
        })).unwrap();
        let output = text(service.coordination(Parameters(request)).await.unwrap());
        assert!(!output.contains("RE-PARK"), "{output}");
        let rows = crate::store::open_prompt_queue_store(&cas_dir).unwrap().poll_all(50).unwrap();
        assert!(rows.iter().any(|row| {
            crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
                .is_some_and(|envelope| envelope.task_id == TASK && envelope.branch_tip == tip)
        }), "{output}");
    }
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

/// A fresh review cycle must park the actual delivery after conflict resolution
/// rewrites the commit-time anchor, without requiring an explicit receipt.
#[tokio::test]
async fn recovery_request_changes_rebase_close_binds_current_tip_without_receipt_cas_4fcb() {
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
            reason: "Rebase the reviewed correction onto the current target and deliver the resolved content".into(),
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
                force: None,
            }))
            .await
            .unwrap(),
    );
    assert_eq!(
        tasks.get(TASK).unwrap().status,
        TaskStatus::InProgress,
        "{started}"
    );
    // A new review cycle records its commit through the real PostToolUse hook.
    // Rebase conflict resolution then rewrites that patch without a git commit
    // hook, leaving the recorded anchor at the pre-rebase object.
    std::fs::write(f.worker_path.join("review.txt"), "worker correction\n").unwrap();
    git(&f.worker_path, &["add", "review.txt"]);
    git(
        &f.worker_path,
        &[
            "commit",
            "-q",
            "-m",
            "fix(cas-c6e1-fixture): review correction",
        ],
    );
    let before_rebase = git(&f.worker_path, &["rev-parse", "HEAD"]);
    crate::hooks::handlers::handlers_events::detect_and_link_git_commit(
        &cas_dir,
        &crate::hooks::HookInput {
            session_id: "recovery-session".into(),
            cwd: f.worker_path.display().to_string(),
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("Bash".into()),
            tool_input: Some(
                serde_json::json!({"command": "git commit -q -m 'fix(cas-c6e1-fixture): review correction'"}),
            ),
            tool_response: Some(serde_json::json!({"exitCode": 0, "stdout": ""})),
            agent_role: Some("worker".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        tasks
            .get(TASK)
            .unwrap()
            .deliverables
            .factory_branch_anchor
            .as_deref(),
        Some(before_rebase.as_str())
    );
    std::fs::write(f.dir.path().join("review.txt"), "target update\n").unwrap();
    git(f.dir.path(), &["add", "review.txt"]);
    git(
        f.dir.path(),
        &["commit", "-q", "-m", "chore(cas-abcd): target update"],
    );
    git(
        f.dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let conflict = std::process::Command::new("git")
        .args(["rebase", "main"])
        .current_dir(&f.worker_path)
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(!conflict.status.success(), "fixture must conflict");
    assert!(
        String::from_utf8_lossy(&conflict.stdout)
            .contains("CONFLICT (add/add): Merge conflict in review.txt"),
        "unexpected rebase stop: {conflict:?}"
    );
    std::fs::write(
        f.worker_path.join("review.txt"),
        "target update plus reviewed correction\n",
    )
    .unwrap();
    git(&f.worker_path, &["add", "review.txt"]);
    git(
        &f.worker_path,
        &["-c", "core.editor=true", "rebase", "--continue"],
    );
    let rebased = git(&f.worker_path, &["rev-parse", "HEAD"]);
    assert_ne!(rebased, before_rebase);
    // Context in the body cannot turn a sibling's subject into our delivery.
    // Keep the task-owned row last so the real close below delivers that tip.
    let active = tasks.get(TASK).unwrap();
    for (message, advances) in [
        (
            format!("fix(cas-abcd): sibling change\n\nContext: {TASK} (cas-4fcb regression)"),
            false,
        ),
        (format!("fix({TASK}): reviewed correction"), true),
    ] {
        git(&f.worker_path, &["commit", "-q", "--amend", "-m", &message]);
        let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        let parking = f.core.advance_commit_time_anchor_before_park(
            tasks.as_ref(),
            &active,
            f.dir.path(),
            "main",
            Some(BRANCH),
            Some(&tip),
        );
        assert_eq!(
            parking.deliverables.factory_branch_anchor.as_deref(),
            Some(if advances {
                tip.as_str()
            } else {
                before_rebase.as_str()
            }),
            "delivery ownership for {message:?}",
        );
    }
    let rebased = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(
        f.dir.path(),
        &[
            "update-ref",
            &format!("refs/remotes/origin/{BRANCH}"),
            &rebased,
        ],
    );
    let output = text(
        f.core
            .cas_task_close(Parameters(TaskCloseRequest {
                no_delivery: None,
                id: TASK.into(),
                reason: Some(format!("PASS {rebased}: reviewed rebased correction")),
                commit_receipt: None,
                supervisor_override: None,
                stranded_branch_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
            }))
            .await
            .unwrap(),
    );
    assert!(output.contains("MERGE REQUIRED"), "{output}");
    let recovered = tasks.get(TASK).unwrap();
    assert_eq!(recovered.status, TaskStatus::AwaitingMerge, "{output}");
    assert_eq!(
        recovered.deliverables.parked_branch.as_deref(),
        Some(BRANCH)
    );
    assert_eq!(
        recovered.deliverables.factory_branch_anchor.as_deref(),
        Some(rebased.as_str())
    );
    let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].bound_head, rebased);
    assert_eq!(rounds[0].branch, BRANCH);
}

/// cas-915b: releasing the implementation lease does not release its delivery
/// from containment. A lane-CI citation is not evidence of a target merge.
#[tokio::test]
async fn released_request_changes_delivery_still_requires_merge_cas_915b() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let branch = crate::factory_isolation::worker_task_branch(WORKER, TASK);
    git(&f.worker_path, &["checkout", "-q", "-b", &branch]);
    let cas_dir = f.dir.path().join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    task.demo_statement.clear();
    tasks.update(&task).unwrap();
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "supervisor");
    supervisor
        .cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest {
            id: TASK.into(),
            reason: "Correct the declined delivery before re-parking".into(),
        }))
        .await
        .unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    f.core
        .cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
            id: TASK.into(),
            brief: Some(true),
            force: None,
        }))
        .await
        .unwrap();
    // Model the normal commit-hook capture during the new implementation cycle.
    let mut corrected = tasks.get(TASK).unwrap();
    assert_eq!(corrected.status, TaskStatus::InProgress);
    corrected.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
    corrected.deliverables.parked_branch = Some(branch.clone());
    tasks.update(&corrected).unwrap();
    f.core
        .cas_task_release(Parameters(crate::mcp::tools::TaskReleaseRequest {
            task_id: TASK.into(),
            force: None,
        }))
        .await
        .unwrap();
    assert!(tasks.get(TASK).unwrap().assignee.is_none());

    std::fs::write(f.worker_path.join("note.txt"), "final corrected delivery\n").unwrap();
    git(&f.worker_path, &["add", "note.txt"]);
    git(
        &f.worker_path,
        &[
            "commit",
            "-q",
            "-m",
            "fix(cas-c6e1-fixture): final correction",
        ],
    );
    let final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(
        f.dir.path(),
        &[
            "update-ref",
            &format!("refs/remotes/origin/{branch}"),
            &final_tip,
        ],
    );
    assert!(!commit_is_merged_into_parent(
        f.dir.path(),
        &final_tip,
        "main"
    ));
    let output = text(f.core.cas_task_close(Parameters(TaskCloseRequest {
        no_delivery: None,
        id: TASK.into(),
        reason: Some(format!("Lane CI passed for {final_tip}: https://github.com/example/cassy/actions/runs/39049693968 . Re-parking for epic merge.")),
        commit_receipt: None,
        supervisor_override: None,
        stranded_branch_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
    })).await.unwrap());
    let parked = tasks.get(TASK).unwrap();
    assert!(output.contains("MERGE REQUIRED"), "{output}");
    assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
    assert_eq!(
        parked.deliverables.parked_branch.as_deref(),
        Some(branch.as_str())
    );
    assert_eq!(
        parked.deliverables.factory_branch_anchor.as_deref(),
        Some(final_tip.as_str())
    );

    // Once the actual delivery is integrated, the released task can close.
    git(
        f.dir.path(),
        &[
            "merge",
            "-q",
            "--no-ff",
            &branch,
            "-m",
            "integrate corrected delivery",
        ],
    );
    git(
        f.dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let mut user_facing = tasks.get(TASK).unwrap();
    user_facing.demo_statement = "Read the corrected status".into();
    tasks.update(&user_facing).unwrap();
    let qa_output = text(
        f.core
            .cas_task_close(Parameters(TaskCloseRequest {
                no_delivery: None,
                id: TASK.into(),
                reason: Some("Delivery integrated; review still required".into()),
                commit_receipt: Some(final_tip.clone()),
                supervisor_override: None,
                stranded_branch_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
            }))
            .await
            .unwrap(),
    );
    assert!(qa_output.contains("INDEPENDENT QA REQUIRED"), "{qa_output}");
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge);
    let mut task = tasks.get(TASK).unwrap();
    task.demo_statement.clear();
    tasks.update(&task).unwrap();
    let output = text(
        f.core
            .cas_task_close(Parameters(TaskCloseRequest {
                no_delivery: None,
                id: TASK.into(),
                reason: Some("Final corrected delivery merged into target".into()),
                commit_receipt: Some(final_tip),
                supervisor_override: None,
                stranded_branch_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
            }))
            .await
            .unwrap(),
    );
    assert_eq!(
        tasks.get(TASK).unwrap().status,
        TaskStatus::Closed,
        "{output}"
    );
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
    let history = (0..12_000)
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
    assert!(task.notes.len() > 1_800_000, "{}", task.notes.len());
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

fn fresh_ledger(f: &Recovery) -> std::path::PathBuf {
    let cas_dir = f.dir.path().join(".cas");
    let paths = crate::config::resolved_factory_artifact_paths(&cas_dir, None);
    let task_dir = paths.task_dirs(TASK)[0].clone();
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("LEDGER.md"),
        "| M01 | render contract | ok | ok | PASS | real-build | qa/M01.txt | - |\n",
    )
    .unwrap();
    task_dir
}

#[tokio::test]
async fn backend_renderer_log_only_close_parks_and_dispatches_qa_cas_2de9() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "src/contracts/pdf.renderer.ts", true);
    let task_dir = fresh_ledger(&f);
    let parked = close(&f).await;
    assert_eq!(
        parked.deliverables.factory_branch_anchor.as_deref(),
        Some(f.final_tip.as_str())
    );
    assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
    let cas_dir = f.dir.path().join(".cas");
    let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].bound_head, f.final_tip);
    // Simulate the independent reviewer through the real store boundary, so
    // re-close tests implementer evidence without a supervisor evidence waiver.
    cas_store::claim_qa_pass(&cas_dir, TASK, "independent-reviewer", chrono::Utc::now()).unwrap();
    cas_store::resolve_qa_pass(
        &cas_dir,
        TASK,
        "independent-reviewer",
        cas_types::QaVerdict::Approved,
        "fixture independent review",
        None,
        task_dir.join("LEDGER.md").to_str().unwrap(),
        chrono::Utc::now(),
    )
    .unwrap();
    git(
        f.dir.path(),
        &[
            "merge",
            "-q",
            "--no-ff",
            BRANCH,
            "-m",
            "Integrate reviewed delivery",
        ],
    );
    git(
        f.dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let output = close_output(&f).await;
    assert_eq!(
        open_task_store(&cas_dir).unwrap().get(TASK).unwrap().status,
        TaskStatus::Closed,
        "{output}"
    );
}

#[tokio::test]
async fn terminal_surfaces_keep_receipt_gate_before_park_cas_2de9() {
    let mut env = TestEnvGuard::temp_home();
    for surface in [
        "src/cli/status.rs",
        "src/tui/status.rs",
        "src/doctor.rs",
        "crates/cas-mux/src/render.rs",
        "crates/ghostty_vt/src/lib.rs",
        "crates/ghostty_vt_sys/src/lib.rs",
        "crates/cas-diffs/src/widget.rs",
    ] {
        let f = fixture_with_surface(&mut env, surface, true);
        let task_dir = fresh_ledger(&f);
        let output = close_output(&f).await;
        assert!(
            output.contains("terminal-qa receipt"),
            "{surface}: {output}"
        );
        let cas_dir = f.dir.path().join(".cas");
        let task = open_task_store(&cas_dir).unwrap().get(TASK).unwrap();
        assert_eq!(task.status, TaskStatus::InProgress, "{surface}: {output}");
        assert!(
            cas_store::list_qa_passes(&cas_dir, TASK)
                .unwrap()
                .is_empty()
        );
        std::fs::create_dir_all(task_dir.join("terminal-qa")).unwrap();
        std::fs::write(
            task_dir.join("terminal-qa/report.md"),
            "terminal-qa: PASS fixture\n",
        )
        .unwrap();
        close(&f).await;
    }
}

#[tokio::test]
async fn exact_terminal_evidence_waiver_still_records_park_cas_2de9() {
    for (surface, has_demo, complete_pairs) in [
        ("src/cli/status.rs", true, false),
        ("note.txt", false, false),
        ("note.txt", false, true),
    ] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture_with_surface(&mut env, surface, true);
        let cas_dir = f.dir.path().join(".cas");
        let task_dir = fresh_ledger(&f);
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        if !has_demo {
            task.demo_statement.clear();
            tasks.update(&task).unwrap();
        }
        let original_notes = task.notes.clone();
        let terminal = task_dir.join("terminal-qa/report.md");
        std::fs::create_dir_all(terminal.parent().unwrap()).unwrap();
        std::fs::write(&terminal, "terminal-qa: PASS fixture\n").unwrap();
        let unrelated = task_dir.join("unrelated.json");
        std::fs::write(&unrelated, r#"{"files":{"cells":["M01.png"]}}"#).unwrap();
        let prose = task_dir.join("prose.txt");
        std::fs::write(&prose, "old bundle instructions, not a manifest").unwrap();
        for cited in [
            "not-a-path".to_string(),
            task_dir.join("missing.json").display().to_string(),
            unrelated.display().to_string(),
            prose.display().to_string(),
        ] {
            task.notes = format!("qa-bundle: {cited}");
            let result = super::super::qa_evidence_gate::qa_evidence_close_gate_for_paths(
                &cas_dir,
                &task,
                f.dir.path(),
                "main",
                Some(&f.final_tip),
                Some(&[surface.into()]),
                None,
            );
            assert!(
                result.is_ok(),
                "{surface} without declared pairs must ignore {cited}: {result:?}"
            );
        }
        task.notes = original_notes;
        tasks.update(&task).unwrap();
        std::fs::remove_file(terminal).unwrap();
        // A capture side in the current files.pairs schema: PNG and JPEG,
        // each with the SHA-256 of its bytes.
        let capture = |name: &str, contents: &str| {
            use sha2::{Digest, Sha256};
            for ext in ["png", "jpg"] {
                std::fs::write(task_dir.join(format!("{name}.{ext}")), contents).unwrap();
            }
            let digest = format!("{:x}", Sha256::digest(contents.as_bytes()));
            serde_json::json!({"png": format!("{name}.png"), "jpg": format!("{name}.jpg"),
                "sha256": digest, "jpg_sha256": digest})
        };
        let before = capture("before", "baseline");
        let after = complete_pairs.then(|| capture("after", "current"));
        std::fs::write(
            task_dir.join("pairs.json"),
            serde_json::json!({
                "schema": 1, "task_id": TASK, "head_sha": f.final_tip,
                "pairs": if complete_pairs { serde_json::json!([{
                    "id": "G3", "before": before, "after": after,
                }]) } else { serde_json::json!([]) },
                "unpaired": if complete_pairs { serde_json::json!([]) }
                    else { serde_json::json!([{"id": "G3", "before": before}]) },
            })
            .to_string(),
        )
        .unwrap();
        let manifest = task_dir.join("bundle.json");
        std::fs::write(
            &manifest,
            serde_json::json!({
                "head_sha": f.final_tip, "files": {"pairs": "pairs.json"},
                // The bundle states what the pairs file holds: a BEFORE-only
                // cell is a partial comparison, a matched one complete.
                "pairs_status": if complete_pairs { "complete" } else { "partial" },
            })
            .to_string(),
        )
        .unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.notes
            .push_str(&format!("\nqa-bundle: {}", manifest.display()));
        tasks.update(&task).unwrap();
        if !complete_pairs {
            let refusal = close_output(&f).await;
            assert!(refusal.contains("missing BEFORE cell(s): G3"), "{refusal}");
        }
        cas_store::waive_qa_pass(
            &cas_dir,
            TASK,
            "supervisor-session",
            WORKER,
            BRANCH,
            &f.final_tip,
            "fixture exact-head terminal evidence waiver",
            chrono::Utc::now(),
        )
        .unwrap();
        let parked = close(&f).await;
        assert_eq!(
            parked.deliverables.factory_branch_anchor.as_deref(),
            Some(f.final_tip.as_str())
        );
        assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
        let waiver_label = if has_demo { "ledger" } else { "pair coverage" };
        assert_eq!(
            parked
                .notes
                .contains(&format!("QA evidence {waiver_label} waived")),
            !complete_pairs,
            "complete coverage must not record an unused waiver: {}", parked.notes,
        );
        let passes = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].bound_head, f.final_tip);
        assert_eq!(passes[0].state, cas_types::QaPassState::Waived);
    }
}

#[tokio::test]
async fn cas_5dc0_repark_opens_next_qa_round_and_reuses_the_idle_reviewer() {
    for busy in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let mut f = fixture(&mut env);
        let cas_dir = f.dir.path().join(".cas");
        let agents = open_agent_store(&cas_dir).unwrap();
        env.set("CAS_FACTORY_SESSION", "qa-reround-fixture");
        for mut agent in agents.list(None).unwrap() {
            agent.factory_session = Some("qa-reround-fixture".into());
            agents.update(&agent).unwrap();
        }
        let mut reviewer = Agent::new_with_role(
            "qa-review-session".into(),
            "qa-reviewer".into(),
            AgentRole::Worker,
        );
        reviewer.status = cas_types::AgentStatus::Idle;
        reviewer.factory_session = Some("qa-reround-fixture".into());
        agents.register(&reviewer).unwrap();
        close(&f).await;
        let first = cas_store::latest_qa_pass(&cas_dir, TASK, chrono::Utc::now())
            .unwrap()
            .unwrap();
        cas_store::claim_qa_pass(&cas_dir, TASK, &reviewer.name, chrono::Utc::now()).unwrap();
        cas_store::resolve_qa_pass(
            &cas_dir,
            TASK,
            &reviewer.name,
            cas_types::QaVerdict::Rejected,
            "Correct the spacing",
            None,
            "fixture/LEDGER.md",
            chrono::Utc::now(),
        )
        .unwrap();
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut qa_task = tasks.get(first.qa_task_id.as_deref().unwrap()).unwrap();
        qa_task.assignee = Some(reviewer.name.clone());
        qa_task.status = TaskStatus::Closed;
        tasks.update(&qa_task).unwrap();
        if busy {
            let mut other = Task::new("cas-review-other".into(), "Other accepted review".into());
            other.status = TaskStatus::InProgress;
            other.assignee = Some(reviewer.name.clone());
            tasks.add(&other).unwrap();
            agents
                .try_claim(&other.id, &reviewer.id, 1800, Some("unrelated review"))
                .unwrap();
        }
        let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
        supervisor.set_agent_id_for_testing("supervisor-session".into());
        env.set("CAS_AGENT_ROLE", "supervisor");
        supervisor
            .cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest {
                id: TASK.into(),
                reason: "Address the independent QA rejection".into(),
            }))
            .await
            .unwrap();
        std::fs::write(f.worker_path.join("footer.css"), ".footer { gap: 12px; }\n").unwrap();
        git(&f.worker_path, &["add", "footer.css"]);
        git(
            &f.worker_path,
            &[
                "commit",
                "-qm",
                "fix(cas-c6e1-fixture): address independent QA",
            ],
        );
        f.final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        git(
            f.dir.path(),
            &[
                "update-ref",
                &format!("refs/remotes/origin/{BRANCH}"),
                &f.final_tip,
            ],
        );
        env.set("CAS_AGENT_ROLE", "worker");
        f.core
            .cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
                id: TASK.into(),
                brief: Some(true),
                force: None,
            }))
            .await
            .unwrap();
        close(&f).await;
        let passes = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
        assert_eq!(
            passes.len(),
            2,
            "re-park must open round2 without manual qa_request"
        );
        let next = passes.iter().find(|pass| pass.round == 2).unwrap();
        assert_eq!(next.bound_head, f.final_tip);
        let assigned = tasks.get(next.qa_task_id.as_deref().unwrap()).unwrap();
        assert_eq!(
            assigned.assignee.as_deref(),
            (!busy).then_some(reviewer.name.as_str()),
            "reuse only an available independent reviewer"
        );
        assert_eq!(
            next.state,
            cas_types::QaPassState::Pending,
            "reviewer must still accept and claim the new round"
        );
        let queued = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .poll_for_target_with_session(&reviewer.name, Some("qa-reround-fixture"), 100)
            .unwrap();
        assert_eq!(
            queued.iter().any(|row| row.prompt.contains(&assigned.id)),
            !busy,
            "only the idle reused reviewer receives the new assignment"
        );
        let prompts = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .poll_for_target_with_session("supervisor", Some("qa-reround-fixture"), 100)
            .unwrap();
        let handoff = prompts
            .iter()
            .find(|row| row.source == format!("qa-dispatch:{}", next.id))
            .unwrap();
        // pin: assigned-reviewer handoffs must not instruct a duplicate reviewer spawn.
        let instructs_spawn = handoff.prompt.contains("Spawn a reviewer");
        assert_eq!(
            instructs_spawn, busy,
            "handoff must match actual reviewer assignment"
        );
    }
}

#[tokio::test]
async fn cas_5dc0_merged_redelivery_has_a_dispatch_even_when_content_refuses_close() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let cas_dir = f.dir.path().join(".cas");
    let mut config = crate::config::Config::load(&cas_dir).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&cas_dir).unwrap();
    // CasCore caches config for its lifetime: enable verification before
    // the first close, so the merged close exercises dispatch preparation.
    assert!(f.core.load_config().verification_enabled());
    close(&f).await;
    let old = cas_store::create_verification_dispatch_bound(
        &cas_dir,
        TASK,
        "recovery-session",
        "supervisor-session",
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    cas_store::resolve_verification_dispatch_with_conn(
        &conn,
        &old.id,
        "supervisor-session",
        None,
        true,
    )
    .unwrap();
    let invalidated = cas_store::invalidate_verification_dispatch_for_new_cycle(&cas_dir, TASK)
        .unwrap()
        .unwrap();
    assert_eq!(invalidated.id, old.id);
    assert_eq!(
        invalidated.state,
        cas_types::VerificationDispatchState::Invalidated
    );
    // A merge fact is retained even when its tree discarded the delivery.
    // The integration gate must still refuse; a verifier must still be able to report.
    git(
        f.dir.path(),
        &[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            BRANCH,
            "-m",
            "merge redelivery with missing source",
        ],
    );
    std::fs::write(f.dir.path().join("footer.css"), ".footer { gap: 12px; }\n").unwrap();
    git(f.dir.path(), &["add", "footer.css"]);
    git(f.dir.path(), &["commit", "-qm", "reviewed replacement"]);
    let replacement = git(f.dir.path(), &["rev-parse", "HEAD"]);
    let output = close_output(&f).await;
    assert!(output.contains("DELIVERY CONTENT DROPPED"), "{output}");
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, TASK)
        .unwrap()
        .unwrap();
    assert_ne!(
        dispatch.id, old.id,
        "merged re-delivery needs a new exact boundary before content refusal"
    );
    assert_eq!(
        dispatch.state,
        cas_types::VerificationDispatchState::Pending
    );
    let proof = dispatch
        .repository
        .as_ref()
        .expect("bind the merged target, never infer an unbound pass");
    assert_eq!(proof.head_commit, git(f.dir.path(), &["rev-parse", "main"]));
    assert_eq!(proof.target_branch.as_deref(), Some("main"));
    let repeated = close_output(&f).await;
    assert!(repeated.contains("DELIVERY CONTENT DROPPED"), "{repeated}");
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, TASK)
            .unwrap()
            .unwrap()
            .id,
        dispatch.id,
        "retry must keep the same exact dispatch"
    );
    assert!(proof.anchor_commits.contains(&f.final_tip));
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "supervisor");
    let reviewed_close = |reason: String| supervisor.cas_task_close(Parameters(TaskCloseRequest {
        no_delivery: None,
        id: TASK.into(),
        reason: Some(reason),
        commit_receipt: Some(f.final_tip.clone()),
        supervisor_override: Some(true),
        stranded_branch_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
    }));
    let invalid = text(reviewed_close(format!("reviewed-drop: {} -- not a replacement", f.final_tip)).await.unwrap());
    assert!(invalid.contains("DELIVERY REVIEW REJECTED") && invalid.contains("must differ from anchor"), "{invalid}");
    let reason = format!("reviewed-drop: {replacement} -- replace footer; keep the \"reviewed\" spacing.");
    let pending = text(reviewed_close(reason.clone()).await.unwrap());
    assert!(pending.contains("VERIFICATION REQUIRED") && !pending.contains("DELIVERY CONTENT DROPPED"), "{pending}");
    supervisor.cas_verification_add(Parameters(serde_json::from_value(serde_json::json!({
        "task_id":TASK,"dispatch_id":dispatch.id,"status":"approved","summary":"Independent review recorded; integration proof remains a separate close obligation"
    })).unwrap())).await.expect("the legitimate independent verdict must be recordable");
    let after_verdict = close_output(&f).await;
    assert!(
        after_verdict.contains("DELIVERY CONTENT DROPPED"),
        "{after_verdict}"
    );
    assert_ne!(
        open_task_store(&cas_dir).unwrap().get(TASK).unwrap().status,
        TaskStatus::Closed,
        "a dispatch never waives the content gate"
    );
    let reviewed = text(reviewed_close(reason).await.unwrap());
    let closed = open_task_store(&cas_dir).unwrap().get(TASK).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed, "{reviewed}");
    assert!(closed.notes.contains(&replacement) && closed.notes.contains("reviewed delivery content supersession accepted"), "{}", closed.notes);

}
