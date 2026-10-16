use std::sync::Mutex;

use super::*;

/// A registered project with a logged-in, cloud-linked store.
fn linked_repo(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("cloud.json"),
        r#"{"endpoint":"https://cloud.invalid","token":"store-token"}"#,
    )
    .unwrap();
    repo
}

#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<(PathBuf, PathBuf)>>,
    results: Mutex<BTreeMap<PathBuf, RunResult>>,
}

impl FakeRunner {
    fn answering(self, repo: &Path, result: RunResult) -> Self {
        self.results
            .lock()
            .unwrap()
            .insert(repo.to_path_buf(), result);
        self
    }

    fn calls(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls.lock().unwrap().clone()
    }
}

impl StoreSyncRunner for FakeRunner {
    fn run(&self, repo: &Path, cas_root: &Path, _: Duration, _: &AtomicBool) -> RunResult {
        self.calls
            .lock()
            .unwrap()
            .push((repo.to_path_buf(), cas_root.to_path_buf()));
        self.results
            .lock()
            .unwrap()
            .get(repo)
            .cloned()
            .unwrap_or(RunResult::Synced)
    }
}

fn sweep(
    config: &IdleSyncConfig,
    repos: &[PathBuf],
    runner: &dyn StoreSyncRunner,
    state: &Path,
    now: DateTime<Utc>,
) -> SweepReport {
    run_sweep(config, repos, runner, state, now, &AtomicBool::new(false)).unwrap()
}

#[test]
fn idle_cloud_linked_store_is_synced_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let repo = linked_repo(dir.path(), "alpha");
    let state = dir.path().join("hub").join(STATE_FILE);
    let runner = FakeRunner::default();
    let now = Utc::now();

    let report = sweep(
        &IdleSyncConfig::default(),
        &[repo.clone()],
        &runner,
        &state,
        now,
    );

    assert_eq!(report.ran, vec![(repo.clone(), SweepOutcome::Synced)]);
    assert_eq!(runner.calls(), vec![(repo.clone(), repo.join(".cas"))]);
    let saved = IdleSyncState::load(&state).unwrap();
    let record = saved.record(&repo).expect("the sweep records the store");
    assert_eq!(record.outcome, SweepOutcome::Synced);
    assert_eq!(record.last_success_at, Some(now));
    assert_eq!(record.next_attempt_after, None);
    assert_eq!(saved.last_sweep_at, Some(now));
}

#[test]
fn each_store_runs_bound_to_its_own_root_one_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = linked_repo(dir.path(), "alpha");
    let beta = linked_repo(dir.path(), "beta");
    let state = dir.path().join(STATE_FILE);
    let runner = FakeRunner::default();

    sweep(
        &IdleSyncConfig::default(),
        &[alpha.clone(), beta.clone()],
        &runner,
        &state,
        Utc::now(),
    );

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    for (repo, cas_root) in calls {
        assert_eq!(
            cas_root,
            repo.join(".cas"),
            "a store syncs under its own root"
        );
    }
}

#[cfg(unix)]
#[test]
fn store_with_a_live_serve_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let repo = linked_repo(dir.path(), "alpha");
    let _serve =
        std::os::unix::net::UnixListener::bind(crate::mcp::socket::socket_path(&repo.join(".cas")))
            .unwrap();
    let state = dir.path().join(STATE_FILE);
    let runner = FakeRunner::default();

    let report = sweep(
        &IdleSyncConfig::default(),
        &[repo.clone()],
        &runner,
        &state,
        Utc::now(),
    );

    assert!(
        runner.calls().is_empty(),
        "a live serve's store is not synced by the sweep"
    );
    assert_eq!(report.live, vec![repo.clone()]);
    assert_eq!(
        IdleSyncState::load(&state)
            .unwrap()
            .record(&repo)
            .unwrap()
            .outcome,
        SweepOutcome::LiveServe
    );
}

#[test]
fn unlinked_and_opted_out_stores_are_not_synced() {
    let dir = tempfile::tempdir().unwrap();
    let unlinked = dir.path().join("unlinked");
    std::fs::create_dir_all(unlinked.join(".cas")).unwrap();
    let opted_out = linked_repo(dir.path(), "opted-out");
    std::fs::write(
        opted_out.join(".cas").join("config.toml"),
        "[cloud]\nauto_sync = false\n",
    )
    .unwrap();
    let state = dir.path().join(STATE_FILE);
    let runner = FakeRunner::default();

    let report = sweep(
        &IdleSyncConfig::default(),
        &[unlinked.clone(), opted_out.clone()],
        &runner,
        &state,
        Utc::now(),
    );

    assert!(runner.calls().is_empty());
    assert!(report.ran.is_empty());
    assert!(IdleSyncState::load(&state).unwrap().stores.is_empty());
}

#[test]
fn sweep_cap_defers_the_rest_and_the_next_sweep_takes_the_least_recent() {
    let dir = tempfile::tempdir().unwrap();
    let repos: Vec<PathBuf> = ["a", "b", "c"]
        .into_iter()
        .map(|name| linked_repo(dir.path(), name))
        .collect();
    let state = dir.path().join(STATE_FILE);
    let config = IdleSyncConfig {
        max_stores_per_sweep: 2,
        ..IdleSyncConfig::default()
    };
    let runner = FakeRunner::default();
    let first_at = Utc::now();

    let first = sweep(&config, &repos, &runner, &state, first_at);
    assert_eq!(first.ran.len(), 2);
    assert_eq!(first.deferred, vec![repos[2].clone()]);

    let second = sweep(
        &config,
        &repos,
        &runner,
        &state,
        first_at + chrono::Duration::minutes(5),
    );
    assert_eq!(
        second.ran.first().map(|(repo, _)| repo),
        Some(&repos[2]),
        "the never-attempted store goes first next sweep"
    );
    assert_eq!(second.ran.len(), 2);
}

#[test]
fn failure_backs_off_until_its_next_attempt_and_success_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = linked_repo(dir.path(), "alpha");
    let state = dir.path().join(STATE_FILE);
    let config = IdleSyncConfig::default();
    let failing = FakeRunner::default().answering(&repo, RunResult::Failed("exit 1: boom".into()));
    let t0 = Utc::now();

    sweep(&config, &[repo.clone()], &failing, &state, t0);
    let record = IdleSyncState::load(&state)
        .unwrap()
        .record(&repo)
        .cloned()
        .unwrap();
    assert_eq!(record.outcome, SweepOutcome::Failed);
    assert_eq!(record.detail.as_deref(), Some("exit 1: boom"));
    assert_eq!(record.consecutive_failures, 1);
    let next = record
        .next_attempt_after
        .expect("a failure schedules a backoff");
    assert_eq!(
        next,
        t0 + chrono::Duration::from_std(config.backoff(1)).unwrap()
    );

    let early = sweep(
        &config,
        &[repo.clone()],
        &failing,
        &state,
        next - chrono::Duration::seconds(1),
    );
    assert_eq!(early.backing_off, vec![repo.clone()]);
    assert_eq!(
        failing.calls().len(),
        1,
        "no retry inside the backoff window"
    );

    let healthy = FakeRunner::default();
    sweep(&config, &[repo.clone()], &healthy, &state, next);
    let record = IdleSyncState::load(&state)
        .unwrap()
        .record(&repo)
        .cloned()
        .unwrap();
    assert_eq!(record.outcome, SweepOutcome::Synced);
    assert_eq!(record.consecutive_failures, 0);
    assert_eq!(record.next_attempt_after, None);
}

#[test]
fn backoff_doubles_and_is_capped() {
    let config = IdleSyncConfig::default();
    assert_eq!(config.backoff(1), Duration::from_secs(600));
    assert_eq!(config.backoff(2), Duration::from_secs(1200));
    assert_eq!(config.backoff(40), config.backoff_max);
}

#[test]
fn unregistered_stores_drop_out_of_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = linked_repo(dir.path(), "alpha");
    let beta = linked_repo(dir.path(), "beta");
    let state = dir.path().join(STATE_FILE);
    let runner = FakeRunner::default();
    sweep(
        &IdleSyncConfig::default(),
        &[alpha.clone(), beta.clone()],
        &runner,
        &state,
        Utc::now(),
    );

    sweep(
        &IdleSyncConfig::default(),
        &[alpha.clone()],
        &runner,
        &state,
        Utc::now(),
    );

    let saved = IdleSyncState::load(&state).unwrap();
    assert!(saved.record(&alpha).is_some());
    assert!(saved.record(&beta).is_none());
}

#[cfg(unix)]
mod process_runner {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A stand-in `cas` binary: records its environment, cwd and arguments
    /// in the store, then runs `body`.
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-cas");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nenv > \"$CAS_ROOT/child-env\"\npwd -P > \"$CAS_ROOT/child-cwd\"\necho \"$@\" > \"$CAS_ROOT/child-args\"\n{body}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(binary: &Path, repo: &Path, timeout: Duration) -> RunResult {
        ProcessSyncRunner::new(binary).run(
            repo,
            &repo.join(".cas"),
            timeout,
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn child_is_bound_to_the_store_with_a_cleared_environment() {
        let dir = tempfile::tempdir().unwrap();
        let repo = linked_repo(dir.path(), "alpha");
        let binary = script(dir.path(), "exit 0");

        assert_eq!(
            run(&binary, &repo, Duration::from_secs(30)),
            RunResult::Synced
        );

        let cas_root = repo.join(".cas");
        let args = std::fs::read_to_string(cas_root.join("child-args")).unwrap();
        assert_eq!(args.trim(), "cloud sync --background");
        let cwd = std::fs::read_to_string(cas_root.join("child-cwd")).unwrap();
        assert_eq!(Path::new(cwd.trim()), repo.canonicalize().unwrap());
        let env = std::fs::read_to_string(cas_root.join("child-env")).unwrap();
        let shell_added = ["PWD", "SHLVL", "_", "OLDPWD"];
        for line in env.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key == "CAS_ROOT" {
                assert_eq!(
                    Path::new(value),
                    cas_root,
                    "CAS_ROOT names the synced store"
                );
                continue;
            }
            assert!(
                ENV_ALLOWLIST.contains(&key) || shell_added.contains(&key),
                "{key} leaked from the hub into a store's sync"
            );
        }
        assert!(env.lines().any(|line| line.starts_with("CAS_ROOT=")));
    }

    #[test]
    fn lease_busy_exit_is_reported_as_busy() {
        let dir = tempfile::tempdir().unwrap();
        let repo = linked_repo(dir.path(), "alpha");
        let binary = script(dir.path(), &format!("exit {SYNC_LEASE_BUSY_EXIT}"));
        assert_eq!(
            run(&binary, &repo, Duration::from_secs(30)),
            RunResult::Busy
        );
    }

    #[test]
    fn failure_carries_the_last_output_line() {
        let dir = tempfile::tempdir().unwrap();
        let repo = linked_repo(dir.path(), "alpha");
        let binary = script(dir.path(), "echo 'Error: cloud said no' >&2\nexit 3");
        assert_eq!(
            run(&binary, &repo, Duration::from_secs(30)),
            RunResult::Failed("exit 3: Error: cloud said no".into())
        );
        let log =
            std::fs::read_to_string(repo.join(".cas").join("logs").join(STORE_LOG_FILE)).unwrap();
        assert!(
            log.contains("cloud said no"),
            "the store keeps the sync's output"
        );
    }

    #[test]
    fn a_sync_past_its_timeout_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = linked_repo(dir.path(), "alpha");
        let binary = script(dir.path(), "sleep 30");
        let started = Instant::now();
        assert_eq!(
            run(&binary, &repo, Duration::from_millis(500)),
            RunResult::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cancelling_kills_the_child_and_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = linked_repo(dir.path(), "alpha");
        let binary = script(dir.path(), "sleep 30");
        let state = dir.path().join(STATE_FILE);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();

        let report = run_sweep(
            &IdleSyncConfig::default(),
            &[repo.clone()],
            &ProcessSyncRunner::new(&binary),
            &state,
            Utc::now(),
            &cancel,
        )
        .unwrap();

        stopper.join().unwrap();
        assert!(report.cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(IdleSyncState::load(&state).unwrap().record(&repo).is_none());
    }
}
