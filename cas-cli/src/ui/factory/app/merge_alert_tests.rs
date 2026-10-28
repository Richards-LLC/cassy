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
    pub(crate) _env: crate::test_support::TestEnvGuard,
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
        task.deliverables.factory_branch_anchor = Some(git(repo.path(), &["rev-parse", "HEAD"]));
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
        app.add_worker_name_for_test("merge-w");
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

    fn in_progress(&mut self) {
        let store = crate::store::open_task_store_local(&self.app.cas_dir).unwrap();
        let mut task = store.get("cas-9efb-test").unwrap();
        task.status = TaskStatus::InProgress;
        store.update(&task).unwrap();
        assert!(crate::store::open_agent_store(&self.app.cas_dir).unwrap()
            .try_claim(&task.id, "merge-w", 600, None).unwrap().is_success());
        let DirectorEvent::WorkerIdle { active_task: Some(active), .. } = &mut self.event else {
            panic!("fixture must carry the active delivery");
        };
        active.task_status = TaskStatus::InProgress;
        active.close_rejected_reason = None;
        // A real terminal turn in this fixture's own account makes liveness idle.
        // Without it the generic liveness gate suppresses the relay before CI is read.
        let account = self.app.cas_dir.join("worker-account");
        let sessions = account.join("sessions/2026/10/17");
        std::fs::create_dir_all(&sessions).unwrap();
        let session = "019f84af-3121-7950-ba14-b01db2dad6c7";
        let clone = self.repo.path().canonicalize().unwrap();
        let meta = serde_json::json!({"timestamp":"2026-10-17T19:00:00Z", "type":"session_meta",
            "payload":{"session_id":session, "cwd":clone, "originator":"codex-tui", "source":"cli"}});
        let done = serde_json::json!({"timestamp":"2026-10-17T19:00:01Z", "type":"event_msg",
            "payload":{"type":"task_complete"}});
        std::fs::write(sessions.join(format!("rollout-2026-10-17T19-00-00-{session}.jsonl")),
            format!("{meta}\n{done}\n")).unwrap();
        let agents = crate::store::open_agent_store(&self.app.cas_dir).unwrap();
        let mut worker = agents.get("merge-w").unwrap();
        worker.cc_session_id = Some(session.into());
        worker.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker.metadata.insert("clone_path".into(), clone.to_str().unwrap().into());
        worker.metadata.insert("worker_account_dir".into(), account.to_str().unwrap().into());
        agents.update(&worker).unwrap();
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

    pub(crate) fn advance_target(&self, merge: bool) {
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

    /// Serve `https://github.com/acme/cas.git` from `to` instead of `from`.
    fn reroute_origin(&self, from: &Path, to: &Path) {
        let key = |path: &Path| format!("url.{}.insteadOf", path.display());
        git(self.repo.path(), &["config", "--unset-all", &key(from)]);
        git(
            self.repo.path(),
            &["config", &key(to), "https://github.com/acme/cas.git"],
        );
    }
}

#[test]
fn cas_65e8_settled_lanes_stop_probing_and_ready_survives_restart() {
    let mut f = Fixture::new();
    f.in_progress();
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut second = store.get("cas-9efb-test").unwrap();
    second.id = "cas-second".into();
    second.assignee = Some("merge-v".into());
    second.deliverables.factory_branch_anchor = None;
    store.add(&second).unwrap();
    let agents = crate::store::open_agent_store(&f.app.cas_dir).unwrap();
    let mut worker = cas_types::Agent::new("merge-v".into(), "merge-v".into());
    worker.role = cas_types::AgentRole::Worker;
    agents.register(&worker).unwrap();
    assert!(agents.try_claim(&second.id, &worker.id, 600, None).unwrap().is_success());
    git(f.repo.path(), &["branch", "factory/merge-v"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-v"]);
    f.app.add_worker_name_for_test("merge-v");
    let data = DirectorData::load_with_stores_for_project(&f.app.cas_dir, None, false, None, None).unwrap();
    f.app.set_director_data_for_test(data);
    let events = vec![f.event.clone(), DirectorEvent::WorkerIdle {
        worker: worker.name,
        active_task: Some(ActiveLeaseSummary {
            task_id: second.id, task_title: second.title, task_status: second.status,
            close_rejected_reason: None, pending_qa: None,
        }),
    }];
    // The second lane retains its failed SHA while only the corrected SHA's
    // GitHub run changes. Keep this at the real external-service boundary.
    let old_tip = git(f.repo.path(), &["rev-parse", "factory/merge-v"]);
    f.ci(Some("failure"));
    std::fs::copy(f.app.cas_dir.join("ci.json"), f.app.cas_dir.join("old-ci.json")).unwrap();
    crate::test_paths::warm_stub(&f.app.cas_dir.join("gh-fixture"), &format!(
        "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{{\"nameWithOwner\":\"acme/cas\"}}'; else printf '%s\\n' \"$*\" >> .cas/gh-requests; case \"$*\" in *{old_tip}*) cat .cas/old-ci.json;; *) cat .cas/ci.json;; esac; fi\n"
    ));
    let trace = f.app.cas_dir.join("git-trace.log");
    f._env.set("GIT_TRACE", &trace);
    let counts = |f: &Fixture| {
        let fetches = std::fs::read_to_string(&trace).unwrap_or_default().lines()
            .filter(|line| line.contains("built-in: git fetch ")).count();
        let ci = std::fs::read_to_string(f.app.cas_dir.join("gh-requests")).unwrap_or_default().lines().count();
        (fetches, ci)
    };
    f.ci(Some("failure"));
    f.app.revalidate_and_prompt_for_delivery(&events);
    let settled = counts(&f);
    assert!(settled.0 > 0 && settled.1 > 0);
    f.app.revalidate_and_prompt_for_delivery(&events);
    assert_eq!(counts(&f), settled);

    std::fs::write(f.repo.path().join("delivery.rs"), "// new pushed tip\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "cas-9efb-test correction"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    f.ci(None);
    f.app.revalidate_and_prompt_for_delivery(&events);
    let pending = counts(&f);
    assert_eq!(pending.1 - settled.1, 1);
    f.ci(Some("success"));
    let (_, ready, _) = f.app.revalidate_and_prompt_for_delivery(&events);
    assert_eq!(ready.iter().filter(|prompt| prompt.merge_alert.is_some()).count(), 1);
    let signalled = counts(&f);
    f.app.revalidate_and_prompt_for_delivery(&events);
    assert_eq!(counts(&f), signalled);

    let data = DirectorData::load_with_stores_for_project(&f.app.cas_dir, None, false, None, None).unwrap();
    f.app = FactoryApp::for_test_at(f.app.cas_dir.clone());
    for name in ["merge-w", "merge-v"] { f.app.add_worker_name_for_test(name); }
    f.app.set_director_data_for_test(data);
    let (_, restarted, _) = f.app.revalidate_and_prompt_for_delivery(&events);
    assert!(restarted.iter().all(|prompt| prompt.merge_alert.is_none()));
    // The red lane may need its first observation in the new process, but the
    // already-signalled green lane must not contact Git or GitHub again.
    let after_restart = counts(&f);
    assert_eq!(after_restart.1 - signalled.1, 1);
    f.app.revalidate_and_prompt_for_delivery(&events);
    assert_eq!(counts(&f), after_restart);
    std::fs::write(f.repo.path().join("delivery.rs"), "// already merged correction\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "cas-9efb-test merged correction"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    f.advance_target(true);
    f.app.revalidate_and_prompt_for_delivery(&events[..1]);
    let merged = counts(&f);
    f.app.merge_alert_retries.now = Some(Utc::now() + chrono::Duration::hours(1));
    f.app.revalidate_and_prompt_for_delivery(&events[..1]);
    assert_eq!(counts(&f), merged);
    let receipts = f.app.cas_dir.join("receipts/merge-ready");
    assert_eq!(std::fs::read_dir(&receipts).unwrap().count(), 1);
    let mut closed = store.get("cas-9efb-test").unwrap();
    closed.status = TaskStatus::Closed;
    store.update(&closed).unwrap();
    f.app.merge_alert_retries.now = Some(Utc::now() + chrono::Duration::hours(2));
    f.app.revalidate_and_prompt_for_delivery(&events[..1]);
    assert_eq!(std::fs::read_dir(&receipts).unwrap().count(), 0);
}

/// Green lane CI can omit the newly failing test; only the worker can mark
/// this test-first checkpoint complete. Exercise producer, cache and release.
#[test]
fn cas_229a_red_checkpoint_never_becomes_a_merge_ready_delivery() {
    let mut f = Fixture::new();
    f.in_progress();
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut task = store.get("cas-9efb-test").unwrap();
    task.execution_note = Some("test-first".into());
    task.deliverables.factory_branch_anchor = None;
    let tip = git(f.repo.path(), &["rev-parse", "HEAD"]);
    // Replace the fixture delivery with a real test-only, intentionally red
    // commit. Its lane CI fixture remains green because that lane skips it.
    git(f.repo.path(), &["reset", "--hard", "epic/test"]);
    std::fs::write(f.repo.path().join("regression_test.rs"), "#[test] fn regression() { assert!(false); }\n").unwrap();
    git(f.repo.path(), &["add", "regression_test.rs"]);
    git(f.repo.path(), &["commit", "-qm", "test: red test-first checkpoint"]);
    git(f.repo.path(), &["push", "-q", "--force", "origin", "factory/merge-w"]);
    let checkpoint = git(f.repo.path(), &["rev-parse", "HEAD"]);
    assert_ne!(tip, checkpoint);
    for (status, methodology, notes) in [
        (TaskStatus::InProgress, Some("test-first"), String::new()),
        (TaskStatus::InProgress, Some("test-first"),
            format!("[2026-10-28 12:00] 📝 PROGRESS Red test-only checkpoint {checkpoint}.")),
        (TaskStatus::Blocked, None,
            format!("[2026-10-28 12:00] 📝 PROGRESS Test-only checkpoint {checkpoint}.\nImplementation pending.")),
        (TaskStatus::InProgress, None,
            format!("[2026-10-28 12:00] 📝 PROGRESS {checkpoint} is not merge-ready.")),
        (TaskStatus::InProgress, Some("test-first"),
            format!("[2026-10-28 12:00] 📝 PROGRESS PASS {tip}: previous delivery complete.")),
    ] {
        task.status = status;
        task.execution_note = methodology.map(str::to_string);
        task.notes = notes;
        store.update(&task).unwrap();
        let prompts = f.relay();
        assert!(prompts.iter().all(|prompt| !prompt.is_merge_ready()),
            "a test-first checkpoint is not a completed delivery: {prompts:?}");
    }
    task.notes.push_str(&format!("\n\n[2026-10-28 12:00] 📝 PROGRESS\nstatus: ready\ntip: {tip} on factory/merge-w\nproof: comparing with {checkpoint}"));
    store.update(&task).unwrap();
    assert!(f.relay().iter().all(|prompt| !prompt.is_merge_ready()));
    // Completion may be recorded before park. It must invalidate the settled
    // negative read and produce one immediate signal without a new commit.
    task.notes.push_str(&format!("\n\n[2026-10-28 12:01] 📝 PROGRESS PASS {checkpoint}: completed test-only correction; regression green."));
    store.update(&task).unwrap();
    let mut ready = f.relay().into_iter().find(|prompt| prompt.is_merge_ready())
        .expect("worker completion enables the pre-park signal");
    assert!(f.relay().iter().all(|prompt| !prompt.is_merge_ready()));
    // A later checkpoint correction must withdraw a signal already cached,
    // receipted and queued, even though the Git tip and lane CI did not move.
    task.notes.push_str(&format!("\n\n[2026-10-28 12:02] 📝 PROGRESS Correction: {checkpoint} remains a red checkpoint, not merge-ready."));
    store.update(&task).unwrap();
    assert!(!f.app.refresh_prompt_for_delivery(&mut ready));
    assert!(f.relay().iter().all(|prompt| !prompt.is_merge_ready()));
    task.notes.push_str(&format!("\n\n[2026-10-28 12:03] 📝 PROGRESS\nstatus: ready\ntip: {checkpoint}"));
    store.update(&task).unwrap();
    assert!(f.app.refresh_prompt_for_delivery(&mut ready));
    // Same-tip completion must not duplicate the wake.
    assert!(f.relay().iter().all(|prompt| !prompt.is_merge_ready()));
    // Ordinary park is an explicit completion boundary. Historical checkpoint
    // notes cannot silence its single signal (use a fresh receipt store).
    std::fs::remove_dir_all(f.app.cas_dir.join("receipts/merge-ready")).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(checkpoint);
    store.update(&task).unwrap();
    let DirectorEvent::WorkerIdle { active_task: Some(active), .. } = &mut f.event else { unreachable!() };
    active.task_status = TaskStatus::AwaitingMerge;
    assert_eq!(f.relay().iter().filter(|prompt| prompt.is_merge_ready()).count(), 1);
    assert!(f.relay().iter().all(|prompt| !prompt.is_merge_ready()));
}

#[test]
fn cas_20d3_green_pushed_tip_signals_once_when_park_was_refused() {
    for status in [TaskStatus::InProgress, TaskStatus::Blocked] {
        let mut f = Fixture::new();
        f.in_progress();
        let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
        let mut task = store.get("cas-9efb-test").unwrap();
        task.status = status;
        // A refused park leaves the task active, with no parked branch or tip.
        task.deliverables.factory_branch_anchor = None;
        task.notes = "Close rejected: FAST ROWS REQUIRED".into();
        store.update(&task).unwrap();
        if status == TaskStatus::Blocked {
            git(f.repo.path(), &["switch", "-qc", "factory/merge-w-cas-9efb-test"]);
            git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w-cas-9efb-test"]);
        }
        let branch = git(f.repo.path(), &["branch", "--show-current"]);
        let data = DirectorData::load_with_stores_for_project(
            &f.app.cas_dir, None, false, None, None,
        ).unwrap();
        f.app.set_director_data_for_test(data);
        let now = Utc::now();
        f.app.merge_alert_retries.now = Some(now);
        // A shared unfiltered snapshot cannot dispatch another supervisor's lane.
        let workers = std::mem::take(&mut f.app.worker_names);
        assert!(f.app.merge_alert_retry_events_at(now).is_empty());
        f.app.worker_names = workers;
        // No WorkerIdle, close, or merge_request event precedes this probe.
        let events = f.app.merge_alert_retry_events_at(now);
        for ci in [None, Some("failure")] {
            f.ci(ci);
            let (_, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&events);
            assert!(prompts.iter().all(|prompt| prompt.merge_alert.is_none()));
        }
        // A settled failure is revisited on the slow same-SHA CI rerun cadence.
        f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(10));
        f.ci(Some("success"));
        let (_, mut prompts, _) = f.app.revalidate_and_prompt_for_delivery(&events);
        assert_eq!(prompts.len(), 1, "{status:?}");
        let prompt = &mut prompts[0];
        // pin: this heading is the supervisor's immediate merge trigger.
        assert!(prompt.text.contains("READY TO MERGE"), "{}", prompt.text);
        assert_eq!(prompt.merge_alert.as_ref().unwrap().factory_branch, branch);
        // pin: merge guidance must identify the resolved target and executable action.
        assert!(prompt.text.contains("epic/test"), "{}", prompt.text);
        // pin: the supervisor merges directly without a worker relay.
        assert!(prompt.text.contains("worktree_merge"), "{}", prompt.text);
        assert!(f.app.refresh_prompt_for_delivery(prompt));
        // Routing is production intent; localized wording cannot turn a green delivery red.
        prompt.text = prompt.text.replacen("READY TO MERGE", "Merge delivery now", 1);
        assert!(prompt.is_merge_ready());
        // The actual director producer, park relay and manual worker request
        // must reserve one shared wake, including before a task can park.
        let session = "readiness-fixture";
        let agents = crate::store::open_agent_store(&f.app.cas_dir).unwrap();
        let mut supervisor = cas_types::Agent::new("lead".into(), prompt.target.clone());
        supervisor.role = cas_types::AgentRole::Supervisor;
        supervisor.factory_session = Some(session.into());
        agents.register(&supervisor).unwrap();
        let mut worker = agents.get("merge-w").unwrap();
        worker.factory_session = Some(session.into());
        agents.update(&worker).unwrap();
        let queue = crate::store::open_prompt_queue_store(&f.app.cas_dir).unwrap();
        let director = crate::ui::factory::daemon::runtime::delivery::enqueue_merge_ready_prompt(
            &f.app.cas_dir, session, prompt,
        ).unwrap();
        let row = queue.queued_prompt(director).unwrap().unwrap();
        assert!(crate::ui::factory::daemon::runtime::delivery::parse_merge_ready_prompt(
            &row.target, &row.prompt).unwrap().is_merge_ready());
        use crate::ui::factory::daemon::runtime::queue_and_events::{read_row_facts, WakeSender, SupervisorWakeClass};
        assert!(matches!(read_row_facts(&f.app.cas_dir, queue.as_ref(), Some(store.as_ref()), &[], &row).director_ready,
            Some(Ok(Some(_)))), "unparked green delivery remains actionable through the queue");
        assert_eq!(crate::ui::factory::daemon::FactoryDaemon::supervisor_wake_class(
            &WakeSender::Daemon, &prompt.target, &row.source, &row.prompt), Some(SupervisorWakeClass::MergeRequest));
        assert_eq!(crate::ui::factory::daemon::FactoryDaemon::supervisor_wake_class(
            &WakeSender::Unattributed, &prompt.target, &row.source, &row.prompt), None,
            "the director envelope cannot grant an unauthenticated wake");
        git(f.remote.path(), &["config", "uploadpack.hideRefs", &format!("refs/heads/{branch}")]);
        assert!(matches!(read_row_facts(&f.app.cas_dir, queue.as_ref(), Some(store.as_ref()), &[], &row).director_ready,
            Some(Err(_))), "an unavailable source fetch defers the sole action instead of consuming it");
        git(f.remote.path(), &["config", "--unset", "uploadpack.hideRefs"]);
        assert!(matches!(read_row_facts(&f.app.cas_dir, queue.as_ref(), Some(store.as_ref()), &[], &row).director_ready,
            Some(Ok(Some(_)))), "source recovery retains the original queued action");
        let head = &prompt.merge_alert.as_ref().unwrap().factory_tip;
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(head.clone());
        store.update(&task).unwrap();
        let lifecycle = format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"{}\" old=\"in_progress\" new=\"awaiting_merge\" actor=\"merge-w\" notification_id=\"1\" occurrence=\"{}\" branch_tip=\"{head}\">MERGE REQUIRED</task-lifecycle>",
            task.id, Utc::now().to_rfc3339(),
        );
        queue.enqueue_idempotent("lifecycle-wake:1", &prompt.target, &lifecycle, Some(session), None, None,
            "park-1", Some(&cas_store::QueueOrigin::Daemon)).unwrap();
        let manual = format!(
            "manual proof remains readable\n<cas-merge-request>{{\"task_id\":\"{}\",\"branch_tip\":\"{head}\",\"target_branch\":\"epic/test\",\"target_branch_tip\":\"{}\"}}</cas-merge-request>",
            task.id, prompt.merge_alert.as_ref().unwrap().epic_sha,
        );
        let manual_id = queue.enqueue_urgent_with_outcome("merge-w", &prompt.target, &manual, Some(session), None, None,
            false, Some(&cas_store::QueueOrigin::RegisteredAgent { agent_id: worker.id.clone() })).unwrap().id();
        let rows = queue.peek_for_targets(&[&prompt.target], Some(session), 10).unwrap();
        assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![director],
            "direct readiness and later park/manual evidence must create only one transport");
        let full = queue.poll_named_for_recipient(&prompt.target, Some(session), &[manual_id]).unwrap();
        // pin: explicit evidence reads preserve the complete submitted body.
        assert_eq!(full[0].prompt, manual, "manual evidence survives dedupe");
        // Restore this test's active-task scenario for its new-tip controls.
        task.status = status;
        task.deliverables.factory_branch_anchor = None;
        store.update(&task).unwrap();
        assert!(f.app.revalidate_and_prompt_for_delivery(&events).1.is_empty());
        std::fs::write(f.repo.path().join("delivery.rs"), "// next delivery\n").unwrap();
        git(f.repo.path(), &["commit", "-qam", "cas-9efb-test correction"]);
        let (_, unpushed, _) = f.app.revalidate_and_prompt_for_delivery(&events);
        assert!(unpushed.iter().all(|prompt| prompt.merge_alert.is_none()));
        assert!(!f.app.refresh_prompt_for_delivery(prompt));
        assert!(matches!(read_row_facts(&f.app.cas_dir, queue.as_ref(), Some(store.as_ref()), &[], &row).director_ready,
            Some(Ok(None))), "a newer local tip withdraws queued readiness for the old tip");
        git(f.repo.path(), &["push", "-q", "origin", &branch]);
        let (_, mut newer, _) = f.app.revalidate_and_prompt_for_delivery(&events);
        assert_eq!(newer.len(), 1);
        let next = crate::ui::factory::daemon::runtime::delivery::enqueue_merge_ready_prompt(
            &f.app.cas_dir, session, &newer[0],
        ).unwrap();
        assert!(queue.peek_for_targets(&[&prompt.target], Some(session), 10).unwrap().iter().any(|row| row.id == next),
            "a different pushed tip gets its own action");
        let next_row = queue.queued_prompt(next).unwrap().unwrap();
        assert!(f.app.revalidate_and_prompt_for_delivery(&events).1.is_empty());
        git(f.repo.path(), &["push", "-q", "origin", "HEAD:epic/test"]);
        assert!(!f.app.refresh_prompt_for_delivery(&mut newer[0]));
        assert!(matches!(read_row_facts(&f.app.cas_dir, queue.as_ref(), Some(store.as_ref()), &[], &next_row).director_ready,
            Some(Ok(None))), "integrated delivery must be withdrawn before queue transport");
    }
}

#[test]
fn cas_9efb_pending_ci_suppresses_worker_idle_merge_relay() {
    let mut f = Fixture::new();
    let now = Utc::now();
    f.ci(None);
    f.app.merge_alert_retries.now = Some(now);
    assert!(f.relay().is_empty(), "CI pending is not actionable");
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(30));
    assert!(
        f.relay().is_empty(),
        "the missing-CI fallback never turns pending into a merge demand"
    );
    f.ci(Some("success"));
    assert_eq!(
        f.relay().len(),
        1,
        "pending must not consume the settled relay"
    );
}

/// A worker waiting on its own pushed CI is not stalled, even if repeated
/// idle episodes arrive while the task is still in progress.
#[test]
fn cas_74b8_in_progress_worker_waits_on_self_reminder_build_or_ci_cas_e390() {
    let mut f = Fixture::new();
    f.in_progress();
    // Exercise idle/wait recovery, rather than the independent green merge signal.
    f.ci(Some("failure"));
    let now = Utc::now();
    f.app.merge_alert_retries.now = Some(now);
    use cas_store::ReminderStore;
    let reminders = cas_store::SqliteReminderStore::open(&f.app.cas_dir).unwrap();
    reminders.init().unwrap();
    let long = reminders.create_with_scope("merge-w", Some("merge-w"), "check next week",
        cas_store::ReminderTriggerType::Time, Some(now + chrono::Duration::days(7)),
        None, None, 604800, None, Some("merge-w"), true, None).unwrap();
    assert_eq!(f.relay().len(), 1, "an untasked week-long reminder cannot hide a stuck worker");
    reminders.cancel(long, "merge-w").unwrap();
    // The real reminder store is reloaded by the delivery read, not the cached panel.
    let reminder = reminders.create_with_scope("merge-w", Some("merge-w"), "check build",
        cas_store::ReminderTriggerType::Time, Some(now + chrono::Duration::days(7)),
        None, None, 604800, None, Some("merge-w"), false, Some("cas-9efb-test")).unwrap();
    assert!(f.relay().is_empty(), "a pending self-reminder owes the worker its next wake");
    reminders.cancel(reminder, "merge-w").unwrap();

    let short = reminders.create_with_scope("merge-w", Some("merge-w"), "check soon",
        cas_store::ReminderTriggerType::Time, Some(now + chrono::Duration::minutes(3)),
        None, None, 3600, None, Some("merge-w"), false, None).unwrap();
    assert!(f.relay().is_empty(), "an untasked short-horizon wake still suppresses idle");
    reminders.cancel(short, "merge-w").unwrap();

    // Use the same OS lane lock the real runner holds during admission and
    // execution. Its release leaves a file behind, which must not hide idle.
    let lane = crate::factory_worker_check::try_lock_lane(&f.app.cas_dir, f.repo.path()).unwrap().unwrap();
    #[cfg(target_os = "linux")]
    assert!(f.relay().is_empty(), "queued/admitted build owns the worker lane");
    drop(lane);
    // A reminder for an old task and another worker's build are not this wait.
    reminders.create_with_scope("merge-w", Some("merge-w"), "old task",
        cas_store::ReminderTriggerType::Time, Some(Utc::now() + chrono::Duration::minutes(3)),
        None, None, 3600, None, Some("merge-w"), false, Some("cas-old")).unwrap();
    let _other_lane = crate::factory_worker_check::try_lock_lane(&f.app.cas_dir, f.remote.path()).unwrap().unwrap();
    std::fs::write(f.app.cas_dir.join("gh-requests"), "").unwrap();
    let minute = now + chrono::Duration::minutes(1);
    f.app.merge_alert_retries.now = Some(minute);
    let retries = f.app.merge_alert_retry_events_at(minute);
    assert_eq!(retries.len(), 1, "suppressed in-progress wait retains its delivery retry");
    assert_eq!(f.app.revalidate_and_prompt_for_delivery(&retries).1.len(), 1,
        "after the wake ends, an idle worker must still alert without a new detector event");
    // A same-SHA rerun is visible when the settled-failure backoff expires.
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(10));
    f.ci(None);
    let relays: usize = (0..3).map(|_| f.relay().len()).sum();
    assert_eq!(relays, 0, "pending pushed-tip CI must suppress every idle relay");
    let tip = git(f.repo.path(), &["rev-parse", "HEAD"]);
    let requests = std::fs::read_to_string(f.app.cas_dir.join("gh-requests")).unwrap();
    assert!(requests.contains(&format!("commits/{tip}/check-runs")), "query the pushed tip: {requests}");
    assert!(f.app.delivery_request(std::slice::from_ref(&f.event)).needs_external_lane(),
        "Git and CI reads must stay off the control/read lanes");

    // A local commit with no pushed CI is not covered by the old tip's run.
    std::fs::write(f.repo.path().join("delivery.rs"), "// unpushed follow-up\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "unpushed follow-up"]);
    assert_eq!(f.relay().len(), 1, "old CI must not hide unpushed work");
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    assert!(f.relay().is_empty(), "new pushed tip can wait on its own CI");
    std::fs::write(f.app.cas_dir.join("ci.json"), r#"{"check_runs":[]}"#).unwrap();
    assert_eq!(f.relay().len(), 1, "unknown CI must not hide an idle delivery");
    f.ci(Some("success"));
    assert_eq!(f.relay().len(), 1, "settled CI restores the lifecycle alert");
}

/// Same task/tip liveness relays may arrive from retry jobs rather than the detector.
#[test]
fn cas_74b8_idle_delivery_coalesces_repeats_until_state_changes_or_long_backoff_cas_e390() {
    let mut f = Fixture::new();
    f.in_progress();
    // Green delivery tips have a one-shot merge signal, not an idle backoff.
    f.ci(Some("failure"));
    let now = Utc::now();
    f.app.merge_alert_retries.now = Some(now);
    assert_eq!(f.relay().len(), 1, "a stuck worker with no pending wake still alerts");
    for minutes in [1, 2, 5] {
        f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(minutes));
        assert!(f.relay().is_empty(), "identical idle signal must not wake again at minute {minutes}");
    }
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::hours(1));
    assert_eq!(f.relay().len(), 1, "a long backoff permits a reminder about a genuinely stuck worker");
    std::fs::write(f.repo.path().join("delivery.rs"), "// new tip\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "new tip"]);
    assert_eq!(f.relay().len(), 1, "new tip is a changed idle signal");
    assert!(f.relay().is_empty());
    // Missing worker refs are a stable state, not permission to wake each minute.
    git(f.repo.path(), &["switch", "-q", "--detach", "epic/test"]);
    git(f.repo.path(), &["branch", "-D", "factory/merge-w"]);
    git(f.repo.path(), &["update-ref", "-d", "refs/remotes/origin/factory/merge-w"]);
    assert_eq!(f.relay().len(), 1, "unknown commit still owes one alert");
    for minutes in [1, 2] {
        f.app.merge_alert_retries.now = Some(now + chrono::Duration::hours(1) + chrono::Duration::minutes(minutes));
        assert!(f.relay().is_empty(), "unknown commit must coalesce at minute {minutes}");
    }
    // A failed fresh read has no worker_waits row; it needs the same unknown key.
    let events = vec![f.event.clone()];
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::hours(2));
    assert_eq!(f.app.revalidate_and_prompt_for_delivery_from(&events, DeliveryInputs::failed_for_test()).1.len(), 1);
    assert!(f.app.revalidate_and_prompt_for_delivery_from(&events, DeliveryInputs::failed_for_test()).1.is_empty(),
        "failed reads retain first-alert liveness without manufacturing repeated wakes");

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

/// cas-e48d: CI can finish after the integration target has moved, while
/// the worker tip remains unchanged. Exercise the real delivery-read path;
/// only the GitHub response is replaced, using a real bare Git remote.
#[test]
fn cas_3f728_release_uses_original_delivery_and_labels_request_changes() {
    for outcome in [
        "pending",
        "merged",
        "superseded",
        "refs_converged",
        "refs_diverged",
    ] {
        let mut f = Fixture::new();
        if outcome == "pending" {
            std::fs::write(
                f.repo.path().join("second.rs"),
                "// second delivery commit\n",
            )
            .unwrap();
            git(f.repo.path(), &["add", "second.rs"]);
            git(f.repo.path(), &["commit", "-qm", "second delivery commit"]);
            let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
            let mut task = store.get("cas-9efb-test").unwrap();
            task.deliverables.factory_branch_anchor =
                Some(git(f.repo.path(), &["rev-parse", "HEAD"]));
            store.update(&task).unwrap();
        }
        if outcome == "refs_converged" {
            git(
                f.repo.path(),
                &["branch", "-f", "epic/test", "factory/merge-w"],
            );
        }
        let mut prompt = f.relay().pop().expect("settled CI builds a merge signal");
        let tip = git(f.repo.path(), &["rev-parse", "factory/merge-w"]);
        if outcome == "pending" {
            // A partial integration reduces 2 to 1; unrelated prose with 12 must stay intact.
            let parent = git(f.repo.path(), &["rev-parse", "HEAD^"]);
            git(
                f.remote.path(),
                &["update-ref", "refs/heads/epic/test", &parent],
            );
            prompt
                .text
                .insert_str(0, "12 unmerged commit(s) in another delivery.\n");
        }
        if outcome == "merged" {
            f.advance_target(true);
            // The shared worker lane can advance while the old CI signal waits.
            std::fs::write(f.repo.path().join("next-task.rs"), "// next delivery\n").unwrap();
            git(f.repo.path(), &["add", "next-task.rs"]);
            git(f.repo.path(), &["commit", "-qm", "another task"]);
        } else if outcome == "superseded" {
            // The request's store need not be named .cas; Git still lives in its parent.
            let cas_dir = f.repo.path().join("task-state");
            std::fs::rename(&f.app.cas_dir, &cas_dir).unwrap();
            f.app.cas_dir = cas_dir;
            f._env.set("CAS_ROOT", &f.app.cas_dir);
            let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
            let mut task = store.get("cas-9efb-test").unwrap();
            task.deliverables.factory_branch_anchor = Some(tip.clone());
            task.deliverables.decline_factory_branch_anchor();
            task.status = TaskStatus::Open;
            store.update(&task).unwrap();
        }
        if outcome == "refs_converged" {
            // An unpushed local merge was reset; both refs now agree that a merge is needed.
            assert!(prompt.merge_alert.as_ref().unwrap().push_required);
            git(
                f.repo.path(),
                &["branch", "-f", "epic/test", "origin/epic/test"],
            );
        } else if outcome == "refs_diverged" {
            // The local merge just landed, but origin still needs the push.
            assert!(!prompt.merge_alert.as_ref().unwrap().push_required);
            git(
                f.repo.path(),
                &["branch", "-f", "epic/test", "factory/merge-w"],
            );
        }
        let deliver = f
            .app
            .deliverability_request()
            .refresh_merge_prompt(&mut prompt);
        match outcome {
            // pin: the released signal's text is included when an integrated tip is wrongly actionable.
            "merged" => assert!(
                !deliver,
                "already-integrated tip must not borrow the next lane tip: {}",
                prompt.text
            ),
            "superseded" => {
                // pin: released director signals must name a declined tip as superseded.
                assert!(
                    prompt.text.to_lowercase().contains("superseded"),
                    "{}",
                    prompt.text
                );
                // pin: this heading asks the supervisor to merge and must disappear on supersession.
                assert!(!prompt.text.contains("MERGE REQUIRED"), "{}", prompt.text);
            }
            "refs_converged" | "refs_diverged" => {
                assert!(deliver, "a pending merge or push still releases");
                let push_required = outcome == "refs_diverged";
                // pin: ref-disagreement must reflect the same live evidence as the action.
                let disagreement = prompt.text.contains("Git ref disagreement detected:");
                // pin: this action applies only while the local merge needs a push.
                let push_action = prompt.text.contains("2. Push required:");
                // pin: the alternative action must ask for the still-outstanding merge.
                let merge_action = prompt.text.contains("2. Merge factory/merge-w into epic/test");
                assert_eq!(
                    (disagreement, push_action, merge_action),
                    (push_required, push_required, !push_required),
                    "{}", prompt.text,
                );
            }
            _ => {
                assert!(deliver, "a genuinely outstanding merge still releases");
                // pin: a live CI-completed signal retains its actionable merge heading.
                assert!(prompt.text.contains("MERGE REQUIRED"));
                // pin: refresh only replaces the complete count, never a suffix of another number.
                assert!(
                    prompt.text.starts_with("12 unmerged commit(s)"),
                    "{}",
                    prompt.text
                );
                assert_eq!(prompt.merge_alert.as_ref().unwrap().unmerged_count, 1);
            }
        }
    }
}

#[test]
fn cas_e48d_target_merged_during_ci_probe_suppresses_relay() {
    let mut f = Fixture::new();
    let tip = git(f.repo.path(), &["rev-parse", "factory/merge-w"]);
    crate::test_paths::warm_stub(
        &f.app.cas_dir.join("gh-fixture"),
        &format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{{\"nameWithOwner\":\"acme/cas\"}}'; else git --git-dir='{}' update-ref refs/heads/epic/test '{}' && cat .cas/ci.json; fi\n",
            f.remote.path().display(),
            tip,
        ),
    );

    let prompts = f.relay();
    assert_eq!(
        git(f.remote.path(), &["rev-parse", "epic/test"]),
        tip,
        "precondition: the merge was pushed while GitHub completed the CI probe"
    );
    assert!(
        prompts.is_empty(),
        "CI completion must not emit a merge demand against the pre-CI target: {prompts:?}"
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
    // After READY, worktree_merge re-reads CI and refuses a same-SHA red run;
    // the director must stop probing that already-signalled delivery.
    assert!(f.relay().is_empty());
    std::fs::write(f.repo.path().join("delivery.rs"), "// new red delivery\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "new red delivery"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    let decision = f.relay();
    assert_eq!(
        decision.len(),
        1,
        "a new red delivery needs one decision"
    );
    assert!(decision[0].text.contains("CI failed"));
    assert!(f.relay().is_empty());
    // A parked failure still needs the slow same-SHA rerun probe; only a
    // successful READY may suppress this tip indefinitely.
    f.app.merge_alert_retries.now = Some(Utc::now() + chrono::Duration::minutes(10));
    f.ci(Some("success"));
    let recovered = f.relay();
    assert_eq!(recovered.len(), 1);
    // pin: this heading is the supervisor's immediate merge trigger.
    assert!(recovered[0].text.contains("READY TO MERGE"));
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
    assert_eq!(prompts.len(), 1, "missing CI");
    assert!(!prompts[0].is_merge_ready());
    assert!(
        prompts[0].text.contains("CI unknown"),
        "{}",
        prompts[0].text
    );
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
    let before_minute = now + chrono::Duration::seconds(59);
    assert!(f.app.merge_alert_retry_events_at(before_minute).is_empty());
    let retries = f
        .app
        .merge_alert_retry_events_at(now + chrono::Duration::minutes(1));
    assert_eq!(
        retries.len(),
        1,
        "the consumed WorkerIdle event must be retried each minute"
    );
    let at_minute = now + chrono::Duration::minutes(1);
    assert!(f.app.merge_alert_retry_events_at(at_minute).is_empty());
    f.ci(Some("success"));
    f.app.merge_alert_retries.now = Some(at_minute);
    let (_, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&retries);
    assert_eq!(prompts.len(), 1);
}

#[test]
fn cas_9efb_missing_ci_retry_survives_an_unchanged_director_snapshot() {
    let mut f = Fixture::new();
    let load = f.app.director_refresh_request().read(None);
    f.app.apply_director_refresh(load).unwrap();
    let now = Utc::now();
    f.ci(None);
    f.app.merge_alert_retries.now = Some(now);
    assert!(f.relay().is_empty());
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(1));
    let load = f.app.director_refresh_request().read(None);
    let events = f.app.apply_director_refresh(load).unwrap();
    assert!(
        events
            .iter()
            .any(|event| super::merge_alert::task_id(event) == Some("cas-9efb-test"))
    );
    f.ci(Some("success"));
    let (_, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&events);
    assert_eq!(prompts.len(), 1);
}

#[test]
fn cas_9efb_docs_only_and_cancelled_ci_have_terminal_decisions() {
    let mut f = Fixture::new();
    git(f.repo.path(), &["reset", "--hard", "epic/test"]);
    std::fs::write(f.repo.path().join("README.md"), "Documentation delivery\n").unwrap();
    git(f.repo.path(), &["add", "README.md"]);
    git(f.repo.path(), &["commit", "-qm", "docs delivery"]);
    git(f.repo.path(), &["push", "-q", "--force", "origin", "factory/merge-w"]);
    std::fs::write(
        f.app.cas_dir.join("ci.json"),
        serde_json::json!({"check_runs": [{
            "name": "Docs Lint", "status": "completed", "conclusion": "success",
        }]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        f.relay().len(),
        1,
        "Docs Lint is sufficient for a docs-only delivery"
    );
    let now = Utc::now();
    f.app.merge_alert_retries.now = Some(now);
    // Same-SHA flips after READY are rechecked by worktree_merge, not by
    // repeated director probes. A new tip starts a new CI observation.
    std::fs::write(f.repo.path().join("README.md"), "Documentation correction\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "docs correction"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    f.ci(Some("cancelled"));
    assert!(
        f.relay().is_empty(),
        "cas-5132: cancelled-only CI is superseded/pending, never red"
    );
    let retry_at = now + chrono::Duration::minutes(1);
    let retries = f.app.merge_alert_retry_events_at(retry_at);
    assert_eq!(retries.len(), 1, "cancelled-only CI must still be retried");
    f.app.merge_alert_retries.now = Some(retry_at);
    f.ci(Some("failure"));
    let (_, failed, _) = f.app.revalidate_and_prompt_for_delivery(&retries);
    assert_eq!(failed.len(), 1, "the later real failure needs a decision");
    assert!(!failed[0].is_merge_ready());
    assert!(failed[0].text.contains("CI failed"));
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
fn cas_65e8_active_target_fetch_failure_retries_without_ci_backoff() {
    let mut f = Fixture::new();
    f.in_progress();
    // Hide only the target ref: the source fetch and green CI still succeed.
    // This is a real upload-pack failure, not a mocked reader return value.
    git(f.remote.path(), &["config", "uploadpack.hideRefs", "refs/heads/epic/test"]);
    let deferred = f.relay();
    assert!(deferred.iter().all(|prompt| prompt.merge_alert.is_none()));
    git(f.remote.path(), &["config", "--unset", "uploadpack.hideRefs"]);
    let recovered = f.relay();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].merge_alert.is_some());
    assert!(f.relay().is_empty());
}

#[test]
fn cas_9efb_failed_target_fetch_defers_cached_positive_evidence() {
    let mut f = Fixture::new();
    let remote = f.remote.path().to_path_buf();
    let missing = f.repo.path().join("missing-remote");
    f.reroute_origin(&remote, &missing);
    assert!(
        f.relay().is_empty(),
        "failed fetch cannot call cached origin live"
    );
    f.reroute_origin(&missing, &remote);
    let mut recovered = f.relay();
    assert_eq!(
        recovered.len(),
        1,
        "a recovered probe still owes its first decision"
    );
    f.reroute_origin(&remote, &missing);
    assert!(
        !f.app.refresh_prompt_for_delivery(&mut recovered[0]),
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

/// cas-c926: the merge-alert retry re-emits a remembered WorkerIdle every
/// minute without passing the detector's hold gate, so a held worker with a
/// parked delivery kept producing idle/liveness relays until release.
#[test]
fn cas_c926_held_worker_gets_no_merge_retry_relay_until_release() {
    let mut f = Fixture::new();
    let now = Utc::now();
    f.ci(None);
    f.app.merge_alert_retries.now = Some(now);
    assert!(f.relay().is_empty(), "pending CI is remembered for retry");
    f.app.event_detector.mark_worker_hold("merge-w");
    f.ci(Some("success"));
    let minute = now + chrono::Duration::minutes(1);
    f.app.merge_alert_retries.now = Some(minute);
    let retries = f.app.merge_alert_retry_events_at(minute);
    let (events, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&retries);
    assert!(
        events.is_empty() && prompts.is_empty(),
        "a held worker gets no relay: {prompts:?}"
    );
    f.app.event_detector.clear_worker_hold("merge-w");
    // Release restores the merge relay.
    let (_, prompts, _) = f.app.revalidate_and_prompt_for_delivery(&retries);
    assert_eq!(prompts.len(), 1);
}

/// cas-43d2: a lane parked at a declined tip settles instead of costing a
/// fetch and a GitHub lookup on every retry until the worker commits.
#[test]
fn cas_43d2_declined_tip_settles_until_the_worker_commits() {
    let mut f = Fixture::new();
    f.in_progress();
    let store = crate::store::open_task_store_local(&f.app.cas_dir).unwrap();
    let mut task = store.get("cas-9efb-test").unwrap();
    let tip = git(f.repo.path(), &["rev-parse", "factory/merge-w"]);
    task.deliverables.declined_factory_branch_anchors = Some(vec![tip]);
    store.update(&task).unwrap();
    let trace = f.app.cas_dir.join("git-trace.log");
    f._env.set("GIT_TRACE", &trace);
    let probes = |f: &Fixture| {
        let fetches = std::fs::read_to_string(&trace).unwrap_or_default().lines()
            .filter(|line| line.contains("built-in: git fetch ")).count();
        fetches + std::fs::read_to_string(f.app.cas_dir.join("gh-requests")).unwrap_or_default().lines().count()
    };
    assert!(f.relay().iter().all(|prompt| prompt.merge_alert.is_none()));
    let settled = probes(&f);
    assert!(f.relay().iter().all(|prompt| prompt.merge_alert.is_none()));
    assert_eq!(probes(&f), settled, "a declined tip is not re-read");
    std::fs::write(f.repo.path().join("delivery.rs"), "// corrected\n").unwrap();
    git(f.repo.path(), &["commit", "-qam", "cas-9efb-test correction"]);
    git(f.repo.path(), &["push", "-q", "origin", "factory/merge-w"]);
    f.relay();
    assert!(probes(&f) > settled, "a new commit is read again");
}

/// cas-16e1: a refused pushed-tip check is re-read on the slow retry timer,
/// because the refusal can rest on state the read fingerprint does not watch.
#[test]
fn cas_16e1_not_ready_lane_is_reread_after_the_settled_retry() {
    let mut f = Fixture::new();
    f.in_progress();
    // Only origin carries the lane, so the worker's own branch lacks the tip.
    git(f.repo.path(), &["switch", "-q", "epic/test"]);
    git(f.repo.path(), &["branch", "-D", "factory/merge-w"]);
    let trace = f.app.cas_dir.join("git-trace.log");
    f._env.set("GIT_TRACE", &trace);
    let probes = |f: &Fixture| {
        let fetches = std::fs::read_to_string(&trace).unwrap_or_default().lines()
            .filter(|line| line.contains("built-in: git fetch ")).count();
        fetches + std::fs::read_to_string(f.app.cas_dir.join("gh-requests")).unwrap_or_default().lines().count()
    };
    let now = Utc::now();
    f.app.merge_alert_retries.now = Some(now);
    f.relay();
    let settled = probes(&f);
    assert!(settled > 0);
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(5));
    f.relay();
    assert_eq!(probes(&f), settled);
    f.app.merge_alert_retries.now = Some(now + chrono::Duration::minutes(11));
    f.relay();
    assert!(probes(&f) > settled);
}
