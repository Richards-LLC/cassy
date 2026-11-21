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
