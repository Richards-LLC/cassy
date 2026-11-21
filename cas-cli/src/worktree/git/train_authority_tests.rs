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
        let env = crate::test_support::TestEnvGuard::temp_home();
        let origin = env.home().join("origin.git");
        let left = env.home().join("left");
        let right = env.home().join("right");
        command(env.home(), &["init", "--bare", "--initial-branch=main", origin.to_str().unwrap()], None).unwrap();
        command(env.home(), &["clone", origin.to_str().unwrap(), left.to_str().unwrap()], None).unwrap();
        std::fs::write(left.join("base"), "base\n").unwrap();
        command(&left, &["add", "."], None).unwrap();
        command(&left, &["commit", "-m", "fixture"], None).unwrap();
        command(&left, &["push", "origin", "HEAD:refs/heads/main", "HEAD:refs/heads/epic/release-train-test-cas-6456"], None).unwrap();
        let base = command(&left, &["rev-parse", "HEAD"], None).unwrap();
        let actor = Identity { owner: "owner-a".into(), incarnation: "session-a/process-a".into() };
        let record = Record {
            protocol: 1, project: "fixture".into(), train: "cas-6456".into(),
            target: "epic/release-train-test-cas-6456".into(), identity: actor.clone(),
            epoch: 1, generation: "generation-a".into(), phase: Phase::Active,
            target_sha: base.clone(), operation: "enroll".into(), transfer: None,
        };
        let tip = record_commit(&left, None, &record).unwrap();
        command(&left, &["push", "origin", &format!("{tip}:{}", reference("cas-6456").unwrap())], None).unwrap();
        command(env.home(), &["clone", origin.to_str().unwrap(), right.to_str().unwrap()], None).unwrap();
        Self { _env: env, origin, left, right, actor, base }
    }

    fn candidate(&self, repo: &Path, name: &str) -> String {
        std::fs::write(repo.join(name), name).unwrap();
        command(repo, &["add", name], None).unwrap();
        command(repo, &["commit", "-m", name], None).unwrap();
        command(repo, &["rev-parse", "HEAD"], None).unwrap()
    }

    fn target(&self) -> String {
        command(&self.origin, &["rev-parse", "refs/heads/epic/release-train-test-cas-6456"], None).unwrap()
    }
}

#[test]
fn train_authority_atomic_publication_cas_6456() {
    // A rejected authority update or missing server capability must never
    // leave the product target advanced on its own.
    for rejection in ["authority-update", "atomic-capability"] {
        let fixture = Fixture::new();
        let admitted = Snapshot::read(&fixture.left, "cas-6456").unwrap();
        let candidate = fixture.candidate(&fixture.left, "refused-pair");
        if rejection == "atomic-capability" {
            command(&fixture.origin, &["config", "receive.advertiseAtomic", "false"], None).unwrap();
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let hook = fixture.origin.join("hooks/update");
                std::fs::write(&hook, "#!/bin/sh\ncase \"$1\" in refs/cas/train-owner/*) exit 1;; esac\n").unwrap();
                std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            #[cfg(not(unix))]
            panic!("authority-update rejection needs an executable Git hook");
        }
        let outcome = admitted.publish(&GitOperations::new(fixture.left.clone()), &fixture.actor, &candidate);
        assert!(!outcome.as_ref().is_ok_and(|p| p.is_published()), "{rejection}: {outcome:?}");
        assert_eq!(fixture.target(), fixture.base);
        assert_eq!(Snapshot::read(&fixture.left, "cas-6456").unwrap().tip, admitted.tip);
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
    assert_eq!(results.iter().filter(|r| r.as_ref().is_ok_and(|p| p.is_published())).count(), 1, "{results:?}");
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
    // Another clone completes the transfer after A's admission, before its
    // delayed publication resumes. The remote state, not a roster, fences A.
    let mut next = admitted.record.clone();
    next.identity = Identity { owner: "owner-b".into(), incarnation: "session-b/process-b".into() };
    next.generation = "generation-b".into();
    next.epoch += 1;
    next.operation = "accept-transfer".into();
    let tip = record_commit(&fixture.right, Some(&admitted.tip), &next).unwrap();
    command(&fixture.right, &["push", "origin", &format!("{tip}:{}", admitted.reference)], None).unwrap();
    let error = admitted.publish(&GitOperations::new(fixture.left.clone()), &fixture.actor, &candidate).unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_OWNER_GENERATION_CHANGED"));
    assert_eq!(fixture.target(), fixture.base);
    assert_eq!(Snapshot::read(&fixture.left, "cas-6456").unwrap().tip, tip);
}

#[test]
fn train_authority_raw_writer_divergence_cas_6456() {
    let fixture = Fixture::new();
    let admitted = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let candidate = fixture.candidate(&fixture.left, "owner-candidate");
    let raw = fixture.candidate(&fixture.right, "raw-writer");
    command(&fixture.right, &["push", "origin", &format!("{raw}:refs/heads/{}", admitted.record.target)], None).unwrap();
    let error = admitted.publish(&GitOperations::new(fixture.left.clone()), &fixture.actor, &candidate).unwrap_err();
    assert_eq!(error.split(':').next(), Some("TRAIN_TRANSFER_TARGET_DIVERGED"));
    assert_eq!(fixture.target(), raw);
    assert_eq!(Snapshot::read(&fixture.left, "cas-6456").unwrap().tip, admitted.tip);
}
