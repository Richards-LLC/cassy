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
        .filter_map(|content| match content {
            rmcp::model::ContentBlock::Text(text) => Some(text.text),
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
                search_manifest: None,
            }))
            .await
            .unwrap(),
    )
}

async fn close(fixture: &Recovery) -> Task {
    let output = close_output(fixture).await;

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
            assert!(output.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "API-only delivery must park: {output}");
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
/// commit; close must bind the pushed GREEN tip.
#[tokio::test]
async fn shared_lane_red_green_close_checks_pushed_tip_cas_2bec() {
    let mut env = TestEnvGuard::temp_home();
    for (subject, explicit_receipt, advances) in [
        (format!("fix({TASK}): GREEN implementation"), false, true),
        ("test: correct pre-push failure".into(), false, false),
        ("test: correct pre-push failure".into(), true, true),
        (
            format!("fix(cas-abcd): sibling work\n\nContext: {TASK}"),
            true,
            false,
        ),
    ] {
        let f = fixture_with_surface(&mut env, "note.txt", false);
        let repo = f.dir.path();
        git(&f.worker_path, &["reset", "--hard", "main"]);
        std::fs::write(f.worker_path.join("regression.txt"), "RED regression\n").unwrap();
        git(&f.worker_path, &["add", "regression.txt"]);
        git(
            &f.worker_path,
            &["commit", "-qm", &format!("test({TASK}): RED regression")],
        );
        let red = git(&f.worker_path, &["rev-parse", "HEAD"]);
        // cas-93d7: pre-push refuses the first delivery before origin updates.
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(
            repo,
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        let hook = repo.join(".git/hooks/pre-push");
        std::fs::write(&hook, "#!/bin/sh\ngit cat-file -e HEAD:fix.txt\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let refused = std::process::Command::new("git")
            .args(["push", "origin", BRANCH])
            .current_dir(&f.worker_path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(!refused.status.success(), "pre-push must reject RED");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("fix.txt"),
            "{refused:?}"
        );
        assert!(git(repo, &["ls-remote", "--heads", "origin"]).is_empty());
        std::fs::write(f.worker_path.join("fix.txt"), "GREEN fix\n").unwrap();
        git(&f.worker_path, &["add", "fix.txt"]);
        git(&f.worker_path, &["commit", "-qm", &subject]);
        let green = git(&f.worker_path, &["rev-parse", "HEAD"]);
        git(&f.worker_path, &["push", "-q", "origin", BRANCH]);

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
                    commit_receipt: explicit_receipt.then(|| green.clone()),
                    no_delivery: None,
                    supervisor_override: None,
                    stranded_branch_override: None,
                    search_manifest: None,
                }))
                .await
                .unwrap(),
        );
        let parked = tasks.get(TASK).unwrap();
        if !advances {
            assert_eq!(
                parked.deliverables.factory_branch_anchor.as_deref(),
                Some(red.as_str()),
                "must keep the old anchor for {subject:?}, explicit={explicit_receipt}: {output}"
            );
            continue;
        }

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
async fn resume_release_sync_accepts_shipped_but_refuses_unshipped_foreign_commits_cas_ef54() {
    for shipped_by in [Some("origin/main"), Some("v9.99.0"), None] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture_with_surface(&mut env, "note.txt", false);
        let repo = f.dir.path();
        let root = repo.join(".cas");
        let agents = open_agent_store(&root).unwrap();
        let mut worker = agents.get("recovery-session").unwrap();
        worker.metadata.insert("clone_path".into(), f.worker_path.display().to_string());
        agents.update(&worker).unwrap();
        git(repo, &["branch", "epic/sync", "main"]);
        std::fs::write(repo.join("release.txt"), "foreign task delivery\n").unwrap();
        git(repo, &["add", "release.txt"]);
        git(repo, &["commit", "-qm", "fix(cas-beef): foreign task delivery"]);
        let foreign = git(repo, &["rev-parse", "HEAD"]);
        match shipped_by {
            Some("origin/main") => { git(repo, &["update-ref", "refs/remotes/origin/main", &foreign]); }
            Some(tag) => { git(repo, &["tag", tag, &foreign]); }
            None => {
                git(repo, &["update-ref", "-d", "refs/remotes/origin/main"]);
                git(repo, &["tag", "v-scratch", &foreign]);
            }
        }
        git(&f.worker_path, &["merge", "-q", "--no-ff", "main", "-m",
            &format!("chore({TASK}): synchronize release")]);
        let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        let tasks = open_task_store(&root).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.deliverables.work_target.as_mut().unwrap().target_branch = "epic/sync".into();
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(), branch: BRANCH.into(),
            base_sha: git(repo, &["rev-parse", "epic/sync"]),
        });
        let boundary = task.deliverables.factory_branch_start.clone();
        task.deliverables.parked_branch = Some(BRANCH.into());
        task.deliverables.factory_branch_anchor = Some(tip.clone());
        task.status = TaskStatus::AwaitingMerge;
        tasks.update(&task).unwrap();
        let supervisor = CasCore::with_daemon(root.clone(), None, None);
        supervisor.set_agent_id_for_testing("supervisor-session".into());
        env.set("CAS_AGENT_ROLE", "supervisor");
        supervisor.cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
            id: TASK.into(), reason: "Keep the sync and rerun the named test".into(),
        })).await.unwrap();
        env.set("CAS_AGENT_ROLE", "worker");
        let result = f.core.cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
            id: TASK.into(), brief: Some(true), force: None,
        })).await;
        let allowed = shipped_by == Some("origin/main");
        if allowed {
            result.unwrap_or_else(|error| panic!("shipped by {shipped_by:?}: {error}"));
        } else {
            assert_eq!(result.unwrap_err().code, ErrorCode::INVALID_PARAMS);
        }
        let resumed = tasks.get(TASK).unwrap();
        assert_eq!(resumed.status, if allowed { TaskStatus::InProgress } else { TaskStatus::Open });
        assert_eq!(resumed.deliverables.factory_branch_start, boundary);
        assert_eq!(git(&f.worker_path, &["rev-parse", "HEAD"]), tip);
        assert_eq!(std::fs::read(f.worker_path.join("release.txt")).unwrap(), b"foreign task delivery\n");
    }
}

/// cas-bc25, replaying cas-afd2 on Nov 12: CI named two failing tests, the
/// first repair brief named only those, and a third test of their class failed
/// next. With the failing tests recorded as data, a decline needs the class.
#[tokio::test]
async fn request_changes_over_recorded_failing_tests_requires_a_class_audit_cas_bc25() {
    use std::io::Write;
    use crate::ui::factory::daemon::runtime::{ci_watch, merge_queue_ejection};
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    // Lane attribution applies to train deliveries too, not just PRs to main.
    git(repo, &["branch", "epic/class-audit", "main"]);
    task.deliverables.work_target.as_mut().unwrap().target_branch = "epic/class-audit".into();
    task.deliverables.parked_branch = Some(BRANCH.into());
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(),
        branch: BRANCH.into(),
        base_sha: git(repo, &["rev-parse", "main"]),
    });
    task.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
    task.status = TaskStatus::AwaitingMerge;
    tasks.update(&task).unwrap();
    // Replay the real CI watcher against a disposable GitHub transport. Logs
    // include test failures, but only structured, head-bound caused receipts
    // may create the class-audit fact (cas-0eb4).
    git(repo, &["remote", "add", "origin", "https://github.com/example/class-audit.git"]);
    let gh = root.join("gh");
    let runs = serde_json::json!({"workflow_runs":[{
        "id":91,"name":"CI","event":"push","head_branch":BRANCH,
        "head_sha":f.final_tip,"html_url":"https://example.invalid/run/91",
        "status":"completed","conclusion":"failure"
    }]});
    for (name, value) in [
        ("runs.json", runs),
        ("branch.json", serde_json::json!({"commit":{"sha":f.final_tip}})),
        ("artifacts.json", serde_json::json!({"artifacts":[{
            "id":901,"name":"lane-impact-91-own","size_in_bytes":4096,"expired":false
        }]})),
    ] { std::fs::write(root.join(name), serde_json::to_vec(&value).unwrap()).unwrap(); }
    let script = format!(r#"#!/bin/sh
case "$*" in
  'repo view example/class-audit '*) printf '%s' '{{"nameWithOwner":"example/class-audit"}}' ;;
  *'/actions/runs/91/artifacts'*) cat '{root}/artifacts.json' ;;
  *'/actions/artifacts/'*'/zip'*) cat '{root}/receipt.zip' ;;
  *'/actions/runs/91/jobs'*) printf '%s' '{{"jobs":[{{"name":"Scoped Validation — impact own tests","conclusion":"failure"}}]}}' ;;
  *'/branches/'*) cat '{root}/branch.json' ;;
  *'/actions/runs'*) cat '{root}/runs.json' ;;
  'api graphql '*) printf '%s' '{{"data":{{"repository":{{"pullRequests":{{"nodes":[]}}}}}}}}' ;;
  'pr list '*) printf '[]' ;;
  'run view 91 '*) printf 'test log_only::decoy ... FAILED\nLane impact: caused by this delivery: log_only::decoy\n' ;;
  *) printf 'Unexpected gh request: %s\n' "$*" >&2; exit 97 ;;
esac
"#, root=root.display());
    crate::test_paths::warm_stub(&gh, &script);
    env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
    // Consume the producer's output so this seam cannot invent a lane schema.
    let producer = root.join("ci-test-impact.py");
    std::fs::write(&producer, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/ci-test-impact.py"))).unwrap();
    let lane_path = root.join("lane.json");
    let output = std::process::Command::new("python3")
        .arg(producer)
        .args(["lane", "--phase", "plan", "--base-sha", &f.predecessor, "--out"])
        .arg(&lane_path)
        .current_dir(&f.worker_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "lane plan: {}", String::from_utf8_lossy(&output.stderr));
    let lane: serde_json::Value = serde_json::from_slice(&std::fs::read(lane_path).unwrap()).unwrap();
    assert_eq!(lane["plan"]["head"], f.final_tip);
    let write_receipt = |head: &str, caused: bool, artifact_id: u64| {
        std::fs::write(root.join("artifacts.json"), serde_json::to_vec(&serde_json::json!({"artifacts":[{
            "id":artifact_id,"name":"lane-impact-91-own","size_in_bytes":4096,"expired":false
        }]})).unwrap()).unwrap();
        let file = std::fs::File::create(root.join("receipt.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let mut lane = lane.clone();
        if head != f.final_tip {
            lane["plan"]["head"] = head.into();
        }
        for (name, data) in [
            ("lane.json", lane),
            ("own-attribution.json", serde_json::json!({
                "base":"base","status":if caused {"caused"} else {"inherited"},
                "caused":if caused {serde_json::json!([
                    {"binary":"cas","test":"ui::list::sorts"},
                    {"binary":"cas","test":"ui::list::filters"}
                ])} else {serde_json::json!([])},
                "inherited":[{"binary":"cas","test":"inherited::decoy"}]
            })),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&serde_json::to_vec(&data).unwrap()).unwrap();
        }
        zip.finish().unwrap();
    };
    let poll = || {
        ci_watch::read_github(&root, repo, "class-audit-session",
            &std::collections::BTreeSet::from([BRANCH.to_string()]),
            &Default::default(), &Default::default(), &Default::default(),
            &merge_queue_ejection::FailedRunLookups::default()).unwrap()
    };
    // Inherited-only receipts and stale-head receipts do not trigger, even
    // when log text appears to attribute a named failure to this delivery.
    for (head, caused, artifact_id) in [(f.final_tip.as_str(), false, 901), (f.predecessor.as_str(), true, 902)] {
        write_receipt(head, caused, artifact_id);
        assert_eq!(poll().failures.len(), 1);
        assert!(tasks.get_execution_state(TASK).unwrap().is_none_or(|state|
            state.get("failing_tests").is_none()));
    }
    write_receipt(&f.final_tip, true, 903);
    assert_eq!(poll().failures.len(), 1);
    assert_eq!(tasks.get_execution_state(TASK).unwrap().unwrap_or_default()["failing_tests"],
        serde_json::json!(["ui::list::sorts", "ui::list::filters"]));
    // The merge-queue producer uses this same sink. Its distinct name and a
    // repeated lane name must survive another lane poll without duplicates.
    crate::mcp::tools::core::task::repair_class::record_failing_tests(&root, TASK,
        &["ui::grid::renders".into(), "ui::list::sorts".into()]);
    let expected = serde_json::json!(["ui::list::sorts", "ui::list::filters", "ui::grid::renders"]);
    assert_eq!(tasks.get_execution_state(TASK).unwrap().unwrap()["failing_tests"], expected);
    assert_eq!(poll().failures.len(), 1);
    assert_eq!(tasks.get_execution_state(TASK).unwrap().unwrap()["failing_tests"], expected);
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "supervisor");
    let request = |class: Option<&str>| {
        Parameters(crate::mcp::tools::TaskRequestChangesRequest {
            id: TASK.into(),
            reason: "Keep the commits; fix the failing tests".into(),
            class_audit: class.map(String::from),
        })
    };
    // The first brief: the two tests, no class. A class that misses them is no better.
    // Also refused: an all-exact class just restates the failing names, and `&` is a
    // class no receipt could cover (coverage understands `|` only).
    for class in [
        None,
        Some("not a filter"),
        Some("test(gates::)"),
        Some("test(ui::list::)"),
        Some("test(=ui::list::sorts) | test(=ui::list::filters)"),
        Some("test(ui::list::) & test(sorts)"),
    ] {
        let refused = supervisor.cas_task_request_changes(request(class)).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::INVALID_PARAMS);
        assert!(refused.message.contains("CLASS_AUDIT_REQUIRED"), "{refused}");
        assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge);
    }
    // cas-adca: decode the structural field and enter the real spawn handler,
    // rather than calling the class helper with an already-recorded answer.
    // This store-level recovery fixture needs the peer outbox used by spawn routing.
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    for sql in crate::migration::MIGRATIONS.iter().find(|m| m.id == 268).unwrap().up {
        conn.execute_batch(sql).unwrap();
    }
    env.remove("CAS_FACTORY_SESSION");
    let mut repair = tasks.get(TASK).unwrap();
    repair.status = TaskStatus::Open;
    repair.assignee = None;
    tasks.update(&repair).unwrap();
    let service = crate::mcp::CasService::new(
        supervisor.clone(),
        #[cfg(feature = "mcp-proxy")]
        None,
    );
    let spawn = |class: Option<&str>| serde_json::from_value(serde_json::json!({
        "action": "spawn_workers", "count": 1, "cli": "claude", "force": true,
        "task_id": TASK, "spawn_brief": "Repair the recorded failure class.",
        "class_audit": class,
    })).unwrap();
    let queue = crate::store::open_spawn_queue_store(&root).unwrap();
    let refused = service.factory(Parameters(spawn(None))).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::INVALID_PARAMS);
    assert_eq!(queue.pending_count().unwrap(), 0);
    assert!(tasks.get_execution_state(TASK).unwrap().unwrap().get("class_audit").is_none());
    let accepted = service.factory(Parameters(spawn(Some("  test(ui::list::) | test(ui::grid::)  "))))
        .await.unwrap();
    assert_ne!(accepted.is_error, Some(true));
    let queued = queue.peek(10).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].task_id.as_deref(), Some(TASK));
    assert_eq!(tasks.get_execution_state(TASK).unwrap().unwrap()["class_audit"],
        "test(ui::list::) | test(ui::grid::)");
    repair.status = TaskStatus::AwaitingMerge;
    tasks.update(&repair).unwrap();
    supervisor.cas_task_request_changes(request(Some("test(ui::list::) | test(ui::grid::)"))).await.unwrap();
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Open);
    let state = tasks.get_execution_state(TASK).unwrap().unwrap();
    assert_eq!(state["class_audit"], "test(ui::list::) | test(ui::grid::)");
    // A repeat decline that omits the class keeps the stored one: the park obligation survives.
    let mut parked = tasks.get(TASK).unwrap();
    parked.status = TaskStatus::AwaitingMerge;
    tasks.update(&parked).unwrap();
    supervisor.cas_task_request_changes(request(None)).await.unwrap();
    let state = tasks.get_execution_state(TASK).unwrap().unwrap();
    assert_eq!(state["class_audit"], "test(ui::list::) | test(ui::grid::)");
    // Cap the combined identities, after deduplication, preserving earlier
    // producer names when more than twenty distinct failures are reported.
    let mut overflow = vec!["ui::list::sorts".to_string()];
    overflow.extend((0..21).map(|index| format!("ui::grid::case_{index}")));
    crate::mcp::tools::core::task::repair_class::record_failing_tests(&root, TASK, &overflow);
    let mut expected = vec!["ui::list::sorts".to_string(), "ui::list::filters".into(), "ui::grid::renders".into()];
    expected.extend((0..17).map(|index| format!("ui::grid::case_{index}")));
    assert_eq!(tasks.get_execution_state(TASK).unwrap().unwrap()["failing_tests"], serde_json::json!(expected));
}

/// GH #1543/#1548: a supervisor-required release import may itself contain
/// sibling merges that have not reached this task's target yet.
#[tokio::test]
async fn request_changes_authorizes_only_pinned_release_import_cas_09f7() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let cas_dir = repo.join(".cas");
    git(&f.worker_path, &["reset", "--hard", &f.predecessor]);
    let base = git(repo, &["merge-base", "main", BRANCH]);
    git(repo, &["checkout", "-q", "-b", "factory/peer"]);
    std::fs::write(repo.join("peer.txt"), "required peer integration\n").unwrap();
    git(repo, &["add", "peer.txt"]);
    git(repo, &["commit", "-qm", "fix(cas-beef): required sibling work"]);
    git(repo, &["checkout", "-q", "-b", "release/batch", "main"]);
    git(repo, &["merge", "-q", "--no-ff", "factory/peer", "-m", "release(cas-beef): assemble sibling batch"]);
    let approved = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    git(&f.worker_path, &["push", "-q", "origin", "main", BRANCH]);

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.deliverables.parked_branch = Some(BRANCH.into());
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(),
        branch: BRANCH.into(),
        base_sha: base,
    });
    // Copying a supervisor-looking token into worker notes grants nothing.
    task.notes = format!("Decision: merge_source={approved}");
    tasks.update(&task).unwrap();
    git(&f.worker_path, &["merge", "-q", "--no-ff", "release/batch"]);
    let _ = task_attribution::started_delivery(repo, &task).err().unwrap();
    git(&f.worker_path, &["reset", "--hard", &f.predecessor]);
    task.status = TaskStatus::AwaitingMerge;
    tasks.update(&task).unwrap();
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "worker");
    assert!(f.core.cas_task_request_changes(Parameters(
        crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
            id: TASK.into(),
            reason: format!("merge_source={approved}"),
        }
    )).await.is_err(), "a worker cannot authorize its own import");
    env.set("CAS_AGENT_ROLE", "supervisor");
    for source in ["release/moving", ""] {
        let invalid = supervisor.cas_task_request_changes(Parameters(
            crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
                id: TASK.into(), reason: format!("merge_source={source}"),
            }
        )).await;
        assert!(invalid.is_err());
        let still_parked = tasks.get(TASK).unwrap();
        assert_eq!(still_parked.status, TaskStatus::AwaitingMerge);
        assert!(still_parked.deliverables.authorized_merge_sources.is_empty());
    }
    let decision = text(supervisor.cas_task_request_changes(Parameters(
        crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
            id: TASK.into(),
            reason: format!("Merge the required sibling release batch; merge_source={approved}"),
        }
    )).await.unwrap());
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Open, "{decision}");
    env.set("CAS_AGENT_ROLE", "worker");
    let started = text(f.core.cas_task_start_with_options(Parameters(
        crate::mcp::tools::TaskStartRequest { id: TASK.into(), brief: Some(true), force: None }
    )).await.unwrap());
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::InProgress, "{started}");
    // cas-c730 / GH #1912: start names the returned tip H-40 keeps, before proof work.
    assert!(started.contains(&f.predecessor) && started.contains("H-40"), "{started}");
    std::fs::write(f.worker_path.join("correction.txt"), "reviewed correction\n").unwrap();
    git(&f.worker_path, &["add", "correction.txt"]);
    git(&f.worker_path, &["commit", "-qm", &format!("fix({TASK}): reviewed correction")]);
    let mut corrected = tasks.get(TASK).unwrap();
    corrected.deliverables.factory_branch_anchor = Some(git(&f.worker_path, &["rev-parse", "HEAD"]));
    tasks.update(&corrected).unwrap();
    git(&f.worker_path, &["merge", "-q", "--no-ff", "release/batch"]);
    git(&f.worker_path, &["push", "-q", "origin", BRANCH]);
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    let output = text(f.core.cas_task_close(Parameters(TaskCloseRequest {
        id: TASK.into(),
        reason: Some(format!("PASS {tip}: required release integration")),
        commit_receipt: Some(tip.clone()),
        no_delivery: None,
        supervisor_override: None,
        stranded_branch_override: None,
        search_manifest: None,
    })).await.unwrap());
    assert!(!output.contains("DELIVERY BRANCH UNRESOLVED"), "{output}");
    let parked = tasks.get(TASK).unwrap();
    assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
    assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()));
    // Later changes on the approved branch are not approved by that decision.
    git(repo, &["checkout", "-q", "release/batch"]);
    std::fs::write(repo.join("unapproved.txt"), "unrelated later work\n").unwrap();
    git(repo, &["add", "unapproved.txt"]);
    git(repo, &["commit", "-qm", "fix(cas-dead): unapproved later work"]);
    git(&f.worker_path, &["merge", "-q", "--no-ff", "release/batch"]);
    let mut continued = parked;
    continued.status = TaskStatus::InProgress;
    let _ = task_attribution::started_delivery(repo, &continued).err().unwrap();
}

/// GH #1785: the active integration must get its exact parent approval
/// before the foreign-history gate will let it park for the first time.
#[tokio::test]
async fn first_park_import_grant_uses_syncing_store_cas_a7b5() {
    let mut env = TestEnvGuard::temp_home();
    for initial_status in [TaskStatus::InProgress, TaskStatus::Blocked] {
    let mut f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let cas_dir = repo.join(".cas");
    let server = wiremock::MockServer::start().await;
    crate::cloud::CloudConfig {
        endpoint: server.uri(), token: Some("fixture-token".into()), ..Default::default()
    }.save_to_cas_dir(&cas_dir).unwrap();
    // Open through the production factory: a logged-in project wraps SQLite
    // in the real syncing store, rather than testing the raw backend alone.
    let tasks = open_task_store(&cas_dir).unwrap();
    let queue = crate::cloud::SyncQueue::open(&cas_dir).unwrap();
    git(&f.worker_path, &["reset", "--hard", &f.predecessor]);
    git(repo, &["switch", "-qc", "epic/approved", "main"]);
    std::fs::write(repo.join("child.txt"), "integrated sibling\n").unwrap();
    git(repo, &["add", "child.txt"]);
    git(repo, &["commit", "-qm", "fix(cas-beef): sibling delivery"]);
    let child = git(repo, &["rev-parse", "HEAD"]);
    // cas-8144 adopts the granted child and its ancestors. Keep real foreign
    // work after that child so the partial grant still cannot own this import.
    std::fs::write(repo.join("second-child.txt"), "another integrated sibling\n").unwrap();
    git(repo, &["add", "second-child.txt"]);
    git(repo, &["commit", "-qm", "fix(cas-cafe): second sibling delivery"]);
    git(repo, &["commit", "-qm", "Assemble approved epic", "--allow-empty"]);
    let approved = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["switch", "-q", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    git(&f.worker_path, &["push", "-q", "origin", "main", BRANCH]);
    git(&f.worker_path, &["merge", "-q", "--no-ff", &approved]);
    f.final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);

    let mut task = tasks.get(TASK).unwrap();
    task.status = initial_status;
    task.deliverables.parked_branch = None;
    task.deliverables.factory_branch_anchor = None;
    task.deliverables.historical_factory_branch_anchors.clear();
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(),
        branch: BRANCH.into(),
        base_sha: f.predecessor.clone(),
    });
    task.notes = format!("Prose alone has no authority: merge_source={approved}");
    tasks.update(&task).unwrap();
    assert!(task_attribution::started_delivery(repo, &task).is_err());
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let request = |sha: &str| Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
        id: TASK.into(),
        reason: format!("Reviewed epic import merge_source={sha}"),
    });

    // Even a supervisor environment cannot turn a registered worker into
    // the grant authority.
    env.set("CAS_AGENT_ROLE", "supervisor");
    assert!(f.core.cas_task_request_changes(request(&approved)).await.is_err());
    assert!(tasks.get(TASK).unwrap().deliverables.authorized_merge_sources.is_empty());
    queue.clear().unwrap();
    supervisor.cas_task_request_changes(request(&child)).await.unwrap();
    let wrong = tasks.get(TASK).unwrap();
    assert_eq!(wrong.status, initial_status);
    assert_eq!(wrong.deliverables.factory_branch_start, task.deliverables.factory_branch_start);
    assert_eq!(wrong.deliverables.factory_branch_anchor, task.deliverables.factory_branch_anchor);
    assert!(task_attribution::started_delivery(repo, &wrong).is_err());
    supervisor.cas_task_request_changes(request(&child)).await.unwrap();
    assert_eq!(tasks.get(TASK).unwrap().updated_at, wrong.updated_at);

    supervisor.cas_task_request_changes(request(&approved)).await.unwrap();
    let granted = tasks.get(TASK).unwrap();
    assert_eq!(granted.status, initial_status);
    assert_eq!(granted.assignee, task.assignee);
    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    let outbound: Task = serde_json::from_str(pending[0].payload.as_deref().unwrap()).unwrap();
    assert_eq!(outbound.deliverables.authorized_merge_sources, granted.deliverables.authorized_merge_sources);
    assert!(tasks.update_if_current(&task, &task).is_err());
    assert_eq!(tasks.get(TASK).unwrap().updated_at, granted.updated_at);
    assert_eq!(queue.pending(10, 5).unwrap()[0].payload, pending[0].payload);
    let delivery = task_attribution::started_delivery(repo, &granted).unwrap().unwrap();
    assert_eq!(delivery.commits, vec![f.final_tip.clone()]);

    // cas-8144 permits unnamed descendants of an approved import. A moved
    // parent that adds another task's work must still require its own grant.
    git(repo, &["switch", "-qc", "epic/moved", &approved]);
    git(repo, &["commit", "-qm", "fix(cas-dead): unapproved advancement", "--allow-empty"]);
    git(&f.worker_path, &["reset", "--hard", &f.predecessor]);
    git(&f.worker_path, &["merge", "-q", "--no-ff", "epic/moved"]);
    assert!(task_attribution::started_delivery(repo, &granted).is_err());
    git(&f.worker_path, &["reset", "--hard", &f.final_tip]);

    env.set("CAS_AGENT_ROLE", "worker");
    let _ = close_output(&f).await;
    assert_eq!(tasks.get(TASK).unwrap().status, initial_status);
    git(&f.worker_path, &["push", "-q", "origin", BRANCH]);
    let parked = close(&f).await;
    assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(f.final_tip.as_str()));
    let window = resolve_task_commit_receipt_window(
        parked.created_at,
        &[],
        task_commit_identity_in_repo(repo, &parked, None).unwrap(),
    );
    let paths = task_attribution::paths(repo, "main", &window, Some(&f.final_tip)).unwrap();
    assert!(paths.iter().any(|path| path == "child.txt"));
    }
}

/// cas-8144: a follow-up task's lane fast-forwarded onto a commit whose
/// subject names the original task refused close and merge; only
/// cancel-and-recreate escaped. A logged-in supervisor's merge_source grant,
/// made through the real handler on the syncing store, adopts that commit.
/// Work stacked after the grant is still judged by its own subject.
#[tokio::test]
async fn merge_source_adopts_fast_forwarded_foreign_named_commit_cas_8144() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let cas_dir = repo.join(".cas");
    crate::cloud::CloudConfig {
        endpoint: "http://127.0.0.1:9".into(), token: Some("fixture-token".into()),
        team_id: Some("team-1".into()), ..Default::default()
    }.save_to_cas_dir(&cas_dir).unwrap();
    let tasks = open_task_store(&cas_dir).unwrap();
    git(&f.worker_path, &["reset", "-q", "--hard", "main"]);
    let base = git(repo, &["rev-parse", "main"]);
    std::fs::write(f.worker_path.join("fix.rs"), "fix\n").unwrap();
    git(&f.worker_path, &["add", "fix.rs"]);
    git(&f.worker_path, &["commit", "-qm", "fix(commander): repair the board (cas-e1db)"]);
    let moved = git(&f.worker_path, &["rev-parse", "HEAD"]);

    let mut task = tasks.get(TASK).unwrap();
    task.deliverables.parked_branch = None;
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base,
    });
    tasks.update(&task).unwrap();
    assert!(task_attribution::started_delivery(repo, &task).is_err());

    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    supervisor.cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
        id: TASK.into(),
        reason: format!("Follow-up delivers the original fix merge_source={moved}"),
    })).await.unwrap();
    let granted = tasks.get(TASK).unwrap();
    assert_eq!(granted.status, TaskStatus::InProgress);
    let delivery = task_attribution::started_delivery(repo, &granted).unwrap().unwrap();
    assert_eq!(delivery.commits, vec![moved.clone()]);
    let identity = task_commit_identity_in_repo(repo, &granted, None).unwrap();
    assert!(!task_attribution::commit_claims_another_task(repo, &moved, &identity));

    std::fs::write(f.worker_path.join("other.rs"), "other\n").unwrap();
    git(&f.worker_path, &["add", "other.rs"]);
    git(&f.worker_path, &["commit", "-qm", "fix(cas-dddd): unrelated delivery"]);
    assert!(task_attribution::started_delivery(repo, &granted).is_err());
}

/// GH #1558: an ordinary close after pushing a target merge must bind the
/// merged tip, without asking the worker for an explicit commit receipt.
#[tokio::test]
async fn fresh_close_after_pushed_target_merge_binds_tip_without_receipt_cas_d81b() {
    let mut env = TestEnvGuard::temp_home();
    for merge_target in [false, true] {
        let f = fixture(&mut env);
        let repo = f.dir.path();
        let cas_dir = repo.join(".cas");
        let tasks = open_task_store(&cas_dir).unwrap();
        git(&f.worker_path, &["reset", "--hard", &f.predecessor]);
        let mut task = tasks.get(TASK).unwrap();
        task.deliverables.parked_branch = None;
        task.deliverables.historical_factory_branch_anchors.clear();
        tasks.update(&task).unwrap();
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        git(&f.worker_path, &["push", "-q", "origin", "main", BRANCH]);
        if merge_target {
            // The default merge subject carries no task id and introduces
            // only target commits after the commit-hook predecessor anchor.
            git(&f.worker_path, &["merge", "-q", "--no-ff", "main"]);
            git(&f.worker_path, &["push", "-q", "origin", BRANCH]);
        }
        let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        assert_eq!(git(repo, &["rev-parse", &format!("origin/{BRANCH}")]), tip);
        let output = text(f.core.cas_task_close(Parameters(TaskCloseRequest {
            id: TASK.into(),
            reason: Some(format!("PASS {tip}: final pushed delivery")),
            commit_receipt: None,
            no_delivery: None,
            supervisor_override: None,
            stranded_branch_override: None,
            search_manifest: None,
        })).await.unwrap());
        let parked = tasks.get(TASK).unwrap();
        assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
        assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH));
        assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()), "{output}");
        let state = tasks.get_execution_state(TASK).unwrap().unwrap();
        assert_eq!(state["qa_impact"]["head_sha"].as_str(), Some(tip.as_str()));
        let rounds = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].bound_head, tip);
        if merge_target {
            assert!(parked.deliverables.historical_factory_branch_anchors.contains(&f.predecessor));
        }
    }
}

/// cas-03738: cas-195e started on its lane, pushed a throwaway proof branch
/// under its per-task name, switched back to the lane and closed. Close must
/// park the checked-out recorded lane, never the per-task proof branch.
#[tokio::test]
async fn close_parks_checked_out_start_lane_over_pushed_proof_branch_cas_03738() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    git(&f.worker_path, &["reset", "--hard", "main"]);
    let base = git(&f.worker_path, &["rev-parse", "HEAD"]);
    std::fs::write(f.worker_path.join("note.txt"), "delivered\n").unwrap();
    git(&f.worker_path, &["add", "note.txt"]);
    git(&f.worker_path, &["commit", "-qm", &format!("fix({TASK}): deliver note")]);
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    let proof = crate::factory_isolation::worker_task_branch(WORKER, TASK);
    git(&f.worker_path, &["switch", "-qc", &proof]);
    std::fs::write(f.worker_path.join("proof.txt"), "proof only\n").unwrap();
    git(&f.worker_path, &["add", "proof.txt"]);
    git(&f.worker_path, &["commit", "-qm", &format!("chore({TASK}): proof-only version bump")]);
    git(&f.worker_path, &["switch", "-q", BRANCH]);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    git(&f.worker_path, &["push", "-q", "origin", "main", BRANCH, &proof]);
    let mut task = tasks.get(TASK).unwrap();
    task.deliverables.parked_branch = None;
    task.deliverables.factory_branch_anchor = Some(tip.clone());
    task.deliverables.historical_factory_branch_anchors.clear();
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base,
    });
    tasks.update(&task).unwrap();
    let output = text(f.core.cas_task_close(Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some(format!("PASS {tip}: lane delivery")),
        commit_receipt: None, no_delivery: None, supervisor_override: None,
        stranded_branch_override: None, search_manifest: None,
    })).await.unwrap());
    let parked = tasks.get(TASK).unwrap();
    assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{output}");
    assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(BRANCH), "{output}");
    assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()), "{output}");
}

/// GH #1571: generated history follows a target-sync merge, while the only
/// passing focused receipt belongs to the complete, pushed final tree.
#[tokio::test]
async fn close_after_target_merge_and_generated_history_checks_final_tip_cas_c7e3() {
    let mut env = TestEnvGuard::temp_home();
    for (per_task_branch, generated_only) in [(true, false), (true, true), (false, false), (false, true)] {
        let f = fixture_with_surface(&mut env, "cas-cli/src/lib.rs", false);
        let repo = f.dir.path();
        let cas_dir = repo.join(".cas");
        let branch = if per_task_branch {
            let branch = crate::factory_isolation::worker_task_branch(WORKER, TASK);
            git(&f.worker_path, &["branch", "-m", &branch]);
            branch
        } else { BRANCH.to_string() };
        std::fs::write(f.worker_path.join("Cargo.toml"), "[workspace]\nmembers = [\"cas-cli\"]\n").unwrap();
        std::fs::write(f.worker_path.join("cas-cli/Cargo.toml"), "[package]\nname = \"cas\"\n").unwrap();
        git(&f.worker_path, &["add", "Cargo.toml", "cas-cli/Cargo.toml"]);
        git(&f.worker_path, &["commit", "--amend", "-q", "--no-edit"]);
        let merged = git(&f.worker_path, &["rev-parse", "HEAD"]);
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        // Resume the same parked lane; the target merge was recorded by its
        // commit hook before the required generated follow-up was committed.
        task.deliverables.parked_branch = Some(branch.clone());
        task.deliverables.factory_branch_anchor = Some(merged.clone());
        task.notes = "no changelog: fixture for final-tip proof selection".into();
        tasks.update(&task).unwrap();
        let ledger = f.worker_path.join(REFERENCE_HISTORY_LEDGER);
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(&ledger, "{}\n").unwrap();
        if !generated_only {
            // A source edit must not borrow the generator's subject/body.
            std::fs::write(f.worker_path.join("cas-cli/src/lib.rs"), "fn foreign() {}\n").unwrap();
        }
        git(&f.worker_path, &["add", "."]);
        git(&f.worker_path, &["commit", "-q", "-m",
            "build(builtins): regenerate reference-history from merged history", "-m",
            &format!("Rebuilt by scripts/regenerate-generated-artifacts.sh after merging {} (cas-fb11).", &merged[..9])]);
        let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(repo, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        git(&f.worker_path, &["push", "-q", "origin", "main", &branch]);
        let request = || TaskCloseRequest {
            id: TASK.into(), reason: Some(format!("PASS {tip}: final pushed delivery")),
            commit_receipt: None, no_delivery: None, supervisor_override: None,
            stranded_branch_override: None, search_manifest: None,
        };
        if generated_only {
            let refused = text(f.core.cas_task_close(Parameters(request())).await.unwrap());
            assert!(refused.contains("FOCUSED PROOF REQUIRED") && refused.contains(&tip[..12]),
                "a generated follow-up still requires proof at the final tip: {refused}");
            assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::InProgress);
        }
        crate::factory_worker_check::record_passing_test_for_tests(
            &cas_dir, &f.worker_path, &tip, "cas", "test(final_delivery)",
        );
        let output = text(f.core.cas_task_close(Parameters(request())).await.unwrap());
        let parked = tasks.get(TASK).unwrap();
        if !generated_only {
            assert_eq!(parked.status, TaskStatus::InProgress, "source edits cannot borrow regeneration authority: {output}");
            assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(merged.as_str()));
            continue;
        }
        assert_eq!(parked.status, TaskStatus::AwaitingMerge, "final tip has focused proof; merge parent does not: {output}");
        assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()), "{output}");
        let state = tasks.get_execution_state(TASK).unwrap().unwrap();
        assert_eq!(state["qa_impact"]["head_sha"].as_str(), Some(tip.as_str()));
    }
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
            search_manifest: None,
        })).await.unwrap();
        let output = text(result);
        let parked = tasks.get(TASK).unwrap();
        if rewrite {
            assert!(output.contains("rewrit") && !output.contains("base is no longer an ancestor"), "{output}");
            assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(f.predecessor.as_str()));
            continue;
        }

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
        crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
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
    // cas-cff4: the assignee is told once, with the decision and branch handling.
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let notices = queue.peek_for_targets(&[WORKER], None, 10).unwrap();
    assert_eq!(notices.len(), 1, "{verdict}");
    // pin: the supervisor must see which prompt carried the decision, never an implied handoff.
    assert!(verdict.contains(&format!("Assignee {WORKER} notified (prompt {})", notices[0].id)), "{verdict}");
    // pin: the worker notice must carry the supervisor's decision text verbatim.
    assert!(notices[0].prompt.contains("Rebase the reviewed correction"), "{}", notices[0].prompt);
    assert!(notices[0].prompt.contains(OLD_BRANCH), "{}", notices[0].prompt);
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
                search_manifest: None,
            }))
            .await
            .unwrap(),
    );

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
        .cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
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
        search_manifest: None,
    })).await.unwrap());
    let parked = tasks.get(TASK).unwrap();

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

    assert!(short_output.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "{short_output}");
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
    std::fs::create_dir_all(task_dir.join("qa")).unwrap();
    std::fs::write(task_dir.join("qa/M01.txt"), "captured output").unwrap();
    std::fs::write(task_dir.join("LEDGER.md"), format!(
        "cells=4; PASS=1; FAIL=0; NOT EXERCISED=3\nlabels: real-build=1; source-inferred=3\n{}\n\
         | M01 | render | ok | ok | PASS | real-build | qa/M01.txt | - |\n\
         | M02 | invalid | refused | not run | NOT EXERCISED: not run in this fixture | source-inferred | - | - |\n\
         | M03 | boundary | bounded | not run | NOT EXERCISED: not run in this fixture | source-inferred | - | - |\n\
         | M04 | adjacent | stable | not run | NOT EXERCISED: not run in this fixture | source-inferred | - | - |\n",
        crate::qa_ledger::HEADER
    )).unwrap();
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
            assert_eq!(open_task_store(&f.dir.path().join(".cas")).unwrap().get(TASK).unwrap().status, TaskStatus::InProgress, "{refusal}");
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
            .cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
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
        let start = || {
            f.core.cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
                id: TASK.into(),
                brief: Some(true),
                force: None,
            }))
        };
        // cas-cff4: the first start surfaces the queued request_changes notice.
        assert!(start().await.is_err(), "the worker must see the decision before restarting");
        start().await.unwrap();
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
async fn cas_5dc0_merged_redelivery_cannot_create_a_postmerge_dispatch() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let cas_dir = f.dir.path().join(".cas");
    let mut config = crate::config::Config::load(&cas_dir).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&cas_dir).unwrap();
    // CasCore caches config for its lifetime: enable verification before
    // the first close, so the merged close cannot skip verification implicitly.
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
    // The integration gate must still refuse without opening a post-merge review.
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
    assert_eq!(dispatch.id, old.id, "close cannot create a post-merge review");
    assert_eq!(dispatch.state, cas_types::VerificationDispatchState::Invalidated);
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
        search_manifest: None,
    }));
    let invalid = text(reviewed_close(format!("reviewed-drop: {} -- not a replacement", f.final_tip)).await.unwrap());
    assert!(invalid.contains("DELIVERY REVIEW REJECTED") && invalid.contains("must differ from anchor"), "{invalid}");
    let reason = format!("reviewed-drop: {replacement} -- replace footer; keep the \"reviewed\" spacing.");
    let reviewed = text(reviewed_close(reason).await.unwrap());
    // Existing explicit supervisor override authority remains audited; it adds
    // no post-merge dispatch and does not pretend an independent check ran.
    assert!(reviewed.contains("Closed task:"), "{reviewed}");
    let closed = open_task_store(&cas_dir).unwrap().get(TASK).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed);
    assert!(closed.notes.contains(&replacement) && closed.notes.contains("reviewed delivery content supersession accepted"), "{}", closed.notes);
    let retained = cas_store::get_latest_verification_dispatch(&cas_dir, TASK).unwrap().unwrap();
    assert_eq!(retained.id, old.id);
    assert_eq!(retained.state, cas_types::VerificationDispatchState::Invalidated);
    let ledger = cas_store::quality_events_for_task(&cas_dir, TASK).unwrap();
    assert!(ledger.iter().any(|event| matches!(&event.payload,
        cas_store::QualityPayload::Intervention { intervention: cas_store::InterventionKind::OverrideClose, .. })), "{ledger:?}");
}

/// Point the fixture task's recorded start at `base` on the worker lane,
/// with no parked or anchored delivery yet.
fn start_on_lane(tasks: &dyn cas_store::TaskStore, base: &str) -> Task {
    let mut task = tasks.get(TASK).unwrap();
    task.deliverables.parked_branch = None;
    task.deliverables.factory_branch_anchor = None;
    task.deliverables.historical_factory_branch_anchors.clear();
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base.into(),
    });
    tasks.update(&task).unwrap();
    task
}

fn commit_file(path: &Path, file: &str, subject: &str) -> String {
    std::fs::write(path.join(file), format!("{subject}\n")).unwrap();
    git(path, &["add", file]);
    git(path, &["commit", "-qm", subject]);
    git(path, &["rev-parse", "HEAD"])
}

/// GH #1944 (cas-f45f): the worker's own commit, recorded in this task's
/// delivery cycle when it was made, names the regression it fixes. Start and
/// close read it as this task's work; an unrecorded commit naming another
/// task still refuses.
#[tokio::test]
async fn recorded_own_commit_naming_another_task_stays_this_delivery_gh1944() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let repo = f.dir.path();
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    git(&f.worker_path, &["reset", "-q", "--hard", "main"]);
    let base = git(repo, &["rev-parse", "main"]);
    let mut task = start_on_lane(tasks.as_ref(), &base);
    // Each commit-hook anchor write appends to the open delivery cycle.
    for (file, subject) in [
        ("a.rs", "fix(post-chat): answer the hook question (cas-c6e1-fixture)"),
        ("b.rs", "fix(post-chat): a vague answer still answers the hook question (cas-7ecf)"),
        ("c.rs", "test(post-chat): cover the vague answer (cas-c6e1-fixture)"),
    ] {
        task.deliverables.factory_branch_anchor = Some(commit_file(&f.worker_path, file, subject));
        tasks.update(&task).unwrap();
    }
    let task = tasks.get(TASK).unwrap();
    let delivery = task_attribution::started_delivery(repo, &task).unwrap().unwrap();
    assert_eq!(delivery.commits.len(), 3, "{:?}", delivery.commits);
    task_commit_identity_in_repo(repo, &task, None).unwrap();

    // GH #2043: recycling may reopen the task. Its recorded commits and
    // original start must survive the real restart path without rewriting Git.
    let boundary = task.deliverables.factory_branch_start.clone();
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    let mut reopened = task.clone();
    reopened.status = TaskStatus::Open;
    tasks.update(&reopened).unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    let restarted = text(f.core.cas_task_start_with_options(Parameters(
        crate::mcp::tools::TaskStartRequest { id: TASK.into(), brief: Some(true), force: None },
    )).await.unwrap());
    let resumed = tasks.get(TASK).unwrap();
    assert_eq!(resumed.status, TaskStatus::InProgress, "{restarted}");
    assert_eq!(resumed.deliverables.factory_branch_start, boundary);
    assert_eq!(git(&f.worker_path, &["rev-parse", "HEAD"]), tip);

    commit_file(&f.worker_path, "d.rs", "fix(cas-dddd): unrecorded other task");
    assert!(task_attribution::started_delivery(repo, &task).is_err());
}

/// GH #1988 (cas-f45f): a replacement lane imports the task's reviewed
/// hotfix with `--no-ff`. One imported commit is another task's reused red
/// test, which the supervisor granted. The rebind's delivery read must find
/// the merge, not drop it for that granted commit.
#[tokio::test]
async fn granted_commit_inside_no_ff_import_keeps_the_merge_gh1988() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let repo = f.dir.path();
    let cas_dir = repo.join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    let base = git(repo, &["rev-parse", "main"]);
    git(repo, &["switch", "-qc", "hotfix", "main"]);
    let reused = commit_file(repo, "red.rs", "test(cas-f4a2): replay the folded member");
    commit_file(repo, "fix.rs", "fix(cas-c6e1-fixture): judge own paths by net range");
    git(repo, &["switch", "-q", "main"]);
    git(&f.worker_path, &["reset", "-q", "--hard", "main"]);
    git(&f.worker_path, &["merge", "-q", "--no-ff", "hotfix", "-m", "Merge cas-c6e1-fixture repair"]);
    let merge = git(&f.worker_path, &["rev-parse", "HEAD"]);
    let task = start_on_lane(tasks.as_ref(), &base);
    assert!(task_attribution::started_delivery(repo, &task).is_err(), "ungranted import refuses");

    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    supervisor.cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest { class_audit: None,
        id: TASK.into(),
        reason: format!("Reuse the red test merge_source={reused}"),
    })).await.unwrap();
    let granted = tasks.get(TASK).unwrap();
    let delivery = task_attribution::started_delivery(repo, &granted).unwrap().unwrap();
    assert_eq!(delivery.commits, vec![merge.clone()]);
    // Exercise rebind itself, including its branch/checkout/lease fences.
    let replacement = format!("{BRANCH}-{TASK}-recovery");
    git(&f.worker_path, &["switch", "-qc", &replacement]);
    supervisor.rebind_task_branch(TASK, &replacement, "Approve the authorized merged repair").unwrap();
    let rebound = tasks.get(TASK).unwrap();
    assert_eq!(rebound.deliverables.factory_branch_start.unwrap().branch, replacement);
    assert_eq!(git(&f.worker_path, &["rev-parse", "HEAD"]), merge);
}

/// GH #1888 (cas-f45f): the returned task's start records the worker lane,
/// while task start cut an empty `factory/<worker>-<task>` ref at the target
/// tip and checked it out. A ref merely named for the task is not its
/// delivery: close checks measure the recorded lane.
#[tokio::test]
async fn empty_per_task_ref_does_not_replace_the_recorded_lane_gh1888() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let repo = f.dir.path();
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    let task = start_on_lane(tasks.as_ref(), &git(repo, &["rev-parse", "main"]));
    let per_task = crate::factory_isolation::worker_task_branch(WORKER, TASK);
    git(&f.worker_path, &["switch", "-qc", &per_task, "main"]);
    assert_eq!(close_measured_factory_branch(repo, &task, WORKER), BRANCH);
}

/// GH #1962: a resumed delivery owns its implementation between a temporary
/// journey edit and its revert; the final tree owes no proof for that journey.
#[tokio::test]
async fn resumed_retargeted_delivery_uses_complete_net_scope_cas_ded9() {
    async fn status(core: &CasCore, state: &str) {
        core.cas_task_update(Parameters(serde_json::from_value(serde_json::json!({
            "id": TASK, "status": state,
        })).unwrap())).await.unwrap();
    }
    async fn start(core: &CasCore) {
        core.cas_task_start_with_options(Parameters(crate::mcp::tools::TaskStartRequest {
            id: TASK.into(), brief: Some(true), force: None,
        })).await.unwrap();
    }
    let mut env = TestEnvGuard::temp_home();
    let mut f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path().to_path_buf();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let agents = open_agent_store(&root).unwrap();
    let mut worker = agents.get("recovery-session").unwrap();
    worker.metadata.insert("clone_path".into(), f.worker_path.display().to_string());
    agents.update(&worker).unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    let earlier = chrono::Utc::now() - chrono::Duration::hours(2);
    env.set("GIT_AUTHOR_DATE", earlier.to_rfc3339());
    env.set("GIT_COMMITTER_DATE", earlier.to_rfc3339());
    git(&f.worker_path, &["reset", "--hard", "main"]);
    let journey = "shared.journey.ts";
    let original = "const run = async () => {};\ntest('shared callback', run);\n";
    std::fs::write(f.worker_path.join(journey), original).unwrap();
    std::fs::create_dir_all(f.worker_path.join("changelog.d")).unwrap();
    std::fs::write(f.worker_path.join("changelog.d/README.md"), "Release fragments.\n").unwrap();
    git(&f.worker_path, &["add", "."]);
    git(&f.worker_path, &["commit", "-qm", "baseline shared callback"]);
    let base = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(&repo, &["reset", "--hard", &base]);
    git(&repo, &["branch", "epic/old", &base]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &base]);
    let mut task = tasks.get(TASK).unwrap();
    task.created_at = earlier - chrono::Duration::hours(1);
    task.depth = cas_types::TaskDepth::Light;
    task.demo_statement.clear();
    task.deliverables.factory_branch_anchor = None;
    task.deliverables.historical_factory_branch_anchors.clear();
    task.deliverables.parked_branch = Some(BRANCH.into());
    task.deliverables.work_target.as_mut().unwrap().target_branch = "epic/old".into();
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base.clone(),
    });
    // Creation time is immutable on update; seed the older task through add.
    tasks.delete(TASK).unwrap();
    tasks.add(&task).unwrap();
    std::fs::write(f.worker_path.join(journey), original.replace("shared callback", "temporary callback")).unwrap();
    git(&f.worker_path, &["add", journey]);
    git(&f.worker_path, &["commit", "-qm", &format!("test({TASK}): temporary journey edit")]);
    let edit = git(&f.worker_path, &["rev-parse", "HEAD"]);
    task.deliverables.factory_branch_anchor = Some(edit.clone());
    tasks.update(&task).unwrap();
    status(&f.core, "blocked").await;
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    env.set("CAS_AGENT_ROLE", "supervisor");
    // e5c1 rebinds empty sources; this nonempty retarget retains its audit.
    supervisor.cas_task_update_with_target(
        serde_json::from_value(serde_json::json!({"id": TASK})).unwrap(),
        None, Some("main"), true,
        Some("Retarget the resumed delivery to the current train"), None,
    ).await.unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    status(&f.core, "open").await;
    start(&f.core).await;
    let resumed = tasks.get(TASK).unwrap();
    assert_eq!(resumed.deliverables.factory_branch_start.as_ref().unwrap().work_target.target_branch, "epic/old");
    assert_eq!(resumed.deliverables.work_target.as_ref().unwrap().target_branch, "main");
    // A target-sync merge imports a sibling, splitting attribution ranges.
    std::fs::write(repo.join("sibling.py"), "sibling = True\n").unwrap();
    git(&repo, &["add", "sibling.py"]);
    git(&repo, &["commit", "-qm", "fix(cas-beef): sibling delivery"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(&f.worker_path, &["merge", "--no-ff", "main", "-m", "merge current target"]);
    std::fs::write(f.worker_path.join("parser.py"), "supported = True\n").unwrap();
    let fragment = format!("changelog.d/{TASK}.md");
    std::fs::write(f.worker_path.join(&fragment), "### Fixed — Checks\n\n- **Resume delivery checks** — Preserve the delivered parser correction.\n").unwrap();
    git(&f.worker_path, &["add", "parser.py", &fragment]);
    git(&f.worker_path, &["commit", "-qm", "fix(parser): retain implementation"]);
    // A source task named as context is not a foreign ownership claim.
    std::fs::write(f.worker_path.join(journey), original.replace("shared callback", "ported callback")).unwrap();
    git(&f.worker_path, &["add", journey]);
    git(&f.worker_path, &["commit", "-qm", "test(journeys): port barrier from cas-e074"]);
    status(&f.core, "blocked").await;
    status(&f.core, "open").await;
    start(&f.core).await;
    std::fs::write(f.worker_path.join(journey), original).unwrap();
    git(&f.worker_path, &["add", journey]);
    git(&f.worker_path, &["commit", "-qm", "test(journeys): restore the released journey"]);
    f.final_tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &f.final_tip]);
    // Match commit-time capture: retain the old checkpoint and record the
    // actual pushed delivery, rather than testing an unrelated stale anchor.
    let mut current = tasks.get(TASK).unwrap();
    current.deliverables.historical_factory_branch_anchors.push(edit);
    current.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
    tasks.update(&current).unwrap();
    let history = agents.get_lease_history(TASK, None).unwrap();
    // Durable-anchor fallback must cover the same full first-parent delivery
    // even when old installed metadata cannot validate the original start.
    let window = resolve_task_commit_receipt_window(current.created_at, &history,
        task_commit_identity(&current, Some(f.final_tip.clone())));
    let ranges = task_attribution::range_bounds(&repo, "main", &window, Some(&f.final_tip)).unwrap();
    let paths = task_attribution::paths(&repo, "main", &window, Some(&f.final_tip)).unwrap();
    let required = crate::factory_worker_check::stability::required(
        &repo, &f.final_tip, "main", &paths, Some(&ranges));
    assert!(paths.contains(&"parser.py".into()) && paths.contains(&fragment), "{ranges:?}: {paths:?}");
    assert!(!paths.contains(&"sibling.py".into()));
    assert!(required.unwrap().is_empty());
    let parked = close(&f).await;
    assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(f.final_tip.as_str()));
    // Surviving ambiguous callbacks still fail closed, unlike the reverted edit.
    std::fs::write(f.worker_path.join(journey), original.replace("shared callback", "actual callback")).unwrap();
    git(&f.worker_path, &["add", journey]);
    git(&f.worker_path, &["commit", "-qm", "test: actual changed callback"]);
    let ambiguous = git(&f.worker_path, &["rev-parse", "HEAD"]);
    assert!(crate::factory_worker_check::stability::required(&repo, &ambiguous, "main",
        &[journey.into()], Some(&[(base, ambiguous.clone())])).is_err());
    std::fs::write(f.worker_path.join("foreign.py"), "foreign = True\n").unwrap();
    git(&f.worker_path, &["add", "foreign.py"]);
    git(&f.worker_path, &["commit", "-qm", "fix(cas-beef): foreign delivery"]);
    let mut active = current;
    active.status = TaskStatus::InProgress;
    assert!(task_commit_identity_in_repo(&repo, &active, None).is_err());
}

/// GH #2041: a live supervisor's explicit stability decision applies at park,
/// before the later risk gates, without forging a passing runner receipt.
#[tokio::test]
async fn supervisor_stability_exception_reaches_park_gh2041() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    git(&f.worker_path, &["reset", "--hard", "main"]);
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    start_on_lane(tasks.as_ref(), &git(repo, &["rev-parse", "main"]));
    std::fs::write(f.worker_path.join("owned.test.js"), "test('custom config case', () => {});\n").unwrap();
    git(&f.worker_path, &["add", "owned.test.js"]);
    git(&f.worker_path, &["commit", "-qm", "test(cas-c6e1-fixture): custom config case"]);
    let tip = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(repo, &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &tip]);
    let request = |override_: bool| Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some("Reviewed stability exception for custom runner config".into()),
        commit_receipt: Some(tip.clone()), supervisor_override: Some(override_),
        no_delivery: None, stranded_branch_override: None, search_manifest: None,
    });
    let held = text(f.core.cas_task_close(request(false)).await.unwrap());
    assert!(crate::gates::codes_in(&held).contains(&"G-61"), "{held}");
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(repo.join(".cas"), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    assert!(supervisor.resolve_live_supervisor_authority().is_ok());
    let parked = text(supervisor.cas_task_close(request(true)).await.unwrap());
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge, "{parked}");
    assert!(tasks.get(TASK).unwrap().notes.contains("G-61"));
    assert!(crate::factory_worker_check::PassingTests::new(&repo.join(".cas"), &f.worker_path, &tip)
        .packages().is_empty());
}

/// pulse-card PR638: a worker anchor and a foreign test follow-up are already
/// integrated, but neither merge CI nor a supervisor's note is a review verdict.
#[tokio::test]
async fn contained_pr_accepts_authenticated_supervisor_review_cas_b73e() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "service.py", false);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let base = git(repo, &["rev-parse", "main"]);
    std::fs::write(f.worker_path.join("followup.test.py"), "assert True\n").unwrap();
    git(&f.worker_path, &["add", "followup.test.py"]);
    git(&f.worker_path, &["commit", "-qm", "test(cas-other): PR-only follow-up"]);
    let pr_head = git(&f.worker_path, &["rev-parse", "HEAD"]);
    git(repo, &["merge", "--no-ff", BRANCH, "-m", "Merge PR638"]);
    // The PR follow-up belongs to the integrated PR, not the parked worker
    // delivery. Keep the worker branch at its actual immutable receipt.
    git(&f.worker_path, &["reset", "--hard", &f.final_tip]);
    let merged = git(repo, &["rev-parse", "main"]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &merged]);
    let mut task = tasks.get(TASK).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
    task.deliverables.parked_branch = Some(BRANCH.into());
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base,
    });
    task.deliverables.delivery_pr_number = Some(638);
    task.notes = "no changelog: disposable recovery fixture; supervisor read the diff".into();
    tasks.update(&task).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
        .replace("enabled=false", "enabled=true")
        .replace("mode='per_task'", "mode='batch'")
        .replace("independent_pass=true", "independent_pass=false")).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let request = || Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some("Recover integrated PR delivery".into()),
        commit_receipt: Some(f.final_tip.clone()), supervisor_override: None,
        no_delivery: None, stranded_branch_override: None, search_manifest: None,
    });
    let held = text(supervisor.cas_task_close(request()).await.unwrap());
    assert!(crate::gates::codes_in(&held).contains(&"G-70"), "{held}");
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge);
    assert!(cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().is_none());
    let contained = supervisor.worktree_merge(
        BRANCH, false, Some(TASK), true, Some(false), false, None,
    ).await.unwrap();
    assert_ne!(contained.is_error, Some(true), "{contained:?}");
    assert_eq!(git(repo, &["rev-parse", "main"]), merged);
    assert!(cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().is_none());
    let review = || Parameters(serde_json::from_value(serde_json::json!({
        "task_id": TASK, "dispatch_id": null, "status": "approved", "summary": "Reviewed integrated PR delivery"
    })).unwrap());
    // A source that is merely parked cannot take this post-integration path.
    git(repo, &["update-ref", "refs/heads/main", &task.deliverables.factory_branch_start.as_ref().unwrap().base_sha]);
    git(repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    assert!(supervisor.cas_verification_add(review()).await.is_err());
    assert!(cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().is_none());
    git(repo, &["reset", "--hard", &merged]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &merged]);
    // A prior verdict is substantive review history, not a missing review.
    let verifications = supervisor.open_verification_store().unwrap();
    let mut prior = cas_types::Verification::new("ver-prior-review".into(), TASK.into());
    prior.status = VerificationStatus::Rejected;
    verifications.add(&prior).unwrap();
    assert!(supervisor.cas_verification_add(review()).await.is_err());
    assert!(cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().is_none());
    verifications.delete(&prior.id).unwrap();
    let recorded = supervisor.cas_verification_add(review()).await
        .expect("contained delivery must accept an authenticated review");
    assert_ne!(recorded.is_error, Some(true), "{recorded:?}");
    let dispatch = cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().unwrap();
    let proof = dispatch.repository.as_ref().unwrap();
    assert_eq!(proof.head_commit, merged);
    assert_eq!(proof.target_branch.as_deref(), Some("main"));
    assert_eq!(proof.anchor_commits, vec![f.final_tip.clone()]);
    git(repo, &["merge-base", "--is-ancestor", &pr_head, &proof.head_commit]);
    let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap().unwrap();
    assert_eq!(verdict.status, VerificationStatus::Approved);
    assert_eq!(verdict.provenance, cas_types::VerificationProvenance::SupervisorDirect);
    let closed = text(supervisor.cas_task_close(request()).await.unwrap());
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Closed, "{closed}");
}

/// GH #2282 (gabber, Nov 21): a delivery parked for `target` that later lands
/// there outside worktree_merge. Returns the fixture, the supervisor's core
/// and the task store after the park; `land` performs the landing.
async fn parked_for(env: &mut TestEnvGuard, target: &str) -> (Recovery, CasCore, std::sync::Arc<dyn cas_store::TaskStore>) {
    let f = fixture_with_surface(env, "service.py", false);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let base = git(repo, &["rev-parse", "main"]);
    // The target starts before the delivery's merge of main, as gabber's
    // develop, staging and epic branches did.
    git(repo, &["branch", target, &git(repo, &["rev-parse", "main~1"])]);
    git(repo, &["update-ref", &format!("refs/remotes/origin/{target}"), target]);
    let tasks = open_task_store(&root).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    let work_target = WorkTarget { repo_selector: "project:cas-c6e1-fixture-project".into(), target_branch: target.into() };
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.work_target = Some(work_target.clone());
    task.deliverables.factory_branch_anchor = Some(f.final_tip.clone());
    task.deliverables.parked_branch = Some(BRANCH.into());
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target, assignee: WORKER.into(), branch: BRANCH.into(), base_sha: base,
    });
    task.notes = "no changelog: disposable reconcile fixture".into();
    tasks.update(&task).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
        .replace("enabled=false", "enabled=true")
        .replace("mode='per_task'", "mode='batch'")
        .replace("independent_pass=true", "independent_pass=false")).unwrap();
    let agents = open_agent_store(&root).unwrap();
    for mut agent in agents.list(None).unwrap() {
        agent.factory_session = Some("reconcile-fixture".into());
        agents.update(&agent).unwrap();
    }
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(root, None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    (f, supervisor, tasks)
}

/// Merge `source` into `target` in the primary checkout and publish it.
fn merge_into(repo: &Path, target: &str, source: &str, message: &str) {
    git(repo, &["checkout", "-q", target]);
    git(repo, &["merge", "-q", "--no-ff", source, "-m", message]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["update-ref", &format!("refs/remotes/origin/{target}"), target]);
}

/// The reconcile pass dispatched a verifier on the contained delivery; its
/// approval closes the task with no per-task override.
async fn approve_dispatched(supervisor: &CasCore, tasks: &dyn cas_store::TaskStore, landed: &str) {
    let root = supervisor.cas_root.clone();
    let parked = tasks.get(TASK).unwrap();
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert_eq!(parked.deliverables.green_merge_commit.as_deref(), Some(landed));
    let dispatch = cas_store::get_latest_verification_dispatch(&root, TASK).unwrap()
        .expect("a contained delivery with no verdict gets a post-merge verifier");
    assert_eq!(dispatch.repository.as_ref().unwrap().anchor_commits, vec![parked.deliverables.factory_branch_anchor.clone().unwrap()]);
    let review = Parameters(serde_json::from_value(serde_json::json!({
        "task_id": TASK, "dispatch_id": dispatch.id, "status": "approved", "summary": "Reviewed the landed delivery"
    })).unwrap());
    let recorded = supervisor.cas_verification_add(review).await.unwrap();
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Closed, "{}", text(recorded));
}

#[tokio::test]
async fn pr_into_develop_reconciles_through_post_merge_verifier_gh2282() {
    let mut env = TestEnvGuard::temp_home();
    let (f, supervisor, tasks) = parked_for(&mut env, "develop").await;
    let repo = f.dir.path();
    merge_into(repo, "develop", BRANCH, "Merge pull request #41 from factory/recovered-worker");
    crate::factory_delivery_wake::reconcile_contained(&repo.join(".cas"), "reconcile-fixture").await.unwrap();
    approve_dispatched(&supervisor, tasks.as_ref(), &git(repo, &["rev-parse", "develop"])).await;
}

#[tokio::test]
async fn manual_epic_merge_with_approval_closes_on_reconcile_gh2282() {
    let mut env = TestEnvGuard::temp_home();
    let (f, supervisor, tasks) = parked_for(&mut env, "epic/gabber-train").await;
    let repo = f.dir.path();
    merge_into(repo, "epic/gabber-train", BRANCH, "merge recovered worker");
    let review = Parameters(serde_json::from_value(serde_json::json!({
        "task_id": TASK, "dispatch_id": null, "status": "approved", "summary": "Reviewed the integrated delivery"
    })).unwrap());
    let recorded = supervisor.cas_verification_add(review).await.unwrap();
    assert_ne!(recorded.is_error, Some(true), "{recorded:?}");
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::AwaitingMerge);
    // The approval alone never closes a delivery merged outside
    // worktree_merge; the reconcile pass's observed landing does.
    crate::factory_delivery_wake::reconcile_contained(&repo.join(".cas"), "reconcile-fixture").await.unwrap();
    let closed = tasks.get(TASK).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed, "{}", closed.notes);
    assert_eq!(closed.deliverables.green_merge_commit, Some(git(repo, &["rev-parse", "epic/gabber-train"])));
}

#[tokio::test]
async fn develop_to_staging_promotion_reconciles_only_once_contained_gh2282() {
    let mut env = TestEnvGuard::temp_home();
    let (f, supervisor, tasks) = parked_for(&mut env, "staging").await;
    let repo = f.dir.path();
    let root = repo.join(".cas");
    git(repo, &["branch", "develop", "staging"]);
    merge_into(repo, "develop", BRANCH, "Merge pull request #42 from factory/recovered-worker");
    // On develop but not yet on its staging target: G-70 still owns it.
    crate::factory_delivery_wake::reconcile_contained(&root, "reconcile-fixture").await.unwrap();
    let parked = tasks.get(TASK).unwrap();
    assert!(parked.deliverables.green_merge_commit.is_none());
    assert!(cas_store::get_latest_verification_dispatch(&root, TASK).unwrap().is_none());
    merge_into(repo, "staging", "develop", "Promote develop to staging");
    crate::factory_delivery_wake::reconcile_contained(&root, "reconcile-fixture").await.unwrap();
    approve_dispatched(&supervisor, tasks.as_ref(), &git(repo, &["rev-parse", "staging"])).await;
}

/// GH2240: a peer fast-forwarded a never-parked delivery into the target and
/// the worker's checkout is not on this machine. The supervisor's close and
/// its direct verdict must bind that delivery, never the supervisor's own
/// unrelated checkout HEAD, so the ordinary close retry succeeds.
#[tokio::test]
async fn supervisor_direct_verdict_binds_delivery_not_checkout_head_gh2240() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let base = git(repo, &["rev-parse", "main"]);
    let mut task = start_on_lane(tasks.as_ref(), &base);
    task.notes = "no changelog: disposable fixture".into();
    tasks.update(&task).unwrap();
    git(repo, &["worktree", "remove", "--force", f.worker_path.to_str().unwrap()]);
    git(repo, &["merge", "-q", "--ff-only", &f.final_tip]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &f.final_tip]);
    git(repo, &["checkout", "-q", "-b", "supervisor/startup"]);
    let checkout_head = commit_file(repo, "managed.txt", "chore: sync managed files");
    let config = root.join("config.toml");
    std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
        .replace("enabled=false", "enabled=true")
        .replace("independent_pass=true", "independent_pass=false")).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let close = |receipt: Option<String>| Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some("Landed by fast-forward; reviewed".into()),
        commit_receipt: receipt, supervisor_override: None,
        no_delivery: None, stranded_branch_override: None, search_manifest: None,
    });
    let pending = text(supervisor.cas_task_close(close(None)).await.unwrap());
    let dispatch = cas_store::get_latest_verification_dispatch(&root, TASK).unwrap()
        .unwrap_or_else(|| panic!("close must open a review cycle: {pending}"));
    let proof = dispatch.repository.as_ref().unwrap();
    assert_ne!(proof.head_commit, checkout_head, "{proof:?}");
    assert_eq!(proof.anchor_commits, vec![f.final_tip.clone()]);
    let review = Parameters(serde_json::from_value(serde_json::json!({
        "task_id": TASK, "dispatch_id": dispatch.id, "status": "approved", "summary": "Reviewed the landed delivery"
    })).unwrap());
    let recorded = supervisor.cas_verification_add(review).await.unwrap();
    assert_ne!(recorded.is_error, Some(true), "{recorded:?}");
    // The pre-close hook names the delivered commit, as GH2240's retry did.
    let closed = text(supervisor.cas_task_close(close(Some(f.final_tip.clone()))).await.unwrap());
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Closed, "{closed}");
}

/// cas-6c33 (GH2254, GH2280; cas-ff85's vdispatch-e726ab4b): a peer
/// machine's lane was never pushed here, the worker's checkout is elsewhere
/// and the supervisor's checkout sits on a stale `main`. The supervisor's
/// no-delivery close must bind the target tip, never that checkout HEAD.
#[tokio::test]
async fn peer_lane_no_delivery_close_binds_target_not_checkout_head_cas_6c33() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let target = git(repo, &["rev-parse", "main"]);
    let mut task = start_on_lane(tasks.as_ref(), &target);
    task.notes = "no changelog: disposable fixture".into();
    tasks.update(&task).unwrap();
    git(repo, &["worktree", "remove", "--force", f.worker_path.to_str().unwrap()]);
    git(repo, &["branch", "-q", "-D", BRANCH, OLD_BRANCH]);
    git(repo, &["update-ref", "-d", &format!("refs/remotes/origin/{BRANCH}")]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &target]);
    git(repo, &["reset", "-q", "--hard", "main~1"]);
    let checkout_head = git(repo, &["rev-parse", "HEAD"]);
    let config = root.join("config.toml");
    std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
        .replace("enabled=false", "enabled=true")
        .replace("independent_pass=true", "independent_pass=false")).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let pending = text(supervisor.cas_task_close(Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some("Already fixed on the target".into()),
        commit_receipt: None, supervisor_override: None,
        no_delivery: Some(true), stranded_branch_override: None, search_manifest: None,
    })).await.unwrap());
    let dispatch = cas_store::get_latest_verification_dispatch(&root, TASK).unwrap()
        .unwrap_or_else(|| panic!("close must open a review cycle: {pending}"));
    let proof = dispatch.repository.as_ref().unwrap();
    assert_ne!(proof.head_commit, checkout_head, "{proof:?}");
    assert_eq!(proof.head_commit, target, "{proof:?}");
}

/// cas-6c33 review: the peer pushed a later commit and it landed, but the
/// closer's cached view of the lane (its local branch and `origin/` ref) is
/// older. The review binds the refreshed pushed tip, never the stale cache.
#[tokio::test]
async fn peer_lane_review_binds_the_refreshed_pushed_tip_not_a_stale_cache_cas_6c33() {
    // A remote-only lane reaches the lane lookup already spelled `origin/<lane>`.
    for remote_only in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture_with_surface(&mut env, "note.txt", false);
        let repo = f.dir.path();
        let root = repo.join(".cas");
        let tasks = open_task_store(&root).unwrap();
        let base = git(repo, &["rev-parse", "main"]);
        let mut task = start_on_lane(tasks.as_ref(), &base);
        task.notes = "no changelog: disposable fixture".into();
        tasks.update(&task).unwrap();
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare"]);
        git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        let pushed = commit_file(&f.worker_path, "later.txt", "fix(cas-c6e1-fixture): later pushed commit");
        git(repo, &["merge", "-q", "--ff-only", &pushed]);
        git(repo, &["push", "-q", "origin", "main", &format!("{BRANCH}:{BRANCH}")]);
        git(repo, &["worktree", "remove", "--force", f.worker_path.to_str().unwrap()]);
        if remote_only {
            git(repo, &["branch", "-q", "-D", BRANCH]);
            assert_eq!(
                super::peer_lane_tip(repo, &format!("origin/{BRANCH}"), cas_types::DeliveryMode::PushBranch),
                Some(pushed.clone()),
            );
        } else {
            git(repo, &["branch", "-q", "-f", BRANCH, &f.final_tip]);
        }
        git(repo, &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &f.final_tip]);
        let config = root.join("config.toml");
        std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
            .replace("enabled=false", "enabled=true")
            .replace("independent_pass=true", "independent_pass=false")).unwrap();
        env.set("CAS_AGENT_ROLE", "supervisor");
        let supervisor = CasCore::with_daemon(root.clone(), None, None);
        supervisor.set_agent_id_for_testing("supervisor-session".into());
        let pending = text(supervisor.cas_task_close(Parameters(TaskCloseRequest {
            id: TASK.into(), reason: Some("Reviewed the peer delivery".into()),
            commit_receipt: None, supervisor_override: None,
            no_delivery: None, stranded_branch_override: None, search_manifest: None,
        })).await.unwrap());
        let dispatch = cas_store::get_latest_verification_dispatch(&root, TASK).unwrap()
            .unwrap_or_else(|| panic!("close must open a review cycle: {pending}"));
        let proof = dispatch.repository.as_ref().unwrap();
        assert_eq!(proof.anchor_commits, vec![pushed], "remote_only={remote_only}: {proof:?}");
    }
}

/// cas-6c33 (GH2280): an approved review recorded before the fix bound the
/// supervisor's unrelated checkout commit, while the peer's delivery merged.
/// The supervisor's recovery close with the merged receipt must retire that
/// review and close, never ask for the unrelated commit to be merged.
#[tokio::test]
async fn foreign_head_review_is_retired_not_merged_gh2280_cas_6c33() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture_with_surface(&mut env, "note.txt", false);
    let repo = f.dir.path();
    let root = repo.join(".cas");
    let tasks = open_task_store(&root).unwrap();
    let base = git(repo, &["rev-parse", "main"]);
    let mut task = start_on_lane(tasks.as_ref(), &base);
    task.notes = "no changelog: disposable fixture".into();
    tasks.update(&task).unwrap();
    git(repo, &["worktree", "remove", "--force", f.worker_path.to_str().unwrap()]);
    git(repo, &["merge", "-q", "--ff-only", &f.final_tip]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &f.final_tip]);
    git(repo, &["checkout", "-q", "-b", "supervisor/startup"]);
    let foreign = commit_file(repo, "settings.json", "chore: sync managed files");
    let bound = crate::mcp::tools::core::task::lifecycle::repository_proof::capture_repository_proof_with_anchors(
        repo, repo, vec![foreign.clone()],
    ).unwrap();
    let dispatch = cas_store::create_verification_dispatch_bound(
        &root, TASK, "supervisor-session", "supervisor-session",
        &cas_types::VerificationProofBoundary::task_at(bound),
        chrono::Utc::now() + chrono::Duration::minutes(10), false,
    ).unwrap();
    let mut verdict = cas_types::Verification::approved(
        "ver-foreign-head".into(), TASK.into(), "Reviewed the delivery note".into(),
    );
    verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
    verdict.agent_id = Some("supervisor-session".into());
    verdict.issuer_agent_id = verdict.agent_id.clone();
    verdict.dispatch_id = Some(dispatch.id.clone());
    use cas_store::VerificationStore;
    cas_store::SqliteVerificationStore::open(&root).unwrap().add(&verdict).unwrap();
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    cas_store::resolve_verification_dispatch_with_conn(
        &conn, &dispatch.id, "supervisor-session", None, true,
    ).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, std::fs::read_to_string(&config).unwrap()
        .replace("enabled=false", "enabled=true")
        .replace("independent_pass=true", "independent_pass=false")).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = CasCore::with_daemon(root.clone(), None, None);
    supervisor.set_agent_id_for_testing("supervisor-session".into());
    let closed = text(supervisor.cas_task_close(Parameters(TaskCloseRequest {
        id: TASK.into(), reason: Some("Reviewed the merged peer delivery".into()),
        commit_receipt: Some(f.final_tip.clone()), supervisor_override: Some(true),
        no_delivery: None, stranded_branch_override: None, search_manifest: None,
    })).await.unwrap());
    assert!(!crate::gates::codes_in(&closed).contains(&"G-65"), "{closed}");
    assert_eq!(tasks.get(TASK).unwrap().status, TaskStatus::Closed, "{closed}");
    assert_eq!(git(repo, &["rev-parse", "main"]), f.final_tip);
    assert_eq!(
        cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().state,
        cas_types::VerificationDispatchState::Invalidated,
    );
}

/// cas-7305: request_changes on an in-progress task (a pushed PR, no park) is
/// refused, and the refusal carries the exact calls that return the findings
/// to its worker: a decision note and a wake, each a replayable payload.
#[tokio::test]
async fn request_changes_on_in_progress_work_names_the_replayable_alternative_cas_7305() {
    use rmcp::handler::server::wrapper::Parameters;
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let root = tempfile::TempDir::new().unwrap();
    let core = CasCore::with_daemon(root.path().to_path_buf(), None, None);
    let tasks = core.open_task_store().unwrap();
    tasks.init().unwrap();
    let mut task = Task::new("cas-7305-open-pr".into(), "pushed, in progress".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("worker-a".into());
    tasks.add(&task).unwrap();
    let before = serde_json::to_value(tasks.get(&task.id).unwrap()).unwrap();

    let reason = "split the migration from the handler change";
    let refusal = core.cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest {
        class_audit: None, id: task.id.clone(), reason: reason.into(),
    })).await.expect_err("an in-progress task has no park to decline");
    assert_eq!(serde_json::to_value(tasks.get(&task.id).unwrap()).unwrap(), before);
    let payload = |tool: &str| -> serde_json::Value {
        let line = refusal.message.lines()
            .find_map(|line| line.strip_prefix(&format!("{tool} ")).filter(|rest| rest.starts_with('{')))
            .unwrap_or_else(|| panic!("no {tool} payload: {}", refusal.message));
        serde_json::from_str(line).unwrap()
    };
    let note = payload("task");
    assert_eq!((note["action"].as_str(), note["id"].as_str(), note["note_type"].as_str()),
        (Some("notes"), Some(task.id.as_str()), Some("decision")));
    assert!(note["notes"].as_str().unwrap().contains(reason));
    let wake = payload("coordination");
    assert_eq!((wake["action"].as_str(), wake["target"].as_str()), (Some("message"), Some("worker-a")));
    assert!(wake["message"].as_str().unwrap().contains(reason));
}
