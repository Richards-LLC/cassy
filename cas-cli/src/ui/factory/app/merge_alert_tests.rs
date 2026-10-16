//! Real delivery read → WorkerIdle prompt regressions; only GitHub is stubbed.
use super::*;
use cas_factory::ActiveLeaseSummary;
use cas_types::{Task, TaskStatus, WorkTarget};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").current_dir(repo)
        .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test"])
        .args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Fixture {
    repo: tempfile::TempDir,
    remote: tempfile::TempDir,
    app: FactoryApp,
    event: DirectorEvent,
    _env: crate::test_support::TestEnvGuard,
}

impl Fixture {
    fn new() -> Self {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let repo = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(repo.path(), &["init", "-q", "-b", "epic/test"]);
        git(repo.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        std::fs::write(repo.path().join("base.rs"), "// base\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "base"]);
        git(repo.path(), &["push", "-q", "origin", "epic/test"]);
        git(repo.path(), &["switch", "-qc", "factory/merge-w"]);
        std::fs::write(repo.path().join("delivery.rs"), "// delivery\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "cas-9efb-test delivery"]);
        git(repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
        // Git fetches the local bare server, while GitHub identity remains explicit.
        git(repo.path(), &["config", &format!("url.{}.insteadOf", remote.path().display()), "https://github.com/acme/cas.git"]);
        git(repo.path(), &["remote", "set-url", "origin", "https://github.com/acme/cas.git"]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        env.set("CAS_ROOT", &cas_dir);
        let mut task = Task::new("cas-9efb-test".into(), "parked delivery".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("merge-w".into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(), target_branch: "epic/test".into(),
        });
        crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
        let mut worker = cas_types::Agent::new("merge-w".into(), "merge-w".into());
        worker.role = cas_types::AgentRole::Worker;
        crate::store::open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
        let mut data = DirectorData::load_with_stores_for_project(&cas_dir, None, false, None, None).unwrap();
        data.agent_id_to_name.insert("merge-w".into(), "merge-w".into());
        let mut app = FactoryApp::for_test_at(cas_dir.clone());
        app.set_director_data_for_test(data);
        let event = DirectorEvent::WorkerIdle {
            worker: "merge-w".into(), active_task: Some(ActiveLeaseSummary {
                task_id: task.id, task_title: task.title, task_status: task.status,
                close_rejected_reason: Some("MERGE REQUIRED".into()), pending_qa: None,
            }),
        };
        let gh = cas_dir.join("gh-fixture");
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{\"nameWithOwner\":\"acme/cas\"}'; else printf '%s\\n' \"$*\" >> .cas/gh-requests; cat .cas/ci.json; fi\n");
        env.set(crate::github_issue_attach::GH_BIN_ENV, gh);
        let fixture = Self { repo, remote, app, event, _env: env };
        fixture.ci(Some("success"));
        fixture
    }

    fn ci(&self, conclusion: Option<&str>) {
        std::fs::write(self.app.cas_dir.join("ci.json"), serde_json::json!({"check_runs": [{
            "name": "Scoped Validation (factory/PR)",
            "status": if conclusion.is_some() { "completed" } else { "in_progress" },
            "conclusion": conclusion,
            "html_url": "https://github.com/acme/cas/actions/runs/123",
        }]}).to_string()).unwrap();
    }

    fn relay(&mut self) -> Vec<Prompt> {
        self.app.revalidate_and_prompt_for_delivery(std::slice::from_ref(&self.event)).1
    }

    fn advance_target(&self, merge: bool) {
        let integration = tempfile::tempdir().unwrap();
        git(integration.path(), &["clone", "-q", "-b", "epic/test", self.remote.path().to_str().unwrap(), "."]);
        if merge {
            git(integration.path(), &["merge", "-q", "--ff-only", "origin/factory/merge-w"]);
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
    assert_eq!(f.relay().len(), 1, "pending must not consume the settled relay");
}

#[test]
fn cas_9efb_live_target_suppresses_relay_with_origin_prefixed_target() {
    let mut f = Fixture::new();
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut task = store.get("cas-9efb-test").unwrap();
    task.deliverables.work_target.as_mut().unwrap().target_branch = "origin/epic/test".into();
    store.update(&task).unwrap();
    f.advance_target(true);
    assert!(f.relay().is_empty(), "target moved after snapshot; origin prefix must not defeat fetch");
}

#[test]
fn cas_9efb_relay_dedupes_task_tip_ci_despite_unrelated_target_move() {
    let mut f = Fixture::new();
    assert_eq!(f.relay().len(), 1);
    assert!(f.relay().is_empty());
    f.advance_target(false);
    assert!(f.relay().is_empty(), "same delivery/CI cannot nag on unrelated epic changes");
    f.ci(Some("failure"));
    let decision = f.relay();
    assert_eq!(decision.len(), 1, "changed CI conclusion needs one decision");
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
