//! Tests for the server registry (cas-7c93, GH #87).
//!
//! These drive real processes, because every claim the issue makes is about
//! process reality: that a registered-shared server outlives worker teardown,
//! that an unregistered one does not, and that a dead pid is never resurrected
//! or re-signalled.

use super::*;

fn spec(name: &str, command: &str, cwd: &Path, shared: bool) -> ServerSpec {
    ServerSpec {
        name: name.to_string(),
        command: command.to_string(),
        cwd: cwd.to_path_buf(),
        expected_port: None,
        owner_task: Some("cas-7c93".to_string()),
        owner_worker: Some("young-finch-81".to_string()),
        factory_session: Some("registry-test".to_string()),
        shared,
    }
}

fn wait_until_gone(pid: u32) -> bool {
    for _ in 0..80 {
        if !crate::mcp::daemon::pid_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

#[cfg(target_os = "linux")]
struct ProcessTreeGuard {
    pid: u32,
    armed: bool,
}

#[cfg(target_os = "linux")]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // `script` makes the workload a session/group leader. Validate that
        // relationship before group cleanup so a failed assertion can never
        // signal the test runner's group.
        let pgid = unsafe { libc::getpgid(self.pid as libc::pid_t) };
        if pgid == self.pid as libc::pid_t {
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        } else {
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

/// cas-44d2: reproduce the incident command shape, including a Cassy-like child
/// that survives TERM/HUP. `script` creates a new session for the workload;
/// stopping only its registered wrapper pid used to return success while this
/// child (and its own children) stayed alive.
#[cfg(target_os = "linux")]
#[test]
fn server_stop_reaps_script_wrapped_cas_factory_descendants() {
    use std::os::unix::fs::PermissionsExt;

    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipping: util-linux script is not installed");
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join("registry");
    std::fs::create_dir_all(&cas_root).unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake_cas = bin.join("cas");
    std::fs::write(
        &fake_cas,
        "#!/bin/sh\n\
         test \"$1 $2 $3 $4\" = \"factory --new -n cas-44d2-proof\" || exit 64\n\
         trap '' HUP TERM\n\
         printf '%s' \"$$\" > \"$CAS_44D2_CHILD_PID\"\n\
         while :; do sleep 300; done\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_cas).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cas, permissions).unwrap();

    let child_pid_file = temp.path().join("cas-factory.pid");
    let transcript = temp.path().join("typescript");
    let command = format!(
        "PATH='{}':\"$PATH\" CAS_44D2_CHILD_PID='{}' \
         script -q -c 'cas factory --new -n cas-44d2-proof' '{}'",
        bin.display(),
        child_pid_file.display(),
        transcript.display(),
    );
    let record = start(
        &cas_root,
        &spec("script-cas-factory", &command, temp.path(), false),
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let workload_pid = loop {
        if let Ok(contents) = std::fs::read_to_string(&child_pid_file)
            && let Ok(pid) = contents.trim().parse::<u32>()
        {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fake `cas factory` never published its pid; log: {:?}",
            record
                .log_path
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok())
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let mut workload_guard = ProcessTreeGuard {
        pid: workload_pid,
        armed: true,
    };

    assert_ne!(
        record.pid, workload_pid,
        "the fixture must include a wrapper"
    );
    assert_eq!(
        unsafe { libc::getpgid(workload_pid as libc::pid_t) },
        workload_pid as libc::pid_t,
        "precondition: util-linux script must put the workload in a new session/group"
    );
    assert!(crate::mcp::daemon::pid_alive(record.pid));
    assert!(crate::mcp::daemon::pid_alive(workload_pid));

    let outcome = stop(&cas_root, &record).unwrap();
    assert!(matches!(outcome, StopOutcome::Stopped { .. }));
    assert!(
        wait_until_gone(record.pid),
        "registered wrapper survived stop"
    );
    assert!(
        wait_until_gone(workload_pid),
        "server_stop returned success while script's `cas factory` descendant survived"
    );
    workload_guard.armed = false;
}

/// AC1: register, query, stop — the end-to-end shape a supervisor sees.
#[test]
fn start_records_ownership_then_list_and_stop_resolve_it() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join("cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let workdir = temp.path().join("project");
    std::fs::create_dir_all(&workdir).unwrap();

    let record = start(&cas_root, &spec("dev-server", "sleep 300", &workdir, false)).unwrap();

    assert_eq!(record.state, ServerState::Running);
    assert_eq!(record.owner_task.as_deref(), Some("cas-7c93"));
    assert_eq!(record.owner_worker.as_deref(), Some("young-finch-81"));
    assert_eq!(record.cwd, workdir);
    assert!(record.command.contains("sleep 300"));
    assert!(
        record.pid_starttime.is_some(),
        "a fingerprint is required or the entry can never be safely signalled"
    );
    assert_eq!(
        liveness(&record),
        ServerLiveness::Live,
        "the recorded pid must be the server itself, not the launcher shell"
    );

    // Queryable by id and by name.
    assert_eq!(
        find(&cas_root, &record.id).unwrap().unwrap().pid,
        record.pid
    );
    assert_eq!(
        find(&cas_root, "dev-server").unwrap().unwrap().id,
        record.id
    );
    let listed = list(&cas_root).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, record.id);

    let outcome = stop(&cas_root, &record).unwrap();
    assert!(
        matches!(outcome, StopOutcome::Stopped { pid, .. } if pid == record.pid),
        "unexpected stop outcome: {outcome:?}"
    );
    assert!(wait_until_gone(record.pid), "stop must actually kill it");

    let after = find(&cas_root, &record.id).unwrap().unwrap();
    assert_eq!(after.state, ServerState::Stopped);
    assert!(after.ended_at.is_some());
}

/// The server must outlive the tool call that started it — it is reparented,
/// not held as a child of the MCP process, and it must not be left a zombie
/// that `list` would misreport as running.
#[test]
fn started_server_is_reparented_and_never_a_zombie_child() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    let record = start(
        &cas_root,
        &spec("short-lived", "sleep 0.2", temp.path(), false),
    )
    .unwrap();

    assert!(wait_until_gone(record.pid) || !matches!(liveness(&record), ServerLiveness::Live));

    // A zombie still has a /proc entry with the original start time; liveness
    // must not call that alive, or refresh would never mark it dead.
    assert_ne!(liveness(&record), ServerLiveness::Live);

    let refreshed = refresh(&cas_root).unwrap();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].state, ServerState::Dead);
}

#[test]
fn stop_does_not_claim_a_legacy_wrapper_is_the_whole_workload() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    let mut record = start(
        &cas_root,
        &spec("legacy-gone", "sleep 0.02", temp.path(), false),
    )
    .unwrap();
    assert!(wait_until_gone(record.pid));

    // Records created before cas-44d2 have no dedicated scope. Once their
    // wrapper pid is gone, a detached child is no longer discoverable by
    // ancestry. Honest failure is the only provable outcome.
    if let Some(scope) = record.cgroup.take() {
        super::super::cgroup::remove_scope(&scope);
    }
    let error = stop(&cas_root, &record).unwrap_err();
    assert!(
        error.to_string().contains("cannot prove"),
        "server_stop must report unverifiable descendants, not success: {error}"
    );
}

/// AC: "dead pids are marked dead, not resurrected."
#[test]
fn refresh_marks_dead_pids_dead_and_never_resurrects_them() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    let mut record = start(&cas_root, &spec("gone", "sleep 300", temp.path(), false)).unwrap();
    stop(&cas_root, &record).unwrap();
    assert!(wait_until_gone(record.pid));

    // Simulate the record still claiming Running (e.g. the server was killed
    // by something outside Cassy) and let refresh reconcile it.
    record.state = ServerState::Running;
    record.ended_at = None;
    record.ended_detail = None;
    write_record(&cas_root, &record).unwrap();

    let refreshed = refresh(&cas_root).unwrap();
    assert_eq!(refreshed[0].state, ServerState::Dead);
    assert!(
        refreshed[0]
            .ended_detail
            .as_deref()
            .is_some_and(|d| d.contains("exited") || d.contains("reused")),
        "a dead entry must say why: {:?}",
        refreshed[0].ended_detail
    );

    // Now let the pid be "reused" by a live process. A terminal record must
    // stay terminal — this is the resurrection the issue forbids.
    let mut resurrected = refreshed[0].clone();
    resurrected.pid = std::process::id();
    resurrected.pid_starttime = crate::mcp::daemon::read_pid_starttime(std::process::id());
    write_record(&cas_root, &resurrected).unwrap();

    let again = refresh(&cas_root).unwrap();
    assert_eq!(
        again[0].state,
        ServerState::Dead,
        "a dead entry must never return to running because its pid is occupied again"
    );
}

/// The fingerprint is what stands between this registry and killing a
/// bystander that inherited a recycled pid. A mismatch must refuse, not kill.
#[test]
fn stop_refuses_to_signal_a_reused_pid() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    // A live, innocent process: this test runner.
    let mut record = start(&cas_root, &spec("victim", "sleep 300", temp.path(), false)).unwrap();
    let real_pid = record.pid;
    record.pid = std::process::id();
    // Deliberately wrong fingerprint for that pid.
    record.pid_starttime = Some(1);
    write_record(&cas_root, &record).unwrap();

    assert_eq!(liveness(&record), ServerLiveness::Replaced);
    let outcome = stop(&cas_root, &record).unwrap();
    assert_eq!(
        outcome,
        StopOutcome::RefusedUnverified(ServerLiveness::Replaced)
    );
    // Still here — the assertion below only runs because we were not killed.
    assert!(crate::mcp::daemon::pid_alive(std::process::id()));

    let after = find(&cas_root, &record.id).unwrap().unwrap();
    assert_eq!(after.state, ServerState::Dead);
    assert!(
        after
            .ended_detail
            .as_deref()
            .is_some_and(|d| d.contains("refused")),
        "the refusal must be legible in the record: {:?}",
        after.ended_detail
    );

    // Clean up the real server this test started.
    // SAFETY: pid from this test's own spawn moments ago.
    unsafe {
        libc::kill(real_pid as libc::pid_t, libc::SIGKILL);
    }
}

#[test]
fn start_rejects_an_empty_command_and_a_missing_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    let empty = start(&cas_root, &spec("nothing", "   ", temp.path(), false)).unwrap_err();
    assert_eq!(empty.kind(), io::ErrorKind::InvalidInput);

    let missing = start(
        &cas_root,
        &spec("nowhere", "sleep 1", &temp.path().join("absent"), false),
    )
    .unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    assert!(list(&cas_root).unwrap().is_empty());
}

/// Output must never reach the caller's stdio: the MCP server speaks protocol
/// there, and a dev server's banner would corrupt the stream.
#[test]
fn server_output_is_captured_to_a_log_not_inherited() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    let record = start(
        &cas_root,
        &spec(
            "chatty",
            "echo hello-from-server; sleep 0.1",
            temp.path(),
            false,
        ),
    )
    .unwrap();

    let log = record.log_path.clone().expect("a log path is recorded");
    let mut contents = String::new();
    for _ in 0..40 {
        contents = std::fs::read_to_string(&log).unwrap_or_default();
        if contents.contains("hello-from-server") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        contents.contains("hello-from-server"),
        "server output must land in {}: {contents:?}",
        log.display()
    );
}

#[test]
fn ids_and_log_names_cannot_escape_the_registry_directory() {
    assert_eq!(sanitize_component("../../etc/passwd"), "etc-passwd");
    assert_eq!(sanitize_component("dev server!"), "dev-server");
    assert!(generate_id("../evil", 42).starts_with("srv-evil-42-"));
    assert!(!generate_id("../evil", 42).contains('/'));
    // A name that sanitizes to nothing still yields a usable id.
    assert!(generate_id("///", 7).starts_with("srv-server-7-"));
}

#[test]
fn a_corrupt_record_does_not_blind_the_listing() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    let record = start(&cas_root, &spec("good", "sleep 300", temp.path(), false)).unwrap();
    std::fs::write(
        registry_dir(&cas_root).join("srv-broken.json"),
        "{ not json at all",
    )
    .unwrap();

    let listed = list(&cas_root).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, record.id);

    let _ = stop(&cas_root, &record);
}

#[cfg(target_os = "linux")]
#[test]
fn zombie_state_is_parsed_from_the_last_paren_not_the_first() {
    // comm can contain spaces and parens; a naive split mis-reads the state.
    assert!(parse_zombie_state("123 (my (weird) server) Z 1 1 1"));
    assert!(!parse_zombie_state("123 (my (weird) server) S 1 1 1"));
    assert!(parse_zombie_state("456 (node) Z 0 0"));
}

/// History ages out; a live entry never does, however old — forgetting a
/// running server would recreate the ambient orphan this registry replaces.
#[test]
fn prune_history_drops_old_terminal_entries_and_never_live_ones() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    let live = start(&cas_root, &spec("live", "sleep 300", temp.path(), false)).unwrap();
    let mut ancient_live = live.clone();
    ancient_live.id = "srv-ancient-live".to_string();
    ancient_live.started_at = Utc::now() - chrono::Duration::days(30);
    write_record(&cas_root, &ancient_live).unwrap();

    let mut ancient_dead = live.clone();
    ancient_dead.id = "srv-ancient-dead".to_string();
    ancient_dead.state = ServerState::Stopped;
    ancient_dead.ended_at = Some(Utc::now() - chrono::Duration::days(30));
    write_record(&cas_root, &ancient_dead).unwrap();

    let mut recent_dead = live.clone();
    recent_dead.id = "srv-recent-dead".to_string();
    recent_dead.state = ServerState::Dead;
    recent_dead.ended_at = Some(Utc::now());
    write_record(&cas_root, &recent_dead).unwrap();

    let all = list(&cas_root).unwrap();
    assert_eq!(prune_history(&cas_root, &all).unwrap(), 1);

    let remaining: Vec<_> = list(&cas_root).unwrap().into_iter().map(|r| r.id).collect();
    assert!(remaining.contains(&live.id));
    assert!(
        remaining.contains(&"srv-ancient-live".to_string()),
        "a record still claiming Running must never be pruned: {remaining:?}"
    );
    assert!(remaining.contains(&"srv-recent-dead".to_string()));
    assert!(!remaining.contains(&"srv-ancient-dead".to_string()));

    let _ = stop(&cas_root, &live);
}

#[test]
fn forget_removes_only_the_named_record_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    let a = start(&cas_root, &spec("a", "sleep 300", temp.path(), false)).unwrap();
    let b = start(&cas_root, &spec("b", "sleep 300", temp.path(), false)).unwrap();

    forget(&cas_root, &a.id).unwrap();
    forget(&cas_root, &a.id).unwrap();
    let listed = list(&cas_root).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, b.id);

    let _ = stop(&cas_root, &a);
    let _ = stop(&cas_root, &b);
}

// ---------------------------------------------------------------------------
// The headline claim (GH #87): a registered-shared server survives worker
// teardown; an unregistered process does not.
// ---------------------------------------------------------------------------

/// Process-group tier, the floor on every host. The caller of `start` stands in
/// for the worker: a shared server must leave the caller's process group (so
/// the `killpg` half of teardown misses it), a private one must stay in it (so
/// teardown still takes it down).
///
/// The escape is asserted structurally rather than by killing the caller's
/// group — that group contains the test runner.
#[cfg(unix)]
#[test]
fn shared_server_leaves_the_callers_process_group_and_a_private_one_stays() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    // SAFETY: read-only process-table query.
    let caller_pgid = unsafe { libc::getpgid(std::process::id() as libc::pid_t) } as u32;

    // Server names must be unique across this file's tests: shared scopes are
    // named cas-server-<session>-<name>, and create_named_scope adopts an
    // existing directory (restart semantics). Two parallel tests sharing a
    // name land in one scope, and whichever calls stop() first reaps both.
    let shared = start(
        &cas_root,
        &spec("pg-shared-srv", "sleep 300", temp.path(), true),
    )
    .unwrap();
    let private = start(
        &cas_root,
        &spec("pg-private-srv", "sleep 300", temp.path(), false),
    )
    .unwrap();

    assert_ne!(
        shared.pgid,
        Some(caller_pgid),
        "GH #87: a shared server must leave the caller's process group, or killpg \
         at teardown reaches it regardless of registration"
    );
    assert_eq!(
        private.pgid,
        Some(caller_pgid),
        "a private server must stay in the caller's group so it dies with it"
    );
    if let Some(own_scope) = super::super::cgroup::own_scope_for_test()
        && own_scope
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("cas-worker-"))
    {
        if let (Some(shared_scope), Some(private_scope)) =
            (shared.cgroup.as_ref(), private.cgroup.as_ref())
        {
            assert_eq!(
                shared_scope.parent(),
                own_scope.parent(),
                "shared server scope must be a worker sibling, not a child that teardown kills"
            );
            assert_eq!(
                private_scope.parent(),
                Some(own_scope.as_path()),
                "private server scope must stay under its worker so teardown still owns it"
            );
        }
    }

    // And the signal targets follow from that: the shared server's own group
    // may be signalled; the private one may only ever be signalled by pid,
    // because its group is the worker's.
    assert_eq!(
        signal_target(&shared),
        SignalTarget::ProcessGroup(shared.pgid.unwrap()),
        "a shared server's wrapper children must be reachable"
    );
    assert_eq!(
        signal_target(&private),
        SignalTarget::Pid(private.pid),
        "killpg on a private server would kill the worker that started it"
    );

    let _ = stop(&cas_root, &shared);
    let _ = stop(&cas_root, &private);
    assert!(wait_until_gone(shared.pid));
    assert!(wait_until_gone(private.pid));
}

/// A shared server whose `setsid` did not take (so it is still in the caller's
/// group) must never have its group signalled — the fallback is pid-only.
#[cfg(unix)]
#[test]
fn a_shared_server_still_in_the_callers_group_is_never_killpg_ed() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();
    // SAFETY: read-only process-table query.
    let caller_pgid = unsafe { libc::getpgid(std::process::id() as libc::pid_t) } as u32;

    let mut record = start(
        &cas_root,
        &spec("degraded", "sleep 300", temp.path(), false),
    )
    .unwrap();
    record.shared = true;
    record.pgid = Some(caller_pgid);

    assert_eq!(signal_target(&record), SignalTarget::Pid(record.pid));

    let _ = stop(&cas_root, &record);
}

/// cgroup tier: the one containment tier with no escape hatch. A shared server
/// must live in its *own* scope, so `cgroup.kill` on the worker's scope — the
/// cas-99f5 teardown — cannot reach it, while an unregistered process started
/// inside the worker's scope dies.
#[cfg(target_os = "linux")]
#[test]
fn shared_server_survives_a_worker_cgroup_kill_that_reaps_an_unregistered_process() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().to_path_buf();

    let Some(worker_scope) = super::super::cgroup::create_scope("registry-test", "server-host")
    else {
        eprintln!(
            "skipping: no writable delegated cgroup v2 tree on this host — \
             the process-group tier is covered by the sibling test"
        );
        return;
    };

    // An unregistered straggler: started by the worker, left in the worker's
    // scope. This is the `npm run dev &` the issue says must still die.
    //
    // Spawned as a bare `sleep`, NOT `sh -c "sleep 300"`: cgroup membership is
    // inherited at fork, so a shell that forks its payload before add_pid runs
    // leaves that payload outside the scope — kill_scope then reaps only the
    // shell, the test still passes, and an orphaned 5-minute sleep holds the
    // test binary's stdio pipe open, stalling every piped `cargo test` run.
    // (The production spawn path joins the scope before forking anything; see
    // cgroup_kills_a_descendant_that_escaped_the_process_group.)
    let mut straggler = Command::new("sleep").arg("300").spawn().unwrap();
    let straggler_pid = straggler.id();
    super::super::cgroup::add_pid(&worker_scope, straggler_pid).unwrap();

    // A registered shared server. `start` must place it outside the worker
    // scope; nothing in this test moves it there.
    let shared = start(
        &cas_root,
        &spec("shared-srv", "sleep 300", temp.path(), true),
    )
    .unwrap();
    let scope = shared
        .cgroup
        .clone()
        .expect("a shared server must get its own cgroup scope on a delegated host");
    assert_ne!(
        scope, worker_scope,
        "a shared server must not share the worker's scope"
    );
    assert!(
        !scope.starts_with(&worker_scope),
        "nor be nested under it: cgroup.kill reaps the whole subtree"
    );

    // Worker teardown.
    super::super::cgroup::kill_scope(&worker_scope).unwrap();
    super::super::cgroup::remove_scope(&worker_scope);

    // The straggler is a direct child of the test process, so it lingers as a
    // zombie until reaped — wait for the exit status rather than for the pid
    // to disappear, or this asserts on an artefact of the test harness.
    let mut straggler_exited = false;
    for _ in 0..80 {
        if matches!(straggler.try_wait(), Ok(Some(_))) {
            straggler_exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        straggler_exited,
        "an unregistered process must not survive containment teardown"
    );
    assert_eq!(
        straggler_pid,
        straggler.id(),
        "sanity: the straggler we waited on is the one we contained"
    );
    assert_eq!(
        liveness(&shared),
        ServerLiveness::Live,
        "GH #87: a registered shared server must survive containment teardown"
    );

    // And stop still works on it afterwards, taking its scope with it.
    let outcome = stop(&cas_root, &shared).unwrap();
    assert!(matches!(outcome, StopOutcome::Stopped { .. }));
    assert!(wait_until_gone(shared.pid));
}
