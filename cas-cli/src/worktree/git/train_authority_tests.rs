use super::*;

struct Fixture {
    _env: crate::test_support::TestEnvGuard,
    origin: std::path::PathBuf,
    left: std::path::PathBuf,
    right: std::path::PathBuf,
    actor: Identity,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self::unenrolled();
        Snapshot::enroll(
            &fixture.left,
            "fixture",
            "cas-6456",
            "epic/release-train-test-cas-6456",
            &fixture.actor,
            &fixture.base,
            Some(&fixture.base),
            &Settled::default(),
        )
        .unwrap();
        fixture
    }

    fn unenrolled() -> Self {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let origin = env.home().join("origin.git");
        let left = env.home().join("left");
        let right = env.home().join("right");
        command(
            env.home(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                origin.to_str().unwrap(),
            ],
            None,
        )
        .unwrap();
        command(
            env.home(),
            &["clone", origin.to_str().unwrap(), left.to_str().unwrap()],
            None,
        )
        .unwrap();
        std::fs::write(left.join("base"), "base\n").unwrap();
        command(&left, &["add", "."], None).unwrap();
        command(&left, &["commit", "-m", "fixture"], None).unwrap();
        command(
            &left,
            &[
                "push",
                "origin",
                "HEAD:refs/heads/main",
                "HEAD:refs/heads/epic/release-train-test-cas-6456",
            ],
            None,
        )
        .unwrap();
        let base = command(&left, &["rev-parse", "HEAD"], None).unwrap();
        let actor = crate::train_owner::identity(&Self::service_actor(), "owner-a").unwrap();
        command(
            env.home(),
            &["clone", origin.to_str().unwrap(), right.to_str().unwrap()],
            None,
        )
        .unwrap();
        Self {
            _env: env,
            origin,
            left,
            right,
            actor,
            base,
        }
    }

    fn service_actor() -> crate::types::Agent {
        let mut actor = crate::types::Agent::new_with_role(
            "owner-a".into(),
            "owner-a".into(),
            crate::types::AgentRole::Supervisor,
        );
        actor.machine_id = Some("fixture-machine".into());
        actor.factory_session = Some("session-a".into());
        actor.cc_session_id = Some("harness-a".into());
        actor.pid = Some(std::process::id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut actor, std::process::id());
        actor
    }

    fn candidate(&self, repo: &Path, name: &str) -> String {
        std::fs::write(repo.join(name), name).unwrap();
        command(repo, &["add", name], None).unwrap();
        command(repo, &["commit", "-m", name], None).unwrap();
        command(repo, &["rev-parse", "HEAD"], None).unwrap()
    }

    fn target(&self) -> String {
        command(
            &self.origin,
            &["rev-parse", "refs/heads/epic/release-train-test-cas-6456"],
            None,
        )
        .unwrap()
    }
}

#[test]
fn train_authority_atomic_publication_cas_6456() {
    // Enrollment cannot bless a changed target, bypass unknown coverage or
    // silently replace an existing authority record.
    {
        let fixture = Fixture::unenrolled();
        let checks = Settled::default();
        checks.pending.set(true);
        let error = Snapshot::enroll(
            &fixture.left,
            "fixture",
            "cas-6456",
            "epic/release-train-test-cas-6456",
            &fixture.actor,
            &fixture.base,
            Some(&fixture.base),
            &checks,
        )
        .unwrap_err();
        assert_eq!(
            error.split(':').next(),
            Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING")
        );
        assert!(
            command(
                &fixture.origin,
                &["show-ref", "--verify", &reference("cas-6456").unwrap()],
                None
            )
            .is_err()
        );
        checks.pending.set(false);
        let enrolled = Snapshot::enroll(
            &fixture.left,
            "fixture",
            "cas-6456",
            "epic/release-train-test-cas-6456",
            &fixture.actor,
            &fixture.base,
            Some(&fixture.base),
            &checks,
        )
        .unwrap();
        let error = Snapshot::enroll(
            &fixture.right,
            "fixture",
            "cas-6456",
            "epic/release-train-test-cas-6456",
            &fixture.actor,
            &fixture.base,
            Some(&fixture.base),
            &checks,
        )
        .unwrap_err();
        assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_CONFLICT"));
        assert_eq!(
            Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
            enrolled.tip
        );
        assert_eq!(fixture.target(), fixture.base);
        let created = Snapshot::enroll(
            &fixture.left,
            "fixture",
            "cas-6457",
            "epic/new-train-cas-6457",
            &fixture.actor,
            &fixture.base,
            None,
            &checks,
        )
        .unwrap();
        assert_eq!(created.record.target_sha, fixture.base);
        assert_eq!(
            command(
                &fixture.origin,
                &["rev-parse", "refs/heads/epic/new-train-cas-6457"],
                None
            )
            .unwrap(),
            fixture.base
        );
    }
    // A rejected authority update or missing server capability must never
    // leave the product target advanced on its own.
    for rejection in ["authority-update", "atomic-capability"] {
        let fixture = Fixture::new();
        let admitted = Snapshot::read(&fixture.left, "cas-6456").unwrap();
        let candidate = fixture.candidate(&fixture.left, "refused-pair");
        if rejection == "atomic-capability" {
            command(
                &fixture.origin,
                &["config", "receive.advertiseAtomic", "false"],
                None,
            )
            .unwrap();
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let hook = fixture.origin.join("hooks/update");
                std::fs::write(
                    &hook,
                    "#!/bin/sh\ncase \"$1\" in refs/cas/train-owner/*) exit 1;; esac\n",
                )
                .unwrap();
                std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            #[cfg(not(unix))]
            panic!("authority-update rejection needs an executable Git hook");
        }
        let outcome = admitted.publish(
            &GitOperations::new(fixture.left.clone()),
            &fixture.actor,
            &candidate,
        );
        assert!(
            !outcome.as_ref().is_ok_and(|p| p.is_published()),
            "{rejection}: {outcome:?}"
        );
        assert_eq!(fixture.target(), fixture.base);
        assert_eq!(
            Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
            admitted.tip
        );
    }
    let fixture = Fixture::new();
    let left = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let right = Snapshot::read(&fixture.right, "cas-6456").unwrap();
    let old_authority = left.tip.clone();
    let left_head = fixture.candidate(&fixture.left, "left-candidate");
    let right_head = fixture.candidate(&fixture.right, "right-candidate");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let actor = &fixture.actor;
        let repo_a = &fixture.left;
        let repo_b = &fixture.right;
        let head_a = &left_head;
        let head_b = &right_head;
        let start = barrier.clone();
        let a = scope.spawn(move || {
            start.wait();
            left.publish(&GitOperations::new(repo_a.clone()), actor, head_a)
        });
        let b = scope.spawn(move || {
            barrier.wait();
            right.publish(&GitOperations::new(repo_b.clone()), actor, head_b)
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_ref().is_ok_and(|p| p.is_published()))
            .count(),
        1,
        "{results:?}"
    );
    let authority = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    assert_ne!(authority.tip, old_authority);
    assert_eq!(authority.record.target_sha, fixture.target());
    assert!([left_head, right_head].contains(&fixture.target()));
}

#[test]
fn train_authority_stale_owner_publication_cas_6456() {
    let fixture = Fixture::new();
    let admitted = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let candidate = fixture.candidate(&fixture.left, "old-owner-candidate");
    let root = crate::store::init_cas_dir(&fixture.left).unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id = \"fixture\"\n",
    )
    .unwrap();
    let mut train = crate::types::Task::new("cas-6456".into(), "Train".into());
    train.task_type = crate::types::TaskType::Epic;
    train.labels = vec!["release-train".into(), crate::train_owner::ENROLLED.into()];
    train.branch = Some(admitted.record.target.clone());
    train.epic_verification_owner = Some(fixture.actor.owner.clone());
    let actor = Fixture::service_actor();
    assert_eq!(
        crate::factory_session_scope::train_owner_generation(&root, &train).unwrap(),
        admitted.record.generation
    );
    assert_eq!(
        crate::factory_session_scope::require_train_owner(&root, &train, &actor).unwrap(),
        admitted.record.generation
    );
    assert!(
        !crate::train_owner::generation_matches(&root, &train, "", &admitted.record.generation)
            .unwrap()
    );
    assert!(
        !crate::train_owner::generation_matches(&root, &train, "old", &admitted.record.generation)
            .unwrap()
    );
    let mut worker = actor.clone();
    worker.role = crate::types::AgentRole::Worker;
    assert!(crate::factory_session_scope::require_train_owner(&root, &train, &worker).is_err());
    let mut reused = actor.clone();
    reused.pid_starttime = Some(actor.pid_starttime.unwrap() + 1);
    let error =
        crate::factory_session_scope::require_train_owner(&root, &train, &reused).unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_OWNER_GENERATION_CHANGED")
    );
    // Another clone completes the transfer after A's admission, before its
    // delayed publication resumes. The remote state, not a roster, fences A.
    let agents = crate::store::open_agent_store(&root).unwrap();
    agents.register(&actor).unwrap();
    agents.update(&actor).unwrap();
    let mut replacement = actor.clone();
    replacement.id = "owner-a-restarted".into();
    replacement.cc_session_id = Some("harness-a-restarted".into());
    replacement.registered_at = chrono::Utc::now();
    agents.register(&replacement).unwrap();
    agents.update(&replacement).unwrap();
    // Registration intentionally retires same-name predecessors. Restore the
    // active row to replay the separate two-live-registrations conflict case.
    agents.update(&actor).unwrap();
    // A matching name/session cannot retire a still-live predecessor.
    assert!(!crate::cloud::peer_mailbox::owns_recipient(
        &root,
        &replacement,
        &actor.id
    ));
    let mut retired = actor.clone();
    retired.status = crate::types::AgentStatus::Shutdown;
    agents.update(&retired).unwrap();
    // A different machine still cannot inherit the old mailbox.
    replacement.machine_id = Some("unrelated-machine".into());
    agents.update(&replacement).unwrap();
    assert!(
        crate::factory_session_scope::require_train_owner(&root, &train, &replacement).is_err()
    );
    replacement.machine_id = actor.machine_id.clone();
    agents.update(&replacement).unwrap();
    assert!(crate::cloud::peer_mailbox::owns_recipient(
        &root,
        &replacement,
        &actor.id
    ));
    assert_eq!(
        crate::factory_session_scope::require_train_owner(&root, &train, &replacement).unwrap(),
        admitted.record.generation
    );
    let restarted = crate::train_owner::identity(&replacement, &fixture.actor.owner).unwrap();
    let refreshed = Snapshot::read(&fixture.right, "cas-6456").unwrap();
    assert_eq!(refreshed.record.identity, restarted);
    assert_eq!(refreshed.record.generation, admitted.record.generation);
    let error = admitted
        .publish(
            &GitOperations::new(fixture.left.clone()),
            &fixture.actor,
            &candidate,
        )
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_OWNER_GENERATION_CHANGED")
    );
    let successor = Identity {
        owner: "owner-b".into(),
        incarnation: "session-b/process-b".into(),
    };
    let settled = Settled::default();
    let draining = refreshed
        .prepare(&fixture.right, &restarted, &successor, "transfer-stale")
        .unwrap();
    let error = crate::train_owner::require_active(&root, &train).unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_OWNER_DRAINING"));
    let quiesced = draining
        .quiesce(&fixture.right, &restarted, &settled)
        .unwrap();
    let accepted = quiesced
        .accept(
            &fixture.right,
            &successor,
            "transfer-stale",
            &admitted.record.generation,
            &settled,
        )
        .unwrap();
    // Replay remote accept followed by a crash before the local projection.
    // Verified Git history repairs it before projected-owner admission checks.
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    tasks.add(&train).unwrap();
    let original = tasks.get(&train.id).unwrap();
    assert!(crate::train_owner::repair_projection(&root, tasks.as_ref(), &original, &admitted).is_err());
    let mut incoming = original.clone();
    incoming.epic_verification_owner = Some(successor.owner.clone());
    let repaired = crate::train_owner::project_synced_owner(&root, tasks.as_ref(), &incoming, &original).unwrap();
    assert_eq!(repaired.epic_verification_owner.as_deref(), Some(successor.owner.as_str()));
    assert_eq!(tasks.get_execution_state(&train.id).unwrap().unwrap()["train_owner"]["generation"], accepted.record.generation);
    let repeated = crate::train_owner::project_synced_owner(&root, tasks.as_ref(), &incoming, &repaired).unwrap();
    assert_eq!(repeated.updated_at, repaired.updated_at);
    assert!(crate::train_owner::project_synced_owner(&root, tasks.as_ref(), &original, &repaired).is_err());
    assert_eq!(tasks.get(&train.id).unwrap().epic_verification_owner, repaired.epic_verification_owner);
    let tip = accepted.tip;
    let error = admitted
        .publish(
            &GitOperations::new(fixture.left.clone()),
            &fixture.actor,
            &candidate,
        )
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_OWNER_GENERATION_CHANGED")
    );
    assert_eq!(fixture.target(), fixture.base);
    assert_eq!(Snapshot::read(&fixture.left, "cas-6456").unwrap().tip, tip);
}

#[test]
fn train_authority_raw_writer_divergence_cas_6456() {
    let fixture = Fixture::new();
    let admitted = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let candidate = fixture.candidate(&fixture.left, "owner-candidate");
    let raw = fixture.candidate(&fixture.right, "raw-writer");
    command(
        &fixture.right,
        &[
            "push",
            "origin",
            &format!("{raw}:refs/heads/{}", admitted.record.target),
        ],
        None,
    )
    .unwrap();
    let error = admitted
        .publish(
            &GitOperations::new(fixture.left.clone()),
            &fixture.actor,
            &candidate,
        )
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_TARGET_DIVERGED")
    );
    assert_eq!(fixture.target(), raw);
    assert_eq!(
        Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
        admitted.tip
    );
    let settled = Settled::default();
    let claim = operator_claim();
    let successor = Identity {
        owner: "owner-b".into(),
        incarnation: "session-b/process-b".into(),
    };
    let error = admitted
        .force_quiesce(&fixture.left, &successor, "recovery", &claim, &settled)
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_TARGET_DIVERGED")
    );
    let error = admitted
        .adopt_target(&fixture.left, &fixture.base, &claim, &settled)
        .unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_CONFLICT"));
    settled.pending.set(true);
    let error = admitted
        .adopt_target(&fixture.left, &raw, &claim, &settled)
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING")
    );
    assert_eq!(
        Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
        admitted.tip
    );
    settled.pending.set(false);
    let adopted = admitted
        .adopt_target(&fixture.left, &raw, &claim, &settled)
        .unwrap();
    assert_eq!(adopted.record.phase, Phase::RecoveryHeld);
    assert_eq!(adopted.record.target_sha, raw);
    assert_ne!(adopted.record.generation, admitted.record.generation);
    assert!(adopted.record.operator_claim.is_some());
    let error = adopted
        .publish(
            &GitOperations::new(fixture.left.clone()),
            &fixture.actor,
            &candidate,
        )
        .unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_OWNER_DRAINING"));
    // Even a valid operator claim cannot waive an armed external publisher.
    settled.pending.set(true);
    let error = adopted
        .force_quiesce(&fixture.left, &successor, "recovery", &claim, &settled)
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING")
    );
    assert_eq!(
        Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
        adopted.tip
    );
    settled.pending.set(false);
    let quiesced = adopted
        .force_quiesce(&fixture.left, &successor, "recovery", &claim, &settled)
        .unwrap();
    let accepted = quiesced
        .accept(
            &fixture.right,
            &successor,
            "recovery",
            &adopted.record.generation,
            &settled,
        )
        .unwrap();
    assert_eq!(accepted.record.identity, successor);
    assert_eq!(fixture.target(), raw);
}

#[test]
fn train_transfer_service_authenticates_accept_and_rechecks_provider_cas_6456() {
    use cas_mcp::types::{TrainOwnerTransferAction as Action, TrainOwnerTransferRequest as Request};
    use serde_json::{Value, json};
    use std::{cell::{Cell, RefCell}, rc::Rc};

    struct Provider { state: Rc<RefCell<Value>>, reads: Rc<Cell<usize>> }
    impl crate::train_owner::provider::Provider for Provider {
        fn prs(&self, branch: &str, _: Option<&str>) -> Result<Value, String> {
            self.reads.set(self.reads.get() + 1);
            let state = self.state.borrow();
            if state["error"] == true { return Err("read failed".into()); }
            Ok(json!({"data": {"repository": {"nameWithOwner": "test/fixture", "pullRequests": {
                "nodes": [{"number": 1, "state": "OPEN", "headRefName": branch,
                    "autoMergeRequest": state["autoMergeRequest"], "mergeQueueEntry": state["mergeQueueEntry"]}],
                "pageInfo": {"hasNextPage": false, "endCursor": null}
            }}}}))
        }
        fn runs(&self, _: &str) -> Result<Value, String> {
            self.reads.set(self.reads.get() + 1);
            let active = self.state.borrow()["active"] == true;
            Ok(if active { json!({"total_count": 1, "workflow_runs": [{"id": 44}]}) }
                else { json!({"total_count": 0, "workflow_runs": []}) })
        }
    }
    let fixture = Fixture::unenrolled();
    let root = crate::store::init_cas_dir(&fixture.left).unwrap();
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='fixture'\n").unwrap();
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    let agents = core.open_agent_store().unwrap();
    let actor = Fixture::service_actor();
    let mut successor = actor.clone();
    successor.id = "owner-b".into(); successor.name = "owner-b".into();
    successor.factory_session = Some("session-b".into()); successor.cc_session_id = Some("harness-b".into());
    for agent in [&actor, &successor] { agents.register(agent).unwrap(); agents.update(agent).unwrap(); }
    let successor_identity = crate::train_owner::identity(&successor, &successor.id).unwrap();
    let target = "epic/release-train-test-cas-6456".to_string();
    let tasks = core.open_task_store().unwrap();
    let mut train = crate::types::Task::new("cas-6456".into(), "train".into());
    train.task_type = crate::types::TaskType::Epic;
    train.branch = Some(target.clone());
    train.labels = vec!["release-train".into()];
    train.epic_verification_owner = Some(actor.id.clone());
    tasks.add(&train).unwrap();
    let state = Rc::new(RefCell::new(json!({})));
    let reads = Rc::new(Cell::new(0));
    let checks = crate::train_owner::provider::fixture_checks(Provider { state: state.clone(), reads: reads.clone() });
    core.set_agent_id_for_testing(actor.id.clone());
    // The first prepare enrolls under the legacy owner. A stale expectation
    // refuses before any Git write and names the current CAS values.
    let mut prepare = Request { operation: Action::Prepare, expected_authority: "unenrolled".into(),
        expected_generation: "stale".into(), transfer_id: "service-transfer".into(),
        request_id: uuid::Uuid::new_v4().to_string(), reason: "move publisher".into(), successor: Some(successor.id.clone()) };
    let stale = core.train_owner_transfer_checked(&train, &prepare, &checks).unwrap_err();
    assert!(Snapshot::read_optional(&fixture.left, &train.id).unwrap().is_none());
    let legacy = crate::factory_session_scope::train_owner_generation(&root, &train).unwrap();
    assert!(stale.contains(&legacy), "{stale}");
    prepare.expected_generation = legacy;
    let prepared = core.train_owner_transfer_checked(&train, &prepare, &checks).unwrap();
    let original = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!((original.record.phase.clone(), original.record.target_sha.clone()), (Phase::Draining, fixture.base.clone()));
    assert_eq!(prepared["authority"], original.tip);
    assert!(tasks.get(&train.id).unwrap().labels.iter().any(|label| label == crate::train_owner::ENROLLED));
    // Draining already fences the prior owner's own publication; the
    // production quiescence evidence is that fence, not a heartbeat.
    assert!(original.publish(&GitOperations::new(fixture.left.clone()), &fixture.actor, &fixture.base).is_err());
    let quiesce = Request { operation: Action::Quiesce, expected_authority: original.tip.clone(),
        expected_generation: original.record.generation.clone(), successor: None, ..prepare.clone() };
    core.train_owner_transfer_checked(&train, &quiesce, &checks).unwrap();
    let quiesced = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!(quiesced.record.quiescence.as_ref().unwrap()["publishers"]["fence"], "atomic-authority-lease");
    let request = Request { operation: Action::Accept, expected_authority: quiesced.tip.clone(), ..quiesce.clone() };
    // A registered supervisor who is not the named successor cannot accept,
    // and cannot choose its own fingerprint in the request.
    let before = reads.get();
    assert!(core.train_owner_transfer_checked(&train, &request, &checks).is_err());
    assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, quiesced.tip);
    assert_eq!(reads.get(), before);
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    core.set_agent_id_for_testing(successor.id.clone());
    // Every unchanged-target accept must freshly reject armed PR, queue, run
    // and unknown/error states before changing authority or task projection.
    for pending in [json!({"autoMergeRequest": {"enabledAt": "now"}}),
        json!({"mergeQueueEntry": {"id": "queue"}}), json!({"active": true}), json!({"error": true})] {
        *state.borrow_mut() = pending;
        let error = core.train_owner_transfer_checked(&train, &request, &checks).unwrap_err();
        assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING"));
        assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, quiesced.tip);
        assert_eq!(tasks.get(&train.id).unwrap().epic_verification_owner, train.epic_verification_owner);
    }
    *state.borrow_mut() = json!({});
    let receipt = core.train_owner_transfer_checked(&train, &request, &checks).unwrap();
    let accepted = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!(accepted.record.identity, successor_identity);
    assert_eq!(receipt["authority"], accepted.tip);
    assert_eq!(accepted.record.audit.as_ref().unwrap()["request_id"], request.request_id);
    assert_eq!(accepted.record.audit.as_ref().unwrap()["actor"], successor.id);
    assert_ne!(accepted.record.generation, original.record.generation);
    assert_eq!(tasks.get(&train.id).unwrap().epic_verification_owner.as_deref(), Some(successor.id.as_str()));
    let clock = tasks.get(&train.id).unwrap().updated_at;
    // Simulate retry with the pre-accept authority and stale A projection.
    // Fresh provider reads precede idempotent repair as well.
    *state.borrow_mut() = json!({"autoMergeRequest": {"enabledAt": "later"}});
    assert!(core.train_owner_transfer_checked(&train, &request, &checks).is_err());
    *state.borrow_mut() = json!({});
    let before = reads.get();
    let retry = core.train_owner_transfer_checked(&train, &request, &checks).unwrap();
    assert!(reads.get() > before);
    assert_eq!(retry, receipt);
    assert_eq!(tasks.get(&train.id).unwrap().updated_at, clock);
    assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, accepted.tip);
    // After the transfer only the accepted owner's fence publishes: generic
    // pushes refuse, the prior owner cannot obtain a fence, and the daemon
    // may publish only for the exact live recorded process.
    let head = fixture.candidate(&fixture.left, "fenced-merge");
    let git = GitOperations::new(fixture.left.clone());
    let unfenced = git.publish_pinned_source_to_origin(&target, &head, TIMEOUT);
    assert!(crate::gates::codes_in(&format!("{unfenced:?}")).contains(&"G-113"), "{unfenced:?}");
    let prior = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    prior.set_agent_id_for_testing(actor.id.clone());
    assert!(crate::train_owner::publication_fence(&prior, &fixture.left, &target).is_err());
    let daemon = crate::train_owner::local_owner_fence(&root, &fixture.left, &target).unwrap().unwrap();
    assert_eq!(daemon.1, successor_identity);
    let (authority, owner) = crate::train_owner::publication_fence(&core, &fixture.left, &target).unwrap().unwrap();
    assert!(git.publish_merge_candidate(&target, &head, false, Some((&authority, &owner))).is_published());
    let published = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!((fixture.target(), published.record.target_sha), (head.clone(), head));
    assert_eq!(published.record.generation, accepted.record.generation);
}

#[test]
fn train_transfer_operator_recovery_requires_h41_and_exact_scope_cas_6456() {
    use crate::config::operator_policy::InvocationContext;
    use crate::train_owner::recovery::{Action, Request};
    use cas_mcp::types::{TrainOwnerTransferAction as TransferAction, TrainOwnerTransferRequest};
    // Exercise the actual CLI grammar; there is no operator identity/override argument.
    for operation in ["force", "adopt-target"] {
        let mut args = vec!["cas", "factory", "train", "owner-transfer", "cas-6456", operation,
            "--expected-authority", "authority", "--expected-generation", "generation",
            "--request-id", "request", "--reason", "reason", "--sha", "target",
            "--evidence", "evidence", "--stopped-or-fenced", "all prior publishers"];
        if operation == "force" { args.extend(["--transfer-id", "transfer", "--successor", "owner-b"]); }
        assert!(<crate::cli::Cli as clap::Parser>::try_parse_from(args.clone()).is_ok());
        let words = args[1..].iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(crate::config::operator_policy::operator_only_cas_invocation(&words).is_some());
        args.extend(["--supervisor-override"]);
        assert!(<crate::cli::Cli as clap::Parser>::try_parse_from(args).is_err());
    }
    let fixture = Fixture::new();
    let root = crate::store::init_cas_dir(&fixture.left).unwrap();
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='fixture'\n").unwrap();
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    let tasks = core.open_task_store().unwrap();
    let first = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let mut train = crate::types::Task::new("cas-6456".into(), "train".into());
    train.task_type = crate::types::TaskType::Epic;
    train.branch = Some(first.record.target.clone());
    train.labels = vec!["release-train".into(), crate::train_owner::ENROLLED.into()];
    train.epic_verification_owner = Some(fixture.actor.owner.clone());
    tasks.add(&train).unwrap();
    let agents = core.open_agent_store().unwrap();
    let mut successor = Fixture::service_actor();
    successor.id = "owner-b".into(); successor.name = "owner-b".into();
    successor.machine_id = Some(crate::types::Agent::get_or_generate_machine_id());
    agents.register(&successor).unwrap(); agents.update(&successor).unwrap();
    let mut request = Request { action: Action::Force { successor: successor.id.clone(), transfer_id: "operator-transfer".into() },
        expected_authority: first.tip.clone(), expected_generation: first.record.generation.clone(),
        expected_target: first.record.target_sha.clone(), request_id: uuid::Uuid::new_v4().to_string(),
        reason: "recover stopped publisher".into(), evidence: "credential revocation receipt".into(),
        stopped_or_fenced: "all prior hosts stopped and automation credentials revoked".into() };
    let operator = InvocationContext { stdin_is_terminal: true, stdout_is_terminal: true,
        ancestors: vec!["-bash".into()], ..Default::default() };
    let mut forbidden = Vec::new();
    for kind in 0..4 {
        let mut context = operator.clone();
        match kind {
            0 => { context.env_names.insert("CAS_AGENT_ROLE".into()); },
            1 => context.ancestors.insert(0, "codex exec".into()),
            2 => context.stdin_is_terminal = false,
            _ => context.cgroup = "0::/cas-worker-test.scope".into(),
        }
        forbidden.push(context);
    }
    let checks = Settled::default();
    for context in &forbidden {
        let error = core.train_owner_recover_checked(&train.id, &request, context, &checks).unwrap_err();
        assert!(crate::gates::codes_in(&error).contains(&"H-41"), "{error}");
        assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, first.tip);
        assert_eq!(checks.reads.get(), 0);
    }
    // The typed MCP surface cannot express operator-only recovery or provenance.
    for operation in ["force", "adopt_target"] {
        let value = serde_json::json!({"operation": operation, "expected_authority": first.tip,
            "expected_generation": first.record.generation, "transfer_id": "operator-transfer",
            "request_id": request.request_id, "reason": request.reason, "operator_claim": {"operator": "forged"}});
        assert!(serde_json::from_value::<TrainOwnerTransferRequest>(value).is_err());
    }
    checks.pending.set(true);
    assert!(core.train_owner_recover_checked(&train.id, &request, &operator, &checks).is_err());
    assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, first.tip);
    checks.pending.set(false);
    let evidence = std::mem::take(&mut request.evidence);
    assert!(core.train_owner_recover_checked(&train.id, &request, &operator, &checks).is_err());
    request.evidence = evidence;
    let before = checks.reads.get();
    let receipt = core.train_owner_recover_checked(&train.id, &request, &operator, &checks).unwrap();
    assert!(checks.reads.get() >= before + 2);
    let forced = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!(forced.record.phase, Phase::Quiesced);
    assert_eq!(forced.record.identity, fixture.actor);
    assert_eq!(receipt["operator_claim_trust"], "unauthenticated");
    assert!(forced.record.operator_claim.as_ref().unwrap().get("unauthenticated_operator_claim").is_some());
    assert_eq!(forced.record.audit.as_ref().unwrap()["request_id"], request.request_id);
    assert_eq!(tasks.get(&train.id).unwrap().epic_verification_owner.as_deref(), Some(fixture.actor.owner.as_str()));
    // A raw push makes force hold. Only adoption of its exact SHA clears divergence,
    // invalidating old generation/acceptance proof and leaving publication held.
    let raw = fixture.candidate(&fixture.right, "operator-raw-writer");
    command(&fixture.right, &["push", "origin", &format!("{raw}:refs/heads/{}", first.record.target)], None).unwrap();
    request.expected_authority = forced.tip.clone();
    request.expected_target = raw.clone();
    assert!(core.train_owner_recover_checked(&train.id, &request, &operator, &checks).is_err());
    request.action = Action::AdoptTarget;
    for context in &forbidden {
        let error = core.train_owner_recover_checked(&train.id, &request, context, &checks).unwrap_err();
        assert!(crate::gates::codes_in(&error).contains(&"H-41"), "{error}");
    }
    request.expected_target = first.record.target_sha.clone();
    assert!(core.train_owner_recover_checked(&train.id, &request, &operator, &checks).is_err());
    request.expected_target = raw.clone();
    checks.pending.set(true);
    assert!(core.train_owner_recover_checked(&train.id, &request, &operator, &checks).is_err());
    assert_eq!(Snapshot::read(&fixture.left, &train.id).unwrap().tip, forced.tip);
    checks.pending.set(false);
    core.train_owner_recover_checked(&train.id, &request, &operator, &checks).unwrap();
    let adopted = Snapshot::read(&fixture.left, &train.id).unwrap();
    assert_eq!(adopted.record.target_sha, raw);
    assert_eq!(adopted.record.phase, Phase::RecoveryHeld);
    assert_ne!(adopted.record.generation, first.record.generation);
    assert!(adopted.record.quiescence.is_none());
    assert!(adopted.record.transfer.is_none());
    assert!(adopted.publish(&GitOperations::new(fixture.left.clone()), &fixture.actor, &raw).is_err());
    let stale_accept = TrainOwnerTransferRequest { operation: TransferAction::Accept,
        expected_authority: forced.tip.clone(), expected_generation: first.record.generation.clone(),
        transfer_id: "operator-transfer".into(), request_id: uuid::Uuid::new_v4().to_string(),
        reason: "accept recovery".into(), successor: None };
    core.set_agent_id_for_testing(successor.id.clone());
    assert!(core.train_owner_transfer_checked(&train, &stale_accept, &checks).is_err());
    request.action = Action::Force { successor: successor.id.clone(), transfer_id: "recovery-after-adoption".into() };
    request.expected_authority = adopted.tip.clone();
    request.expected_generation = adopted.record.generation.clone();
    core.train_owner_recover_checked(&train.id, &request, &operator, &checks).unwrap();
    let quiesced = Snapshot::read(&fixture.left, &train.id).unwrap();
    let accept = TrainOwnerTransferRequest { expected_authority: quiesced.tip,
        expected_generation: adopted.record.generation, transfer_id: "recovery-after-adoption".into(), ..stale_accept };
    core.train_owner_transfer_checked(&tasks.get(&train.id).unwrap(), &accept, &checks).unwrap();
    assert_eq!(tasks.get(&train.id).unwrap().epic_verification_owner.as_deref(), Some(successor.id.as_str()));
    assert_eq!(fixture.target(), raw);
}

#[derive(Default)]
struct Settled {
    reads: std::cell::Cell<usize>,
    pending: std::cell::Cell<bool>,
}

impl TransferChecks for Settled {
    fn external_clear(&self, _: &Snapshot) -> Result<serde_json::Value, String> {
        self.reads.set(self.reads.get() + 1);
        if self.pending.get() {
            return Err("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: fixture has an armed or uncertain publisher".into());
        }
        Ok(serde_json::json!({"provider": "fixture", "prs": [], "runs": []}))
    }
    fn publishers_stopped(&self, _: &Snapshot) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"runtime": "fixture", "disposition": "fenced"}))
    }
}

#[test]
fn train_authority_abort_reaccept_cas_6456() {
    let fixture = Fixture::new();
    let first = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let successor = Identity {
        owner: "owner-b".into(),
        incarnation: "session-b/process-b".into(),
    };
    let settled = Settled::default();
    let draining = first
        .prepare(&fixture.left, &fixture.actor, &successor, "transfer-1")
        .unwrap();
    settled.pending.set(true);
    let error = draining
        .quiesce(&fixture.left, &fixture.actor, &settled)
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING")
    );
    assert_eq!(
        Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
        draining.tip
    );
    settled.pending.set(false);
    let quiesced = draining
        .quiesce(&fixture.left, &fixture.actor, &settled)
        .unwrap();
    let aborted = quiesced
        .abort(&fixture.left, &fixture.actor, &settled)
        .unwrap();
    assert_ne!(aborted.record.generation, first.record.generation);
    assert_eq!(aborted.record.phase, Phase::Active);
    // B's delayed accept from the other clone cannot revive the aborted id.
    let error = quiesced
        .accept(
            &fixture.right,
            &successor,
            "transfer-1",
            &first.record.generation,
            &settled,
        )
        .unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_CONFLICT"));
    assert_eq!(
        Snapshot::read(&fixture.right, "cas-6456").unwrap().tip,
        aborted.tip
    );
    let draining = aborted
        .prepare(&fixture.left, &fixture.actor, &successor, "transfer-2")
        .unwrap();
    let error = draining
        .prepare(&fixture.right, &fixture.actor, &fixture.actor, "transfer-2")
        .unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_CONFLICT"));
    let quiesced = draining
        .quiesce(&fixture.left, &fixture.actor, &settled)
        .unwrap();
    let accepted = quiesced
        .accept(
            &fixture.right,
            &successor,
            "transfer-2",
            &aborted.record.generation,
            &settled,
        )
        .unwrap();
    assert_ne!(accepted.record.generation, aborted.record.generation);
    // Projection repair reuses the accepted generation, but always rechecks
    // external publishers even when the target has never moved.
    settled.pending.set(true);
    let error = quiesced
        .accept(
            &fixture.left,
            &successor,
            "transfer-2",
            &aborted.record.generation,
            &settled,
        )
        .unwrap_err();
    assert_eq!(
        error.split(':').next(),
        Some("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING")
    );
    assert_eq!(
        Snapshot::read(&fixture.left, "cas-6456").unwrap().tip,
        accepted.tip
    );
    settled.pending.set(false);
    let observations = settled.reads.get();
    let repaired = quiesced
        .accept(
            &fixture.left,
            &successor,
            "transfer-2",
            &aborted.record.generation,
            &settled,
        )
        .unwrap();
    assert_eq!(repaired.tip, accepted.tip);
    assert!(settled.reads.get() >= observations + 2);
    let draining = accepted
        .prepare(&fixture.right, &successor, &fixture.actor, "transfer-3")
        .unwrap();
    let quiesced = draining
        .quiesce(&fixture.right, &successor, &settled)
        .unwrap();
    let returned = quiesced
        .accept(
            &fixture.left,
            &fixture.actor,
            "transfer-3",
            &accepted.record.generation,
            &settled,
        )
        .unwrap();
    assert_ne!(returned.record.generation, first.record.generation);
    assert_ne!(returned.record.generation, aborted.record.generation);
    assert_eq!(returned.record.identity, fixture.actor);
    assert_eq!(fixture.target(), fixture.base);
    assert_eq!(returned.record.target_sha, fixture.base);
    drop(fixture);

    let fixture = Fixture::new();
    let first = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let draining = first
        .prepare(&fixture.left, &fixture.actor, &successor, "race")
        .unwrap();
    let quiesced = draining
        .quiesce(&fixture.left, &fixture.actor, &settled)
        .unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            quiesced.abort(&fixture.left, &fixture.actor, &Settled::default())
        });
        let b = scope.spawn(|| {
            barrier.wait();
            quiesced.accept(
                &fixture.right,
                &successor,
                "race",
                &first.record.generation,
                &Settled::default(),
            )
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    let live = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    assert_eq!(
        live.tip,
        results.iter().find_map(|r| r.as_ref().ok()).unwrap().tip
    );
    assert_ne!(live.record.generation, first.record.generation);
    assert_eq!(live.record.phase, Phase::Active);
    assert_eq!(live.record.target_sha, fixture.base);
    assert_eq!(fixture.target(), fixture.base);
}

fn operator_claim() -> OperatorClaim {
    OperatorClaim {
        operator: "fixture-operator".into(),
        reason: "explicit recovery".into(),
        evidence: "fixture old runtime stopped".into(),
        stopped_or_fenced: "all old publication paths".into(),
    }
}
