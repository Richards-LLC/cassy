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
    let fixture = Fixture::new();
    let left = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    let right = Snapshot::read(&fixture.right, "cas-6456").unwrap();
    let old_authority = left.tip.clone();
    let left_head = fixture.candidate(&fixture.left, "left-candidate");
    let right_head = fixture.candidate(&fixture.right, "right-candidate");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let run = |snapshot: Snapshot, repo: &Path, head: &str| {
            let barrier = barrier.clone();
            let actor = &fixture.actor;
            scope.spawn(move || {
                barrier.wait();
                snapshot.publish(&GitOperations::new(repo.to_path_buf()), actor, head)
            })
        };
        let a = run(left, &fixture.left, &left_head);
        let b = run(right, &fixture.right, &right_head);
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|r| r.as_ref().is_ok_and(|p| p.is_published())).count(), 1, "{results:?}");
    let authority = Snapshot::read(&fixture.left, "cas-6456").unwrap();
    assert_ne!(authority.tip, old_authority);
    assert_eq!(authority.record.target_sha, fixture.target());
    assert!([left_head, right_head].contains(&fixture.target()));
}
