//! Real delivery read → WorkerIdle prompt regressions; only GitHub is stubbed.
use super::*;
use cas_factory::ActiveLeaseSummary;
use cas_types::{Task, TaskStatus, WorkTarget};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(crate) struct Fixture {
    repo: tempfile::TempDir,
    remote: tempfile::TempDir,
    pub(crate) app: FactoryApp,
    pub(crate) event: DirectorEvent,
    _env: crate::test_support::TestEnvGuard,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let repo = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(repo.path(), &["init", "-q", "-b", "epic/test"]);
        git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        std::fs::write(repo.path().join("base.rs"), "// base\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "base"]);
        git(repo.path(), &["push", "-q", "origin", "epic/test"]);
        git(repo.path(), &["switch", "-qc", "factory/merge-w"]);
        std::fs::write(repo.path().join("delivery.rs"), "// delivery\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "cas-9efb-test delivery"]);
        git(repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
        // Git fetches a local bare server; GitHub queries retain an explicit origin.
        git(
            repo.path(),
            &[
                "config",
                &format!("url.{}.insteadOf", remote.path().display()),
                "https://github.com/acme/cas.git",
            ],
        );
        git(
            repo.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/acme/cas.git",
            ],
        );
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        env.set("CAS_ROOT", &cas_dir);
        let mut task = Task::new("cas-9efb-test".into(), "parked delivery".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("merge-w".into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "epic/test".into(),
        });
        crate::store::open_task_store_local(&cas_dir)
            .unwrap()
            .add(&task)
            .unwrap();
        let mut worker = cas_types::Agent::new("merge-w".into(), "merge-w".into());
        worker.role = cas_types::AgentRole::Worker;
        crate::store::open_agent_store(&cas_dir)
            .unwrap()
            .register(&worker)
            .unwrap();
        let mut data =
            DirectorData::load_with_stores_for_project(&cas_dir, None, false, None, None).unwrap();
        data.agent_id_to_name
            .insert("merge-w".into(), "merge-w".into());
        let mut app = FactoryApp::for_test_at(cas_dir.clone());
        app.set_director_data_for_test(data);
        let event = DirectorEvent::WorkerIdle {
            worker: "merge-w".into(),
            active_task: Some(ActiveLeaseSummary {
                task_id: task.id,
                task_title: task.title,
                task_status: task.status,
                close_rejected_reason: Some("MERGE REQUIRED".into()),
                pending_qa: None,
            }),
        };
        let gh = cas_dir.join("gh-fixture");
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{\"nameWithOwner\":\"acme/cas\"}'; else printf '%s\\n' \"$*\" >> .cas/gh-requests; cat .cas/ci.json; fi\n",
        );
        env.set(crate::github_issue_attach::GH_BIN_ENV, gh);
        let fixture = Self {
            repo,
            remote,
            app,
            event,
            _env: env,
        };
        fixture.ci(Some("success"));
        fixture
    }

    fn ci(&self, conclusion: Option<&str>) {
        std::fs::write(
            self.app.cas_dir.join("ci.json"),
            serde_json::json!({"check_runs": [{
                "name": "Scoped Validation (factory/PR)",
                "status": if conclusion.is_some() { "completed" } else { "in_progress" },
                "conclusion": conclusion,
                "html_url": "https://github.com/acme/cas/actions/runs/123",
            }]})
            .to_string(),
        )
        .unwrap();
    }

    fn relay(&mut self) -> Vec<Prompt> {
        self.app
            .revalidate_and_prompt_for_delivery(std::slice::from_ref(&self.event))
            .1
    }

    fn advance_target(&self, merge: bool) {
        let integration = tempfile::tempdir().unwrap();
        git(
            integration.path(),
            &[
                "clone",
                "-q",
                "-b",
                "epic/test",
                self.remote.path().to_str().unwrap(),
                ".",
            ],
        );
        if merge {
            git(
                integration.path(),
                &["merge", "-q", "--ff-only", "origin/factory/merge-w"],
            );
        } else {
            std::fs::write(integration.path().join("other.rs"), "// unrelated\n").unwrap();
            git(integration.path(), &["add", "."]);
            git(integration.path(), &["commit", "-qm", "other task"]);
        }
        git(integration.path(), &["push", "-q", "origin", "epic/test"]);
    }
}

#[test]
fn cas_9efb_pending_ci_suppresses_worker_idle_merge_relay() {
    let mut f = Fixture::new();
    f.ci(None);
    assert!(f.relay().is_empty(), "CI pending is not actionable");
    f.ci(Some("success"));
    assert_eq!(
        f.relay().len(),
        1,
        "pending must not consume the settled relay"
    );
}

#[test]
fn cas_9efb_live_target_suppresses_relay_with_origin_prefixed_target() {
    let mut f = Fixture::new();
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut task = store.get("cas-9efb-test").unwrap();
    task.deliverables
        .work_target
        .as_mut()
        .unwrap()
        .target_branch = "origin/epic/test".into();
    store.update(&task).unwrap();
    f.advance_target(true);
    assert!(
        f.relay().is_empty(),
        "target moved after snapshot; origin prefix must not defeat fetch"
    );
}

#[test]
fn cas_9efb_relay_dedupes_task_tip_ci_despite_unrelated_target_move() {
    let mut f = Fixture::new();
    assert_eq!(f.relay().len(), 1);
    assert!(f.relay().is_empty());
    f.advance_target(false);
    assert!(
        f.relay().is_empty(),
        "same delivery/CI cannot nag on unrelated epic changes"
    );
    f.ci(Some("failure"));
    let decision = f.relay();
    assert_eq!(
        decision.len(),
        1,
        "changed CI conclusion needs one decision"
    );
    assert!(decision[0].text.contains("CI failed"));
    assert!(f.relay().is_empty());
}

#[test]
fn cas_9efb_red_ci_relays_one_decision() {
    let mut f = Fixture::new();
    f.ci(Some("failure"));
    let prompts = f.relay();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].text.contains("CI failed"), "{}", prompts[0].text);
    assert!(f.relay().is_empty());
}

#[test]
fn cas_9efb_missing_ci_has_one_bounded_unknown_decision() {
    let mut f = Fixture::new();
    let now = Utc::now();
    std::fs::write(f.app.cas_dir.join("ci.json"), "{\"check_runs\":[]}").unwrap();
    f.app.merge_alert_retries.now = Some(now);
    assert!(f.relay().is_empty());
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(9));
    assert!(f.relay().is_empty());
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(10));
    let prompts = f.relay();
    assert_eq!(prompts.len(), 1, "missing CI must not silence the request forever");
    assert!(prompts[0].text.contains("CI unknown"), "{}", prompts[0].text);
    assert!(f.relay().is_empty());
    f.ci(Some("success"));
    assert_eq!(f.relay().len(), 1, "later settled CI is a new decision");
}

#[test]
fn cas_9efb_missing_ci_retries_without_a_new_worker_idle_event() {
    let mut f = Fixture::new();
    let now = Utc::now();
    f.ci(None);
    f.app.merge_alert_retries.now = Some(now);
    assert!(f.relay().is_empty());
    assert!(f.app.merge_alert_retry_events_at(now + chrono::Duration::seconds(59)).is_empty());
    let retries = f.app.merge_alert_retry_events_at(now + chrono::Duration::minutes(1));
    assert_eq!(retries.len(), 1, "the consumed WorkerIdle event must be retried each minute");
    assert!(f.app.merge_alert_retry_events_at(now + chrono::Duration::minutes(1)).is_empty());
    f.ci(Some("success"));
    let (_, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&retries);
    assert_eq!(prompts.len(), 1);
}

#[test]
fn cas_9efb_docs_only_and_cancelled_ci_have_terminal_decisions() {
    let mut f = Fixture::new();
    std::fs::write(f.app.cas_dir.join("ci.json"), serde_json::json!({"check_runs": [{
        "name": "Docs Lint", "status": "completed", "conclusion": "success",
    }]}).to_string()).unwrap();
    assert_eq!(f.relay().len(), 1, "Docs Lint is sufficient for a docs-only delivery");
    f.ci(Some("cancelled"));
    let cancelled = f.relay();
    assert_eq!(cancelled.len(), 1);
    assert!(cancelled[0].text.contains("CI failed"));
    assert!(f.relay().is_empty());
}

#[test]
fn cas_9efb_unknown_or_partly_pending_ci_defers_without_consuming_decision() {
    let mut f = Fixture::new();
    let ci_file = f.app.cas_dir.join("ci.json");
    std::fs::write(&ci_file, "{\"check_runs\":[]}").unwrap();
    assert!(f.relay().is_empty());
    f.ci(Some("failure"));
    let mut ci: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ci_file).unwrap()).unwrap();
    ci["check_runs"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "Docs Lint", "status": "in_progress", "conclusion": null,
        }));
    std::fs::write(&ci_file, ci.to_string()).unwrap();
    assert!(
        f.relay().is_empty(),
        "a red check cannot bypass unfinished CI"
    );
    f.ci(Some("failure"));
    assert_eq!(f.relay().len(), 1);
}

#[test]
fn cas_9efb_failed_target_fetch_defers_cached_positive_evidence() {
    let mut f = Fixture::new();
    git(
        f.repo.path(),
        &[
            "config",
            "--unset-all",
            &format!("url.{}.insteadOf", f.remote.path().display()),
        ],
    );
    let missing = f.repo.path().join("missing-remote");
    git(
        f.repo.path(),
        &[
            "config",
            &format!("url.{}.insteadOf", missing.display()),
            "https://github.com/acme/cas.git",
        ],
    );
    assert!(
        f.relay().is_empty(),
        "failed fetch cannot call cached origin live"
    );
    git(
        f.repo.path(),
        &[
            "config",
            "--unset-all",
            &format!("url.{}.insteadOf", missing.display()),
        ],
    );
    git(
        f.repo.path(),
        &[
            "config",
            &format!("url.{}.insteadOf", f.remote.path().display()),
            "https://github.com/acme/cas.git",
        ],
    );
    let recovered = f.relay();
    assert_eq!(
        recovered.len(),
        1,
        "a recovered probe still owes its first decision"
    );
    git(
        f.repo.path(),
        &[
            "config",
            "--unset-all",
            &format!("url.{}.insteadOf", f.remote.path().display()),
        ],
    );
    git(
        f.repo.path(),
        &[
            "config",
            &format!("url.{}.insteadOf", missing.display()),
            "https://github.com/acme/cas.git",
        ],
    );
    let prompt = recovered.first().unwrap();
    assert!(
        !f.app.prompt_is_still_deliverable(prompt),
        "a late failed fetch must defer transport too"
    );
}

#[test]
fn cas_9efb_parked_branch_and_new_delivery_tip_use_their_own_ci() {
    let mut f = Fixture::new();
    let branch = "factory/merge-w-cas-9efb-test";
    git(f.repo.path(), &["branch", branch]);
    git(
        f.repo.path(),
        &["update-ref", "refs/heads/factory/merge-w", "epic/test"],
    );
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut task = store.get("cas-9efb-test").unwrap();
    task.deliverables.parked_branch = Some(branch.into());
    store.update(&task).unwrap();
    let first = f.relay();
    assert_eq!(first.len(), 1);
    assert!(first[0].text.contains(branch), "{}", first[0].text);
    git(f.repo.path(), &["switch", "-q", branch]);
    std::fs::write(f.repo.path().join("delivery.rs"), "// correction\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "correct delivery"]);
    let tip = git(f.repo.path(), &["rev-parse", branch]);
    assert_eq!(f.relay().len(), 1, "new delivery deserves a decision");
    assert!(f.relay().is_empty());
    let queries = std::fs::read_to_string(f.app.cas_dir.join("gh-requests")).unwrap();
    assert!(
        queries.contains(&format!("repos/acme/cas/commits/{tip}/check-runs")),
        "{queries}"
    );
}
