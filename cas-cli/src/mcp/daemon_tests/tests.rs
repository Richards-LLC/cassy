use crate::mcp::daemon::*;
use std::path::PathBuf;
use tempfile::TempDir;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::SqliteStore;
use crate::store::init_cas_dir;
use cas_types::{Agent, AgentRole, Session};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::test_support::TestEnvGuard;

#[test]
fn test_activity_tracker() {
    let tracker = ActivityTracker::new(5);
    assert!(tracker.idle_seconds() < 1);
    assert!(!tracker.is_idle());

    tracker.touch();
    assert!(tracker.idle_seconds() < 1);
}

#[test]
fn test_daemon_config_conversion() {
    let config = EmbeddedDaemonConfig {
        cas_root: PathBuf::from("/tmp/cas"),
        maintenance_interval_secs: 1800,
        archive_retention_days: 90,
        relevance_sampling_enabled: false,
        relevance_sampling_interval_secs: 3600,
        relevance_sampling_sample_size: 7,
        ..Default::default()
    };

    let daemon_config = config.to_daemon_config();
    assert_eq!(daemon_config.interval_minutes, 30);
    assert_eq!(daemon_config.cas_root, PathBuf::from("/tmp/cas"));
    assert_eq!(daemon_config.archive_max_bytes, config.archive_max_bytes);
    assert_eq!(daemon_config.archive_retention_days, 90);
    assert!(!daemon_config.relevance_sampling_enabled);
    assert_eq!(daemon_config.relevance_sampling_interval_secs, 3600);
    assert_eq!(daemon_config.relevance_sampling_sample_size, 7);
}

#[test]
fn apply_factory_worker_metadata_records_worker_model_effort_and_clone_path() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_CLONE_PATH", Some("/tmp/cas-worker-clone")),
        ("CAS_FACTORY_WORKER_MODEL", Some("sonnet")),
        ("CAS_FACTORY_WORKER_EFFORT", Some("high")),
    ]);
    let mut agent = Agent::new("agent-id".to_string(), "agent".to_string());

    apply_factory_worker_metadata(&mut agent, None);

    assert_eq!(
        agent.metadata.get("clone_path").map(String::as_str),
        Some("/tmp/cas-worker-clone")
    );
    assert_eq!(
        agent.metadata.get("worker_model").map(String::as_str),
        Some("sonnet")
    );
    assert_eq!(
        agent.metadata.get("worker_effort").map(String::as_str),
        Some("high")
    );
}

/// cas-be49 / GH #161: a repeated SessionStart for a still-live factory
/// worker used to insert the fresh hook session id as a second agent row.
/// This is the narrow durable reproduction of the urgent-interrupt symptom:
/// the first row represents the eager MCP/bootstrap registration, then the
/// socket reports a different session id for the same Claude Code PID.
#[test]
fn repeated_factory_worker_session_start_reuses_live_pid_identity() {
    let temp = TempDir::new().expect("temp project");
    let cas_root = init_cas_dir(temp.path()).expect("init cas dir");
    let agent_store = crate::store::open_agent_store(&cas_root).expect("open agent store");
    let cc_pid = std::process::id();

    let mut original = Agent::new(
        "worker-established-session".to_string(),
        "strong-bear-44".to_string(),
    );
    original.role = AgentRole::Worker;
    original.agent_type = cas_types::AgentType::Worker;
    // Eager MCP bootstrap runs in a child process: it records its own PID
    // and the Claude Code process as `ppid`. The follow-up socket event
    // carries that Claude Code PID directly.
    original.pid = Some(cc_pid.checked_add(1).expect("non-max test pid"));
    original.ppid = Some(cc_pid);
    agent_store
        .register(&original)
        .expect("register original worker");

    let (resolved, reused) = register_session_start_agent(
        agent_store.as_ref(),
        "fresh-session-after-urgent-interrupt",
        Some("strong-bear-44"),
        Some("worker"),
        cc_pid,
        Some("/tmp/strong-bear-44"),
    )
    .expect("reconcile repeated SessionStart");

    assert!(
        reused,
        "same live worker PID must reuse its durable identity"
    );
    assert_eq!(resolved.id, original.id);
    assert!(
        agent_store
            .get("fresh-session-after-urgent-interrupt")
            .is_err(),
        "the fresh SessionStart id must not mint a ghost row"
    );
    let same_worker_rows = agent_store
        .list(None)
        .expect("list agents")
        .into_iter()
        .filter(|agent| agent.name == "strong-bear-44")
        .count();
    assert_eq!(same_worker_rows, 1, "worker identity must remain singular");
}

#[test]
fn supervisor_hook_registration_persists_role_and_resolves_owner() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        (
            "CAS_FACTORY_SESSION",
            Some("factory-supervisor-registration"),
        ),
    ]);
    let temp = TempDir::new().expect("temp project");
    let cas_root = init_cas_dir(temp.path()).expect("init cas dir");
    let agent_store = crate::store::open_agent_store(&cas_root).expect("open agent store");

    let (supervisor, reused) = register_session_start_agent(
        agent_store.as_ref(),
        "supervisor-hook-session",
        Some("lively-supervisor-11"),
        Some("supervisor"),
        std::process::id(),
        None,
    )
    .expect("register supervisor from SessionStart hook");

    assert!(!reused);
    assert_eq!(supervisor.role, AgentRole::Supervisor);
    assert_eq!(supervisor.agent_type, cas_types::AgentType::Primary);
    assert_eq!(
        supervisor.factory_session.as_deref(),
        Some("factory-supervisor-registration")
    );
    let owner =
        crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_owning_supervisor(
            agent_store.as_ref(),
            Some("factory-supervisor-registration"),
        )
        .expect("registered supervisor resolves as factory owner");
    assert_eq!(owner.agent_id, "supervisor-hook-session");
    assert_eq!(owner.name, "lively-supervisor-11");
}

/// cas-921f (P1 fix-round): the real env→register→resolve chain for a
/// worker's harness, end to end — from a REAL `CAS_FACTORY_WORKER_CLI` env
/// var (not injected `Agent.metadata` directly, which is what the earlier
/// factory_ops.rs `worker_cli_from_agent` tests do and why they missed the
/// live bug: `PtyConfig::grok` never actually set this env var, so
/// `apply_factory_worker_metadata` never had anything to read). Proves the
/// full path: env var → `apply_factory_worker_metadata` → `agent.metadata`
/// → `worker_cli_from_agent` → `SupervisorCli::Grok`.
#[test]
fn apply_factory_worker_metadata_real_env_persists_worker_cli_grok() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_WORKER_CLI", Some("grok")),
    ]);
    let mut agent = Agent::new("agent-id".to_string(), "grok-worker".to_string());

    apply_factory_worker_metadata(&mut agent, None);

    assert_eq!(
        agent.metadata.get("worker_cli").map(String::as_str),
        Some("grok")
    );
    assert_eq!(
        crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent),
        cas_mux::SupervisorCli::Grok,
        "the persisted metadata must round-trip through worker_cli_from_agent \
         to the real Grok harness, not silently default to Claude"
    );
}

#[test]
fn apply_factory_worker_metadata_skips_model_effort_for_non_worker() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", None),
        ("CAS_CLONE_PATH", None),
        ("CAS_FACTORY_WORKER_MODEL", Some("sonnet")),
        ("CAS_FACTORY_WORKER_EFFORT", Some("high")),
    ]);
    let mut agent = Agent::new_with_role(
        "agent-id".to_string(),
        "agent".to_string(),
        AgentRole::Supervisor,
    );

    apply_factory_worker_metadata(&mut agent, Some("/tmp/explicit-clone"));

    assert_eq!(
        agent.metadata.get("clone_path").map(String::as_str),
        Some("/tmp/explicit-clone")
    );
    assert!(!agent.metadata.contains_key("worker_model"));
    assert!(!agent.metadata.contains_key("worker_effort"));
}

#[test]
fn stale_factory_worker_queues_exact_forced_process_tree_shutdown() {
    let temp = TempDir::new().expect("temp project");
    let cas_root = init_cas_dir(temp.path()).expect("init cas dir");
    let mut worker = Agent::new("stale-worker-id".to_string(), "stale-worker".to_string());
    worker.role = AgentRole::Worker;
    worker.factory_session = Some("factory-gh236".to_string());
    let request_id = queue_stale_factory_worker_shutdown(&cas_root, &worker).expect("queue stale factory worker").expect("factory worker must queue shutdown");
    let queue = crate::store::open_spawn_queue_store(&cas_root).expect("open spawn queue");
    let request = queue.peek(10).expect("peek shutdown queue").into_iter().find(|request| request.id == request_id).expect("exact shutdown request");
    assert_eq!(request.action, cas_store::SpawnAction::Shutdown);
    assert_eq!(request.worker_names, vec!["stale-worker"]);
    assert!(request.force);
    assert_eq!(request.factory_session.as_deref(), Some("factory-gh236"));
    assert_eq!(queue_stale_factory_worker_shutdown(&cas_root, &worker).expect("dedupe queue"), None);
}

// =========================================================================
// EmbeddedDaemonStatus tests
// =========================================================================

#[test]
fn test_embedded_daemon_status_default() {
    let status = EmbeddedDaemonStatus::default();
    assert!(!status.running);
    assert!(status.last_maintenance.is_none());
    assert!(status.last_cloud_sync.is_none());
    assert!(status.next_maintenance.is_none());
    assert_eq!(status.observations_processed, 0);
    assert_eq!(status.decay_applied, 0);
    assert_eq!(status.cloud_sync_pending, 0);
    assert!(!status.cloud_sync_available);
    assert_eq!(status.cloud_items_pushed, 0);
    assert_eq!(status.cloud_items_pulled, 0);
    assert_eq!(status.idle_seconds, 0);
    assert!(!status.is_idle);
    assert!(status.last_error.is_none());
}

// =========================================================================
// EmbeddedDaemonConfig tests
// =========================================================================

#[test]
fn test_embedded_daemon_config_default() {
    let config = EmbeddedDaemonConfig::default();
    assert_eq!(config.maintenance_interval_secs, 30 * 60);
    assert_eq!(config.cloud_sync_interval_secs, 60);
    assert_eq!(config.min_idle_secs, 60);
    assert!(config.apply_decay);
    assert!(config.process_observations);
    assert!(config.cloud_sync_enabled);
    assert_eq!(config.batch_size, 20);
}

#[test]
fn test_get_sessions_for_sync_uses_cas_root_directory_path() {
    let temp = TempDir::new().expect("temp dir");
    let cas_root = init_cas_dir(temp.path()).expect("init cas dir");

    let sqlite_store = SqliteStore::open(&cas_root).expect("open sqlite store");

    let mut session = Session::new(
        "session-for-sync".to_string(),
        temp.path().display().to_string(),
        Some("default".to_string()),
    );
    session.started_at = chrono::Utc::now() - chrono::Duration::hours(1);
    sqlite_store
        .start_session(&session)
        .expect("insert session");

    let queue = SyncQueue::open(&cas_root).expect("open sync queue");
    queue.init().expect("init sync queue");

    let sessions = super::get_sessions_for_sync(&cas_root, &queue);
    assert_eq!(sessions.len(), 1, "expected one session from sqlite");
    assert_eq!(sessions[0].session_id, "session-for-sync");
}

/// cas-8248: the automatic daemon cycle must drain the same team queue that
/// `cas cloud sync` drains.  This is deliberately a daemon-level regression
/// test: direct `push_team` coverage cannot detect missing scheduler wiring.
#[tokio::test]
async fn embedded_daemon_cloud_cycle_drains_team_queue() {
    const TEAM_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    let server = MockServer::start().await;
    let personal_pull_path = ["/api/sync", "pull"].join("/");
    Mock::given(method("GET"))
        .and(path(personal_pull_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEAM_ID}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "entries": 1, "tasks": 0, "rules": 0, "skills": 0,
                "sessions": 0, "verifications": 0, "events": 0,
                "prompts": 0, "file_changes": 0, "commit_links": 0,
                "agents": 0, "worktrees": 0,
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().expect("temp project");
    let cas_root = init_cas_dir(temp.path()).expect("init cas dir");
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, "daemon-cloud-cycle")
        .expect("pin the project fixture identity");
    let nonexistent_user_config = temp.path().join("no-user-cloud.json");
    let cas_root_text = cas_root.to_string_lossy().into_owned();
    let user_config_text = nonexistent_user_config.to_string_lossy().into_owned();
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_ROOT", Some(cas_root_text.as_str())),
        ("CAS_USER_CLOUD_JSON", Some(user_config_text.as_str())),
    ]);

    let mut cloud = CloudConfig::default();
    cloud.endpoint = server.uri();
    cloud.token = Some("synthetic-test-token".to_string());
    cloud.set_team(TEAM_ID, "synthetic-team");
    cloud
        .save_to_cas_dir(&cas_root)
        .expect("save synthetic cloud config");

    let queue = SyncQueue::open(&cas_root).expect("open sync queue");
    queue.init().expect("init sync queue");
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "daemon-team-entry",
            SyncOperation::Upsert,
            Some(r#"{"id":"daemon-team-entry","scope":"project","content":"queued"}"#),
            TEAM_ID,
        )
        .expect("enqueue team item");

    let daemon = EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: cas_root.clone(),
        index_code: false,
        ..Default::default()
    });
    let result = daemon
        .trigger_cloud_sync()
        .await
        .expect("automatic cloud cycle");

    assert_eq!(result.pushed_entries, 1);
    assert!(
        queue
            .pending_for_team(TEAM_ID, 100, 5)
            .expect("read team queue")
            .is_empty(),
        "automatic cloud cycle must drain team-scoped rows"
    );
}

// =========================================================================
// Agent heartbeat liveness gate (EPIC cas-9508 / cas-2749)
// =========================================================================
//
// Regression coverage: the shared `cas serve` daemon must not keep pinging
// `store.heartbeat()` for an agent whose Claude Code client has died. Without
// this gate, a crashed CC client (e.g. Bun/React-Ink unhandled-rejection
// zombie) keeps the agent's `last_heartbeat` fresh forever and supervisors see
// dead workers as "active" in `worker_status`.

#[cfg(unix)]
#[test]
fn pid_alive_self_is_live() {
    let our_pid = std::process::id();
    assert!(
        crate::mcp::daemon::pid_alive(our_pid),
        "our own PID must report alive"
    );
}

#[cfg(unix)]
#[test]
fn pid_alive_dead_child_is_dead() {
    // Spawn a short-lived child, wait for it to exit, then confirm its PID
    // is reported dead. `waitpid`/`Child::wait` returning Ok guarantees the
    // child has been reaped — the kernel removes the process synchronously
    // at reap time, so the very next `kill(pid, 0)` sees
    // ESRCH. No poll loop is needed (cas-8240: the prior 200ms poll was
    // defensive against a kernel behavior that does not actually occur on
    // post-reap; the stronger synchronous assertion catches
    // regressions a forgiving poll would mask).
    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("spawn true from PATH");
    let pid = child.id();
    child.wait().expect("wait for child");
    assert!(
        !crate::mcp::daemon::pid_alive(pid),
        "reaped child PID {pid} must report dead (ESRCH) immediately after wait()"
    );
}

#[cfg(unix)]
#[test]
fn pid_alive_obviously_invalid_pid_is_dead() {
    // PID space on Linux tops out at /proc/sys/kernel/pid_max (typically
    // 4_194_304). A PID near u32::MAX cannot be a live process. This guards
    // the liveness gate from silently treating out-of-range PIDs as live,
    // which would defeat the whole cas-2749 fix.
    assert!(
        !crate::mcp::daemon::pid_alive(u32::MAX - 1),
        "out-of-range PID must report dead"
    );
}

// =========================================================================
// PID-reuse-resistant fingerprint via /proc/<pid>/stat starttime (cas-ea46)
// =========================================================================
//
// Regression coverage: `pid_alive(pid)` alone cannot distinguish the original
// Claude Code client from a kernel-recycled occupant of the same PID slot.
// The liveness gate now pairs `pid_alive` with `read_pid_starttime` (field
// 22 of /proc/<pid>/stat) via `pid_matches_fingerprint` so PID reuse no
// longer bypasses the gate.

#[cfg(target_os = "linux")]
#[test]
fn read_pid_starttime_self_is_stable() {
    // Our own process starttime must parse and return a positive u64.
    // Repeated reads within a single test must yield the same value —
    // starttime is set at exec and does not drift.
    let our_pid = std::process::id();
    let first = crate::mcp::daemon::read_pid_starttime(our_pid)
        .expect("read_pid_starttime must succeed on self");
    assert!(
        first > 0,
        "starttime must be positive clock ticks since boot"
    );
    let second =
        crate::mcp::daemon::read_pid_starttime(our_pid).expect("second read must also succeed");
    assert_eq!(
        first, second,
        "starttime must be invariant for the lifetime of a process"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn read_pid_starttime_out_of_range_is_none() {
    // /proc/<u32::MAX-1>/stat does not exist; the helper must return None
    // rather than panic, so callers can fall back to pid-only liveness.
    assert_eq!(
        crate::mcp::daemon::read_pid_starttime(u32::MAX - 1),
        None,
        "out-of-range PID must yield None (no panic, no false positive)"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn read_pid_starttime_reaped_child_is_none() {
    // After a child has been reaped, /proc/<pid>/stat disappears. This
    // path is the workhorse for detecting a dead CC client: the agent
    // record's stashed starttime will never again match a live process
    // under the same PID, because the stat file itself is gone.
    let mut child = std::process::Command::new("/bin/true")
        .spawn()
        .expect("spawn /bin/true");
    let pid = child.id();
    let _ = child.wait().expect("wait for child");
    // /proc cleanup is synchronous after reap on Linux — no poll needed.
    assert_eq!(
        crate::mcp::daemon::read_pid_starttime(pid),
        None,
        "reaped child's /proc/<pid>/stat must be gone; starttime read returns None"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn pid_matches_fingerprint_true_on_self_with_correct_starttime() {
    // Positive control: the fingerprint helper must agree with itself
    // when fed the live PID and its just-read starttime.
    let our_pid = std::process::id();
    let st = crate::mcp::daemon::read_pid_starttime(our_pid).expect("starttime on self");
    assert!(
        crate::mcp::daemon::pid_matches_fingerprint(our_pid, st),
        "self + current starttime must match"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn pid_matches_fingerprint_false_on_self_with_wrong_starttime() {
    // This is the cas-ea46 core assertion: a live PID with a *different*
    // starttime than what was stashed at registration must be rejected as
    // "someone else got this PID". We fake the stashed value by flipping
    // a bit in the real starttime; the helper must return false.
    let our_pid = std::process::id();
    let real_st = crate::mcp::daemon::read_pid_starttime(our_pid).expect("starttime on self");
    let wrong_st = real_st.wrapping_add(1);
    assert_ne!(real_st, wrong_st, "bit-flip must change the value");
    assert!(
        !crate::mcp::daemon::pid_matches_fingerprint(our_pid, wrong_st),
        "live PID + mismatched starttime must report as dead (PID recycled)"
    );
}

#[cfg(unix)]
#[test]
fn pid_matches_fingerprint_false_on_dead_pid() {
    // An out-of-range PID is dead regardless of claimed starttime; the
    // pid_alive() short-circuit must return false before any /proc read.
    assert!(
        !crate::mcp::daemon::pid_matches_fingerprint(u32::MAX - 1, 12345),
        "dead PID must report as non-matching regardless of starttime"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn parse_starttime_from_stat_handles_comm_with_parens_and_spaces() {
    // Adversarial/testing review: /proc/<pid>/stat field 2 (`comm`) is wrapped
    // in parens and may itself contain spaces and parens. The parser splits
    // on the *last* `)` to preserve correct field indexing; flipping to the
    // *first* `)` would silently shift every subsequent field index by the
    // number of stray `)` inside comm. Pin the invariant with a synthetic
    // line where comm contains both a space and a `)`.
    //
    // Field layout (after comm): state=R, ppid=1, pgrp=1, session=1, tty_nr=0,
    // tpgid=-1, flags=0, minflt=0, cminflt=0, majflt=0, cmajflt=0, utime=0,
    // stime=0, cutime=0, cstime=0, priority=20, nice=0, num_threads=1,
    // itrealvalue=0, starttime=9876543210. That's 19 fields after state,
    // matching field 22 = index 19 in the post-comm tail.
    let synthetic =
        "1234 (weird )name with spaces) R 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 9876543210 1 2 3";
    assert_eq!(
        crate::mcp::daemon::parse_starttime_from_stat(synthetic),
        Some(9876543210),
        "parser must split on the last `)` and land on field 22"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn parse_starttime_from_stat_returns_none_on_malformed_input() {
    // Garbage input must not panic and must not fabricate a starttime.
    assert_eq!(
        crate::mcp::daemon::parse_starttime_from_stat(""),
        None,
        "empty input → None"
    );
    assert_eq!(
        crate::mcp::daemon::parse_starttime_from_stat("no paren here"),
        None,
        "no `)` → None"
    );
    // Too few fields after comm.
    assert_eq!(
        crate::mcp::daemon::parse_starttime_from_stat("1 (short) R 1 2"),
        None,
        "truncated stat → None"
    );
}

// =========================================================================
// evaluate_liveness outcome matrix (cas-5b1c)
// =========================================================================
//
// The heartbeat-gate branch selection was extracted from send_agent_heartbeat
// so the fingerprint-vs-pid-only decision can be unit-tested without a live
// daemon, store, or tokio runtime. These tests pin the selection logic.
// Adding a new outcome or reordering the match arms will fail at least one
// of these.

use crate::mcp::daemon::{LivenessOutcome, PID_STARTTIME_KEY, evaluate_liveness};

fn make_test_agent(pid: Option<u32>, starttime_meta: Option<&str>) -> crate::types::Agent {
    let mut agent = crate::types::Agent::new("eval-test".to_string(), "w".to_string());
    agent.pid = pid;
    if let Some(raw) = starttime_meta {
        agent
            .metadata
            .insert(PID_STARTTIME_KEY.to_string(), raw.to_string());
    }
    agent
}

#[test]
fn evaluate_liveness_no_pid_recorded_when_agent_pid_is_none() {
    // Legacy agent (pre-cas-2749). Neither probe should be consulted; the
    // outcome must be NoPidRecorded so the caller can emit the legacy warn.
    let agent = make_test_agent(None, None);
    let probe_calls = std::cell::Cell::new(0u32);
    let outcome = evaluate_liveness(
        &agent,
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            true
        },
        |_, _| {
            probe_calls.set(probe_calls.get() + 1);
            true
        },
    );
    assert_eq!(outcome, LivenessOutcome::NoPidRecorded);
    assert_eq!(
        probe_calls.get(),
        0,
        "no pid → neither probe should be called"
    );
}

#[test]
fn evaluate_liveness_alive_with_fingerprint_when_match() {
    // Metadata contains a valid fingerprint and fingerprint_matches_fn agrees
    // → Alive { fingerprint_checked: true }. pid_alive must not be called
    // (the strict check is authoritative when a fingerprint is present).
    let agent = make_test_agent(Some(4242), Some("9876543210"));
    let pid_alive_called = std::cell::Cell::new(false);
    let fp_called_with = std::cell::Cell::new(None);
    let outcome = evaluate_liveness(
        &agent,
        |_| {
            pid_alive_called.set(true);
            true
        },
        |pid, st| {
            fp_called_with.set(Some((pid, st)));
            true
        },
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 4242,
            fingerprint_checked: true
        }
    );
    assert!(
        !pid_alive_called.get(),
        "pid_alive must NOT be called when fingerprint is present"
    );
    assert_eq!(
        fp_called_with.get(),
        Some((4242u32, 9876543210u64)),
        "fingerprint_matches_fn must be called with (pid, expected)"
    );
}

#[test]
fn evaluate_liveness_dead_when_fingerprint_mismatch() {
    // Metadata contains a valid fingerprint but fingerprint_matches_fn
    // returns false → Dead with fingerprint_checked=true. This is the
    // core cas-ea46 AC in extracted form.
    let agent = make_test_agent(Some(4242), Some("9876543210"));
    let outcome = evaluate_liveness(&agent, |_| unreachable!(), |_, _| false);
    assert_eq!(
        outcome,
        LivenessOutcome::Dead {
            cc_pid: 4242,
            fingerprint_checked: true
        }
    );
}

#[test]
fn evaluate_liveness_alive_legacy_when_no_fingerprint_and_pid_alive() {
    // Pre-cas-ea46 agent: pid present but no metadata key. pid_alive_fn
    // reports alive → AliveLegacyFingerprint. fingerprint_matches_fn must
    // not be consulted because there is no expected starttime to compare.
    let agent = make_test_agent(Some(7777), None);
    let fp_called = std::cell::Cell::new(false);
    let outcome = evaluate_liveness(
        &agent,
        |pid| {
            assert_eq!(pid, 7777);
            true
        },
        |_, _| {
            fp_called.set(true);
            true
        },
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 7777,
            fingerprint_checked: false
        }
    );
    assert!(
        !fp_called.get(),
        "fingerprint fn must NOT be called when no fingerprint metadata is stashed"
    );
}

#[test]
fn evaluate_liveness_dead_when_no_fingerprint_and_pid_dead() {
    // Pre-cas-ea46 agent with dead pid: pid_alive_fn reports dead →
    // Dead with fingerprint_checked=false so caller tracing can
    // distinguish pid-only from fingerprint-verified verdicts.
    let agent = make_test_agent(Some(7777), None);
    let outcome = evaluate_liveness(&agent, |_| false, |_, _| unreachable!());
    assert_eq!(
        outcome,
        LivenessOutcome::Dead {
            cc_pid: 7777,
            fingerprint_checked: false
        }
    );
}

#[test]
fn evaluate_liveness_malformed_fingerprint_falls_back_to_pid_only() {
    // If a future writer puts garbage in PID_STARTTIME_KEY (or a migration
    // mangles it), `parse::<u64>()` yields None → behavior must be
    // indistinguishable from "no fingerprint stashed": pid-only fallback.
    // This pins the graceful-degradation contract surfaced in cas-ea46
    // adversarial review.
    let agent = make_test_agent(Some(9999), Some("not-a-number"));
    let outcome = evaluate_liveness(
        &agent,
        |pid| {
            assert_eq!(pid, 9999);
            true
        },
        |_, _| panic!("fingerprint fn must not be called on malformed fingerprint"),
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 9999,
            fingerprint_checked: false
        }
    );
}

#[test]
fn evaluate_liveness_non_linux_fallback_live_pid_unreadable_proc() {
    // Simulates the non-Linux / unreadable-/proc case on a host where
    // the fingerprint WAS stashed at registration but is now unverifiable.
    // Per cas-ea46 strict semantics: fingerprint_matches_fn returns false
    // → evaluate_liveness must report Dead, not silently trust pid_alive.
    // This pins the "None from /proc on a fingerprinted agent = suspicious"
    // adversarial catch.
    let agent = make_test_agent(Some(1234), Some("5555"));
    let outcome = evaluate_liveness(
        &agent,
        |_| panic!("pid_alive must not be consulted when fingerprint path is taken"),
        |pid, st| {
            // Simulate strict: live pid, /proc unreadable → fingerprint_fn
            // returns false (pid_matches_fingerprint's semantics).
            assert_eq!((pid, st), (1234, 5555));
            false
        },
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Dead {
            cc_pid: 1234,
            fingerprint_checked: true
        }
    );
}

// --- cas-b157: typed pid_starttime preferred, metadata fallback kept ----

#[test]
fn evaluate_liveness_prefers_typed_pid_starttime_over_metadata() {
    // cas-b157: when BOTH the typed field and the legacy metadata key
    // are present, the typed field wins. The fingerprint fn must be
    // called with the TYPED value, not the metadata value — otherwise
    // a worker that upgraded mid-flight would still rely on whatever
    // the legacy metadata held, defeating the typed promotion.
    let mut agent = make_test_agent(Some(5555), Some("11111"));
    agent.pid_starttime = Some(22222);

    let outcome = evaluate_liveness(
        &agent,
        |_| panic!("pid_alive must not be consulted when a fingerprint is present"),
        |pid, st| {
            assert_eq!(
                (pid, st),
                (5555u32, 22222u64),
                "typed field must take precedence over metadata"
            );
            true
        },
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 5555,
            fingerprint_checked: true
        }
    );
}

#[test]
fn evaluate_liveness_falls_back_to_metadata_when_typed_is_none() {
    // cas-b157: legacy agents registered before the typed-field
    // migration still carry their fingerprint in `metadata`. When
    // `pid_starttime` is None but the metadata key parses, the gate
    // must still perform the strict check against the metadata value.
    let mut agent = make_test_agent(Some(7777), Some("33333"));
    agent.pid_starttime = None;

    let outcome = evaluate_liveness(
        &agent,
        |_| panic!("pid_alive must not be consulted when metadata fingerprint parses"),
        |pid, st| {
            assert_eq!(
                (pid, st),
                (7777u32, 33333u64),
                "metadata fallback must drive fingerprint fn"
            );
            true
        },
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 7777,
            fingerprint_checked: true
        }
    );
}

#[test]
fn evaluate_liveness_typed_none_with_malformed_metadata_pid_only() {
    // cas-b157: neither source yields a usable fingerprint (typed is
    // None, metadata is garbage). Gate degrades to pid-only as before.
    let mut agent = make_test_agent(Some(9999), Some("not-a-number"));
    agent.pid_starttime = None;

    let outcome = evaluate_liveness(
        &agent,
        |pid| {
            assert_eq!(pid, 9999);
            true
        },
        |_, _| panic!("fingerprint fn must not be called without a parseable fingerprint"),
    );
    assert_eq!(
        outcome,
        LivenessOutcome::Alive {
            cc_pid: 9999,
            fingerprint_checked: false
        }
    );
}

#[cfg(target_os = "linux")]
#[test]
fn stamp_pid_fingerprint_writes_both_typed_and_metadata_shadow() {
    // cas-b157: stamp must populate BOTH the typed field AND the
    // metadata shadow entry so legacy readers on an older binary that
    // restart the daemon mid-flight still see the fingerprint.
    let mut agent = crate::types::Agent::new("a".to_string(), "a".to_string());
    let pid = std::process::id();

    crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);

    let typed = agent.pid_starttime.expect("typed field must be populated");
    let meta: u64 = agent
        .metadata
        .get(crate::mcp::daemon::PID_STARTTIME_KEY)
        .expect("metadata shadow must be populated")
        .parse()
        .expect("metadata shadow must parse as u64");
    assert_eq!(
        typed, meta,
        "typed field and metadata shadow must agree on the same starttime"
    );
}

// =========================================================================
// Registration-site fingerprint-stamp parity (cas-5b1c)
// =========================================================================
//
// Every agent-registration code path that sets `agent.pid = Some(pid)` must
// also stamp the pid_starttime fingerprint. A silent drift (one site omits
// the stamp) degrades PID-reuse protection to pid-only for that site with
// no compile-time signal. The table below enumerates the call-path shape
// each site produces; adding a 4th site is one append. If a new site is
// introduced without a row here, that is the review catch.

#[cfg(target_os = "linux")]
#[test]
fn all_agent_registration_sites_stamp_pid_fingerprint() {
    // Use this process's own PID so read_pid_starttime has a real /proc
    // entry to observe. Each builder mirrors the pid + stamp_pid_fingerprint
    // sequence from one of the three real registration code paths.
    let pid = std::process::id();

    type AgentBuilder = fn(u32) -> crate::types::Agent;

    fn socket_driven_register(pid: u32) -> crate::types::Agent {
        // Mirrors daemon::register_agent (socket-driven hook path).
        let mut agent = crate::types::Agent::new("sock-driven".to_string(), "w".to_string());
        agent.pid = Some(pid);
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
        agent
    }

    fn self_register_hints(pid: u32) -> crate::types::Agent {
        // Mirrors server::register_agent_with_hints (MCP bootstrap path).
        let mut agent = crate::types::Agent::new("self-hints".to_string(), "w".to_string());
        agent.pid = Some(pid);
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
        agent
    }

    fn re_register_missing(pid: u32) -> crate::types::Agent {
        // Mirrors server::mod.rs re-register-missing fallback.
        let mut agent = crate::types::Agent::new("re-reg".to_string(), "w".to_string());
        agent.pid = Some(pid);
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
        agent
    }

    // CONTRACT: adding a row here is the ONLY thing you need to do when you
    // add a new registration site. Append one (name, builder) pair; the
    // assertions below apply uniformly — same contract, same test.
    // (See cas-389c for the real-fn-invocation follow-up that will make
    // this catch a real site that forgets to stamp, not just a mirror.)
    let sites: &[(&str, AgentBuilder)] = &[
        (
            "daemon::register_agent (socket-driven)",
            socket_driven_register,
        ),
        (
            "server::register_agent_with_hints (self)",
            self_register_hints,
        ),
        ("server::re-register-missing (self)", re_register_missing),
    ];

    let expected_st = crate::mcp::daemon::read_pid_starttime(pid).expect("starttime on self");

    for (name, build) in sites {
        let agent = build(pid);
        assert_eq!(agent.pid, Some(pid), "[{name}] pid must be populated");
        let stamped = agent
            .metadata
            .get(PID_STARTTIME_KEY)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!("[{name}] must populate PID_STARTTIME_KEY with a parseable u64");
            });
        assert_eq!(
            stamped, expected_st,
            "[{name}] stamped value must match live read_pid_starttime"
        );
    }
}

// =========================================================================
// Source-scanning invariant: every real `agent.pid = Some(...)` must be
// followed by a `stamp_pid_fingerprint` call (cas-389c)
// =========================================================================
//
// The table-driven registration-site parity test above uses mirror builders
// that reimplement the (pid + stamp) shape locally. If a real site drops
// the stamp, the mirror still passes — exactly the antipattern cas-389c
// exists to close (per MEMORY.md feedback_verify_writer_and_reader, same
// class of gap that shipped cas-3086 with reader+tests but no writer).
//
// This test scans cas-cli/src recursively for every `agent.pid = Some(`
// line in PRODUCTION code (skipping test files, doc comments, and the
// helper itself) and asserts that `stamp_pid_fingerprint` appears within
// a short window of lines below it. A real site that forgets to stamp
// fails this test at compile+run time, giving us the writer-exists guard
// the mirror test cannot provide.
//
// AC from cas-389c: "temporarily comment the stamp at one real site → the
// new test must fail." Verified before ship.

#[test]
fn every_production_agent_pid_assignment_has_nearby_fingerprint_stamp() {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Recursively collect all .rs files under `dir`.
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    let src_root = crate::test_paths::crate_root().join("src");
    assert!(
        src_root.exists(),
        "cas-cli/src must exist at {}",
        src_root.display()
    );

    let mut rs_files = Vec::new();
    collect_rs_files(&src_root, &mut rs_files);
    assert!(
        !rs_files.is_empty(),
        "source walk must find .rs files under {}",
        src_root.display()
    );

    // How many lines AFTER an `agent.pid = Some(` line can the stamp call
    // appear? 10 is comfortable — real sites today stamp within 2 lines.
    const WINDOW: usize = 10;
    // The regex-free needle we search for. Exact spelling tolerates
    // trailing whitespace but nothing else. If the helper is ever renamed,
    // update this constant and the test message.
    const STAMP_NEEDLE: &str = "stamp_pid_fingerprint";
    const PID_NEEDLE: &str = "agent.pid = Some(";

    let mut violations: Vec<String> = Vec::new();
    let mut sites_checked = 0usize;

    for path in &rs_files {
        // Skip test modules and the source-scanning test itself — the
        // invariant applies to production registration sites only.
        let path_str = path.to_string_lossy();
        let is_test_file = path_str.contains("_tests/") || path_str.ends_with("_tests.rs");
        if is_test_file {
            continue;
        }

        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        for (lineno, line) in lines.iter().enumerate() {
            // Strip leading whitespace to match indentation-invariant. Skip
            // lines that begin with `//` so doc/comment mentions of the
            // pattern don't trip the scan.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if !line.contains(PID_NEEDLE) {
                continue;
            }
            sites_checked += 1;

            // Look at this line and the next WINDOW lines for the stamp
            // call. This tolerates a few intervening fields being set
            // between `agent.pid = Some(...)` and the stamp.
            let stamp_found = lines
                .iter()
                .skip(lineno)
                .take(WINDOW + 1)
                .any(|l| l.contains(STAMP_NEEDLE));
            if !stamp_found {
                violations.push(format!(
                    "{}:{} — `{PID_NEEDLE}` without `{STAMP_NEEDLE}` within {WINDOW} lines below",
                    path.strip_prefix(&src_root).unwrap_or(path).display(),
                    lineno + 1,
                ));
            }
        }
    }

    assert!(
        sites_checked > 0,
        "scan must find at least one `{PID_NEEDLE}` occurrence — either the \
         needle drifted (rename?), the scan path is wrong, or all sites were \
         refactored out (update this test)"
    );

    assert!(
        violations.is_empty(),
        "cas-389c invariant violated: {} production site(s) set `agent.pid` \
         without a nearby `{STAMP_NEEDLE}` call — adding a pid without a \
         fingerprint silently disables PID-reuse protection for that agent. \
         Violations:\n  {}\n\n\
         Fix: call `crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid)` \
         immediately after the `agent.pid = Some(pid);` line.",
        violations.len(),
        violations.join("\n  ")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn stamp_pid_fingerprint_writes_metadata_for_self() {
    // cas-ea46 / maintainability review: a single helper writes the
    // pid_starttime metadata key, so every registration site gets the
    // fingerprint without drift. Assert the helper actually populates the
    // key with a parseable u64 matching read_pid_starttime.
    let mut agent = crate::types::Agent::new("test-stamp".to_string(), "unit".to_string());
    let pid = std::process::id();
    let expected = crate::mcp::daemon::read_pid_starttime(pid).expect("starttime on self");
    crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
    let stamped = agent
        .metadata
        .get(crate::mcp::daemon::PID_STARTTIME_KEY)
        .expect("stamp_pid_fingerprint must populate PID_STARTTIME_KEY");
    assert_eq!(
        stamped.parse::<u64>().ok(),
        Some(expected),
        "stamped value must round-trip as a u64 equal to read_pid_starttime(self)"
    );
}

// ===== cas-499c: idle-preferred code-index scheduling =====

/// The polite path is unchanged: an idle daemon indexes on every tick.
#[test]
fn code_index_runs_immediately_when_idle() {
    assert!(should_run_code_index(true, std::time::Duration::ZERO));
    assert!(should_run_code_index(
        true,
        std::time::Duration::from_secs(CODE_INDEX_MAX_STALENESS_SECS * 10)
    ));
}

/// A busy daemon defers — but only up to the ceiling. This is the regression that made the
/// symbol index empty on every install: a hard `is_idle()` gate on a daemon that is never idle
/// means the job never runs at all, so `code_files` stays 0 forever.
#[test]
fn code_index_defers_while_busy_then_overrides_at_the_ceiling() {
    let ceiling = std::time::Duration::from_secs(CODE_INDEX_MAX_STALENESS_SECS);

    assert!(
        !should_run_code_index(false, std::time::Duration::ZERO),
        "a busy daemon with a fresh index must wait for a quiet moment"
    );
    assert!(
        !should_run_code_index(false, ceiling - std::time::Duration::from_secs(1)),
        "still inside the ceiling: keep deferring"
    );
    assert!(
        should_run_code_index(false, ceiling),
        "at the ceiling the daemon must index anyway — politeness may defer, never cancel"
    );
    assert!(
        should_run_code_index(false, ceiling * 3),
        "long past the ceiling it must certainly run"
    );
}

/// The ceiling is a product decision, not an incidental number: it bounds how stale
/// `code_search` may be, and the doctor lag line is calibrated against it.
#[test]
fn code_index_max_staleness_is_five_minutes() {
    assert_eq!(CODE_INDEX_MAX_STALENESS_SECS, 300);
}
