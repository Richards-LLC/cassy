use crate::AgentStore;
use crate::agent_store::SqliteAgentStore;
use cas_types::{Agent, AgentRole, AgentStatus, AgentType, ClaimResult};
use chrono::{Duration, Utc};
use rusqlite::params;
use tempfile::TempDir;

static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(vars: &[(&str, Option<&str>)]) -> Self {
        let lock = ENV_MUTEX.lock().unwrap();
        let mut saved = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            let key = (*key).to_string();
            let prev = std::env::var(&key).ok();
            match value {
                Some(value) => unsafe { std::env::set_var(&key, value) },
                None => unsafe { std::env::remove_var(&key) },
            }
            saved.push((key, prev));
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain(..) {
            match prev {
                Some(value) => unsafe { std::env::set_var(&key, value) },
                None => unsafe { std::env::remove_var(&key) },
            }
        }
    }
}

fn create_test_store() -> (TempDir, SqliteAgentStore) {
    let temp = TempDir::new().unwrap();
    let store = SqliteAgentStore::open(temp.path()).unwrap();
    store.init().unwrap();
    (temp, store)
}

#[test]
fn test_agent_crud() {
    let (_temp, store) = create_test_store();

    // Register agent
    let agent = Agent::new("agent-test".to_string(), "Test Agent".to_string());
    store.register(&agent).unwrap();

    // Get agent
    let retrieved = store.get("agent-test").unwrap();
    assert_eq!(retrieved.name, "Test Agent");
    assert_eq!(retrieved.status, AgentStatus::Active);

    // Update agent
    let mut updated = retrieved;
    updated.name = "Updated Agent".to_string();
    store.update(&updated).unwrap();

    let retrieved = store.get("agent-test").unwrap();
    assert_eq!(retrieved.name, "Updated Agent");

    // List agents
    let agents = store.list(None).unwrap();
    assert_eq!(agents.len(), 1);

    // Unregister
    store.unregister("agent-test").unwrap();
    store.unregister("agent-test").unwrap();
    assert!(store.get("agent-test").is_err());
}

// --- cas-b157: typed pid_starttime field round-trips through INSERT/UPDATE/SELECT

#[test]
fn test_agent_pid_starttime_round_trips_through_register_and_update() {
    let (_temp, store) = create_test_store();

    // Register with pid_starttime set — INSERT path.
    let mut agent = Agent::new("agent-pidrs".to_string(), "pid-rs".to_string());
    agent.pid = Some(1234);
    agent.pid_starttime = Some(1_616_103);
    store.register(&agent).unwrap();

    let retrieved = store.get("agent-pidrs").unwrap();
    assert_eq!(
        retrieved.pid_starttime,
        Some(1_616_103),
        "INSERT must persist pid_starttime and SELECT must read it back"
    );

    // Update path — different value.
    let mut updated = retrieved;
    updated.pid_starttime = Some(2_000_000);
    store.update(&updated).unwrap();

    let re_retrieved = store.get("agent-pidrs").unwrap();
    assert_eq!(
        re_retrieved.pid_starttime,
        Some(2_000_000),
        "UPDATE must overwrite pid_starttime"
    );
}

#[test]
fn test_agent_factory_session_round_trips_through_register_and_update() {
    let (_temp, store) = create_test_store();

    let mut agent = Agent::new("agent-factory".to_string(), "factory worker".to_string());
    agent.factory_session = Some("factory-session-a".to_string());
    store.register(&agent).unwrap();

    let retrieved = store.get("agent-factory").unwrap();
    assert_eq!(
        retrieved.factory_session.as_deref(),
        Some("factory-session-a"),
        "INSERT must persist factory_session and SELECT must read it back"
    );

    let mut updated = retrieved;
    updated.factory_session = Some("factory-session-b".to_string());
    store.update(&updated).unwrap();

    let re_retrieved = store.get("agent-factory").unwrap();
    assert_eq!(
        re_retrieved.factory_session.as_deref(),
        Some("factory-session-b"),
        "UPDATE must overwrite factory_session"
    );
}

#[test]
fn test_agent_factory_session_stamps_from_env_and_survives_absent_reregister() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", Some("env-session-a"))]);
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-env-factory".to_string(), "env factory".to_string());
    assert!(agent.factory_session.is_none());
    store.register(&agent).unwrap();

    let retrieved = store.get("agent-env-factory").unwrap();
    assert_eq!(retrieved.factory_session.as_deref(), Some("env-session-a"));

    unsafe { std::env::remove_var("CAS_FACTORY_SESSION") };
    let mut reregister = Agent::new("agent-env-factory".to_string(), "env factory 2".to_string());
    reregister.factory_session = None;
    store.register(&reregister).unwrap();

    let retrieved = store.get("agent-env-factory").unwrap();
    assert_eq!(
        retrieved.factory_session.as_deref(),
        Some("env-session-a"),
        "re-register without struct/env session must not erase existing tag"
    );
}

#[test]
fn test_agent_factory_session_update_none_preserves_existing_tag() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", None)]);
    let (_temp, store) = create_test_store();

    let mut agent = Agent::new("agent-update-preserve".to_string(), "preserve".to_string());
    agent.factory_session = Some("session-original".to_string());
    store.register(&agent).unwrap();

    let mut updated = store.get("agent-update-preserve").unwrap();
    updated.name = "preserved after update".to_string();
    updated.factory_session = None;
    store.update(&updated).unwrap();

    let retrieved = store.get("agent-update-preserve").unwrap();
    assert_eq!(
        retrieved.factory_session.as_deref(),
        Some("session-original"),
        "update without struct session must preserve existing tag"
    );
}

#[test]
fn test_agent_pid_starttime_none_round_trips_as_null() {
    // Legacy / non-Linux path: no fingerprint → column is NULL → typed
    // field reads back as None (not Some(0) or a parse-defaulted value).
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-no-fp".to_string(), "no fingerprint".to_string());
    assert!(agent.pid_starttime.is_none());
    store.register(&agent).unwrap();

    let retrieved = store.get("agent-no-fp").unwrap();
    assert!(
        retrieved.pid_starttime.is_none(),
        "None must round-trip as SQL NULL, not coerced to Some(0)"
    );
}

#[test]
fn test_agent_pid_starttime_cleared_on_update_to_none() {
    // Regression: an UPDATE that sets pid_starttime back to None must
    // null the column — otherwise a worker that replaced its process
    // would carry a stale fingerprint forever.
    let (_temp, store) = create_test_store();

    let mut agent = Agent::new("agent-clr".to_string(), "clear".to_string());
    agent.pid_starttime = Some(42);
    store.register(&agent).unwrap();
    assert_eq!(store.get("agent-clr").unwrap().pid_starttime, Some(42));

    let mut updated = agent.clone();
    updated.pid_starttime = None;
    store.update(&updated).unwrap();
    assert!(store.get("agent-clr").unwrap().pid_starttime.is_none());
}

#[test]
fn test_heartbeat() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-hb".to_string(), "Heartbeat Test".to_string());
    store.register(&agent).unwrap();

    let before = store.get("agent-hb").unwrap().last_heartbeat;
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.heartbeat("agent-hb").unwrap();
    let after = store.get("agent-hb").unwrap().last_heartbeat;

    assert!(after > before);
}

#[test]
fn test_lease_claim_and_release() {
    let (_temp, store) = create_test_store();

    // Register agent
    let agent = Agent::new("agent-1".to_string(), "Agent 1".to_string());
    store.register(&agent).unwrap();

    // Claim task
    let result = store
        .try_claim("task-1", "agent-1", 600, Some("Testing"))
        .unwrap();
    assert!(result.is_success());

    let lease = result.lease().unwrap();
    assert_eq!(lease.task_id, "task-1");
    assert_eq!(lease.agent_id, "agent-1");
    assert_eq!(lease.claim_reason, Some("Testing".to_string()));

    // Verify agent's active task count
    let agent = store.get("agent-1").unwrap();
    assert_eq!(agent.active_tasks, 1);

    // Try to claim same task with different agent - should fail
    let agent2 = Agent::new("agent-2".to_string(), "Agent 2".to_string());
    store.register(&agent2).unwrap();

    let result = store.try_claim("task-1", "agent-2", 600, None).unwrap();
    assert!(!result.is_success());
    match result {
        ClaimResult::AlreadyClaimed { held_by, .. } => {
            assert_eq!(held_by, "agent-1");
        }
        _ => panic!("Expected AlreadyClaimed"),
    }

    // Release lease
    store.release_lease("task-1", "agent-1").unwrap();

    // Now agent-2 can claim
    let result = store.try_claim("task-1", "agent-2", 600, None).unwrap();
    assert!(result.is_success());
}

#[test]
fn test_lease_renewal() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-renew".to_string(), "Renew Test".to_string());
    store.register(&agent).unwrap();

    store
        .try_claim("task-renew", "agent-renew", 60, None)
        .unwrap();

    let before = store.get_lease("task-renew").unwrap().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.renew_lease("task-renew", "agent-renew", 120).unwrap();
    let after = store.get_lease("task-renew").unwrap().unwrap();

    assert!(after.expires_at > before.expires_at);
    assert_eq!(after.renewal_count, 1);
}

/// cas-85d9: `heartbeat` must renew ALL of an agent's active task leases,
/// not just worktree leases — this is the root-cause fix for "task leases
/// are never renewed" (found while verifying cas-d165). Before this fix,
/// `renew_lease` had zero production call sites for task leases at all.
#[test]
fn test_heartbeat_renews_task_lease_past_original_duration() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-hb-renew".to_string(), "Heartbeat Renew Test".to_string());
    store.register(&agent).unwrap();

    // Claim with a very short duration — simulates a task claimed near (or
    // past) the default ~30min window that's about to run out.
    store
        .try_claim("task-hb-renew", "agent-hb-renew", 1, None)
        .unwrap();

    let before = store.get_lease("task-hb-renew").unwrap().unwrap();

    // Heartbeat renews the lease to `now + TASK_LEASE_HEARTBEAT_RENEWAL_SECS`
    // (600s in production) — well past the original 1s claim duration.
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.heartbeat("agent-hb-renew").unwrap();

    let after = store.get_lease("task-hb-renew").unwrap().unwrap();
    assert!(
        after.expires_at > before.expires_at,
        "heartbeat must extend the lease's expires_at"
    );
    assert_eq!(
        after.renewal_count, 1,
        "heartbeat-driven renewal must be recorded the same way explicit renew_lease is"
    );

    // Wait past the ORIGINAL 1s claim duration — before cas-85d9, the
    // lease would now be expired and `reclaim_expired_leases` would sweep
    // it, even though the agent just heartbeated.
    std::thread::sleep(std::time::Duration::from_secs(2));

    let reclaimed = store.reclaim_expired_leases().unwrap();
    assert_eq!(
        reclaimed, 0,
        "a heartbeat-renewed lease must survive past its original claim duration"
    );
    assert!(
        store.get_lease("task-hb-renew").unwrap().is_some(),
        "task-hb-renew's lease must still be active after a heartbeat renewed it"
    );
}

/// Safety property: renewal happens ONLY on heartbeat. An agent that never
/// heartbeats after claiming (crashed immediately, or the lease predates
/// its first heartbeat) must still let the lease expire normally — the
/// dead-holder-recovery property the lease exists for is unaffected.
#[test]
fn test_lease_still_expires_without_heartbeat() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-no-hb".to_string(), "No Heartbeat Test".to_string());
    store.register(&agent).unwrap();

    store
        .try_claim("task-no-hb", "agent-no-hb", 1, None)
        .unwrap();
    // No heartbeat call.

    std::thread::sleep(std::time::Duration::from_secs(2));

    let reclaimed = store.reclaim_expired_leases().unwrap();
    assert_eq!(
        reclaimed, 1,
        "a lease with no heartbeat renewal must still expire normally"
    );
}

/// cas-85d9: heartbeat must ALSO renew worktree leases at the store layer.
/// Discovered while auditing task leases: worktree leases nominally had a
/// renewal call site (`cas_agent_heartbeat`, the client-invoked MCP
/// `heartbeat` action), but the actual high-frequency production heartbeat
/// is the daemon's internal PID-liveness loop, which calls this `heartbeat`
/// store method directly and never goes through that MCP handler — so
/// worktree leases had the same latent gap as task leases in practice.
#[test]
fn test_heartbeat_renews_worktree_lease_past_original_duration() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new(
        "agent-hb-wt-renew".to_string(),
        "Heartbeat Worktree Renew Test".to_string(),
    );
    store.register(&agent).unwrap();

    store
        .try_claim_worktree("wt-hb-renew", "agent-hb-wt-renew", 1)
        .unwrap();

    let before = store.get_worktree_lease("wt-hb-renew").unwrap().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));
    store.heartbeat("agent-hb-wt-renew").unwrap();

    let after = store.get_worktree_lease("wt-hb-renew").unwrap().unwrap();
    assert!(
        after.expires_at > before.expires_at,
        "heartbeat must extend the worktree lease's expires_at"
    );

    // Wait past the ORIGINAL 1s claim duration and reclaim — the renewed
    // lease must survive.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let reclaimed = store.reclaim_expired_worktree_leases().unwrap();
    assert_eq!(
        reclaimed, 0,
        "a heartbeat-renewed worktree lease must survive past its original claim duration"
    );
}

/// Heartbeat renewal must be scoped to the heartbeating agent's OWN leases
/// — must not accidentally touch or extend another agent's lease.
#[test]
fn test_heartbeat_renewal_does_not_affect_other_agents_leases() {
    let (_temp, store) = create_test_store();

    let agent_a = Agent::new("agent-hb-a".to_string(), "A".to_string());
    let agent_b = Agent::new("agent-hb-b".to_string(), "B".to_string());
    store.register(&agent_a).unwrap();
    store.register(&agent_b).unwrap();

    store.try_claim("task-a", "agent-hb-a", 1, None).unwrap();
    store.try_claim("task-b", "agent-hb-b", 1, None).unwrap();

    let b_before = store.get_lease("task-b").unwrap().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));
    store.heartbeat("agent-hb-a").unwrap();

    let b_after = store.get_lease("task-b").unwrap().unwrap();
    assert_eq!(
        b_after.expires_at, b_before.expires_at,
        "agent-hb-a's heartbeat must not renew agent-hb-b's lease"
    );
    assert_eq!(b_after.renewal_count, 0);
}

#[test]
fn test_expired_lease_reclaim() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-expire".to_string(), "Expire Test".to_string());
    store.register(&agent).unwrap();

    // Claim with very short duration
    store
        .try_claim("task-expire", "agent-expire", 1, None)
        .unwrap();

    // Wait for expiration
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Reclaim expired
    let count = store.reclaim_expired_leases().unwrap();
    assert_eq!(count, 1);

    // Verify no active lease exists (get_lease only returns active leases)
    let lease = store.get_lease("task-expire").unwrap();
    assert!(
        lease.is_none(),
        "Expired lease should not be returned by get_lease"
    );

    // Verify expiration was logged in history
    let history = store.get_lease_history("task-expire", Some(1)).unwrap();
    assert!(!history.is_empty());
    assert_eq!(history[0].event_type, "expired");

    // Another agent can now claim
    let agent2 = Agent::new("agent-2".to_string(), "Agent 2".to_string());
    store.register(&agent2).unwrap();

    let result = store
        .try_claim("task-expire", "agent-2", 600, None)
        .unwrap();
    assert!(result.is_success());
}

#[test]
fn test_list_agent_leases() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-list".to_string(), "List Test".to_string());
    store.register(&agent).unwrap();

    store.try_claim("task-1", "agent-list", 600, None).unwrap();
    store.try_claim("task-2", "agent-list", 600, None).unwrap();
    store.try_claim("task-3", "agent-list", 600, None).unwrap();

    let leases = store.list_agent_leases("agent-list").unwrap();
    assert_eq!(leases.len(), 3);

    // Verify agent's active task count
    let agent = store.get("agent-list").unwrap();
    assert_eq!(agent.active_tasks, 3);
}

#[test]
fn test_mark_stale_releases_leases() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-stale".to_string(), "Stale Test".to_string());
    store.register(&agent).unwrap();

    store
        .try_claim("task-stale", "agent-stale", 600, None)
        .unwrap();

    // Mark agent as stale
    store.mark_stale("agent-stale").unwrap();

    // Verify agent is stale
    let agent = store.get("agent-stale").unwrap();
    assert_eq!(agent.status, AgentStatus::Stale);

    // Verify no active lease exists (get_lease only returns active leases)
    let lease = store.get_lease("task-stale").unwrap();
    assert!(
        lease.is_none(),
        "Revoked lease should not be returned by get_lease"
    );

    // Verify revocation was logged in history
    let history = store.get_lease_history("task-stale", Some(1)).unwrap();
    assert!(!history.is_empty());
    assert_eq!(history[0].event_type, "revoked");

    // Another agent can now claim
    let agent2 = Agent::new("agent-alive".to_string(), "Alive".to_string());
    store.register(&agent2).unwrap();

    let result = store
        .try_claim("task-stale", "agent-alive", 600, None)
        .unwrap();
    assert!(result.is_success());
}

#[test]
fn release_lease_if_owner_epoch_never_releases_replacement_generation() {
    let (_temp, store) = create_test_store();
    for (id, name) in [("lease-owner", "owner"), ("lease-replacement", "replacement")] {
        store
            .register(&Agent::new(id.to_string(), name.to_string()))
            .unwrap();
    }

    let ClaimResult::Success(original) = store
        .try_claim("task-generation", "lease-owner", 600, None)
        .unwrap()
    else {
        panic!("original lease must be claimed")
    };
    store
        .release_lease("task-generation", "lease-owner")
        .unwrap();
    let ClaimResult::Success(replacement) = store
        .try_claim("task-generation", "lease-replacement", 600, None)
        .unwrap()
    else {
        panic!("replacement lease must be claimed")
    };

    assert!(!store
        .release_lease_if_owner_epoch(
            "task-generation",
            "lease-owner",
            original.epoch,
            "stale completion handoff",
        )
        .unwrap());
    assert_eq!(
        store.get_lease("task-generation").unwrap().unwrap().agent_id,
        "lease-replacement"
    );
    assert!(store
        .release_lease_if_owner_epoch(
            "task-generation",
            "lease-replacement",
            replacement.epoch,
            "exact completion handoff",
        )
        .unwrap());
    assert!(store.get_lease("task-generation").unwrap().is_none());
}

#[test]
fn test_agent_get_handles_legacy_text_active_tasks() {
    let (temp, store) = create_test_store();

    let agent = Agent::new("agent-legacy".to_string(), "Legacy Agent".to_string());
    store.register(&agent).unwrap();

    // Simulate legacy/dirty schema data where active_tasks was stored as TEXT.
    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    conn.execute(
        "UPDATE agents SET active_tasks = ? WHERE id = ?",
        params!["3", "agent-legacy"],
    )
    .unwrap();

    let loaded = store.get("agent-legacy").unwrap();
    assert_eq!(loaded.active_tasks, 3);
}

#[test]
fn test_lease_history_audit_log() {
    let (_temp, store) = create_test_store();

    // Register agent
    let agent = Agent::new("agent-history".to_string(), "History Test".to_string());
    store.register(&agent).unwrap();

    // Claim task with reason
    store
        .try_claim("task-history", "agent-history", 600, Some("Starting work"))
        .unwrap();

    // Verify claim is logged
    let history = store.get_lease_history("task-history", None).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].event_type, "claimed");
    assert_eq!(history[0].agent_id, "agent-history");
    assert_eq!(history[0].epoch, 1);
    assert!(history[0].details.is_some());

    // Renew the lease
    store
        .renew_lease("task-history", "agent-history", 120)
        .unwrap();

    // Verify renewal is logged
    let history = store.get_lease_history("task-history", None).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].event_type, "renewed"); // Most recent first
    assert_eq!(history[1].event_type, "claimed");

    // Release the lease
    store
        .release_lease("task-history", "agent-history")
        .unwrap();

    // Verify release is logged
    let history = store.get_lease_history("task-history", None).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].event_type, "released");

    // Test limit parameter
    let history = store.get_lease_history("task-history", Some(2)).unwrap();
    assert_eq!(history.len(), 2);
}

#[test]
fn test_lease_history_expired() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new(
        "agent-expire-hist".to_string(),
        "Expire History".to_string(),
    );
    store.register(&agent).unwrap();

    // Claim with very short duration
    store
        .try_claim("task-expire-hist", "agent-expire-hist", 1, None)
        .unwrap();

    // Wait for expiration
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Reclaim expired
    store.reclaim_expired_leases().unwrap();

    // Verify expired event is logged
    let history = store.get_lease_history("task-expire-hist", None).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].event_type, "expired");
    assert_eq!(history[1].event_type, "claimed");
}

#[test]
fn test_lease_history_revoked() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new(
        "agent-revoke-hist".to_string(),
        "Revoke History".to_string(),
    );
    store.register(&agent).unwrap();

    store
        .try_claim("task-revoke-hist", "agent-revoke-hist", 600, None)
        .unwrap();

    // Mark agent as stale (revokes lease)
    store.mark_stale("agent-revoke-hist").unwrap();

    // Verify revoked event is logged
    let history = store.get_lease_history("task-revoke-hist", None).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].event_type, "revoked");
    assert_eq!(history[0].reason.as_deref(), Some("agent_stale"));
    assert_eq!(
        history[0].details, None,
        "the canonical reason column must not be duplicated in details JSON"
    );
    assert_eq!(history[1].event_type, "claimed");
}

#[test]
fn test_graceful_shutdown_records_reason_only_in_canonical_column() {
    let (_temp, store) = create_test_store();
    let agent = Agent::new(
        "agent-graceful-history".to_string(),
        "Graceful History".to_string(),
    );
    store.register(&agent).unwrap();
    store
        .try_claim(
            "task-graceful-history",
            "agent-graceful-history",
            600,
            None,
        )
        .unwrap();

    store.graceful_shutdown("agent-graceful-history").unwrap();

    let history = store
        .get_lease_history("task-graceful-history", Some(1))
        .unwrap();
    assert_eq!(history[0].event_type, "released");
    assert_eq!(history[0].reason.as_deref(), Some("graceful_shutdown"));
    assert_eq!(
        history[0].details, None,
        "the canonical reason column must not be duplicated in details JSON"
    );
}

#[test]
fn test_get_agent_worked_tasks() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-worked".to_string(), "Worked Tasks Test".to_string());
    store.register(&agent).unwrap();

    // Claim multiple tasks
    store
        .try_claim("task-1", "agent-worked", 600, None)
        .unwrap();
    store
        .try_claim("task-2", "agent-worked", 600, None)
        .unwrap();
    store
        .try_claim("task-3", "agent-worked", 600, None)
        .unwrap();

    // Release some (simulating task completion)
    store.release_lease("task-1", "agent-worked").unwrap();
    store.release_lease("task-2", "agent-worked").unwrap();

    // get_agent_worked_tasks with None should return ALL tasks that were ever claimed
    // even if they were released
    let worked_tasks = store.get_agent_worked_tasks("agent-worked", None).unwrap();
    assert_eq!(worked_tasks.len(), 3);
    assert!(worked_tasks.contains(&"task-1".to_string()));
    assert!(worked_tasks.contains(&"task-2".to_string()));
    assert!(worked_tasks.contains(&"task-3".to_string()));

    // list_agent_leases should only return active leases (task-3)
    let active_leases = store.list_agent_leases("agent-worked").unwrap();
    assert_eq!(active_leases.len(), 1);
    assert_eq!(active_leases[0].task_id, "task-3");
}

#[test]
fn test_get_agent_worked_tasks_with_since_filter() {
    use chrono::Utc;

    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-filter".to_string(), "Filter Test".to_string());
    store.register(&agent).unwrap();

    // Claim a task
    store
        .try_claim("old-task", "agent-filter", 600, None)
        .unwrap();

    // Sleep briefly and record timestamp
    std::thread::sleep(std::time::Duration::from_millis(50));
    let cutoff = Utc::now();
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Claim another task after the cutoff
    store
        .try_claim("new-task", "agent-filter", 600, None)
        .unwrap();

    // Without filter: both tasks
    let all_tasks = store.get_agent_worked_tasks("agent-filter", None).unwrap();
    assert_eq!(all_tasks.len(), 2);

    // With filter: only new task
    let filtered_tasks = store
        .get_agent_worked_tasks("agent-filter", Some(cutoff))
        .unwrap();
    assert_eq!(filtered_tasks.len(), 1);
    assert!(filtered_tasks.contains(&"new-task".to_string()));
}

#[test]
fn test_working_epics() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-epic".to_string(), "Epic Test".to_string());
    store.register(&agent).unwrap();

    // Add working epics
    store.add_working_epic("agent-epic", "epic-1").unwrap();
    store.add_working_epic("agent-epic", "epic-2").unwrap();
    store.add_working_epic("agent-epic", "epic-1").unwrap(); // Duplicate should be ignored

    // Get working epics
    let epics = store.get_working_epics("agent-epic").unwrap();
    assert_eq!(epics.len(), 2);
    assert!(epics.contains(&"epic-1".to_string()));
    assert!(epics.contains(&"epic-2".to_string()));

    // Remove one epic
    store.remove_working_epic("agent-epic", "epic-1").unwrap();
    let epics = store.get_working_epics("agent-epic").unwrap();
    assert_eq!(epics.len(), 1);
    assert!(epics.contains(&"epic-2".to_string()));

    // Clear all epics
    store.add_working_epic("agent-epic", "epic-3").unwrap();
    store.clear_working_epics("agent-epic").unwrap();
    let epics = store.get_working_epics("agent-epic").unwrap();
    assert_eq!(epics.len(), 0);
}

#[test]
fn test_orphaned_working_epics() {
    let (_temp, store) = create_test_store();

    // Create two agents - one active, one will be marked dead
    let agent_active = Agent::new("agent-active".to_string(), "Active Agent".to_string());
    let agent_dead = Agent::new("agent-dead".to_string(), "Dead Agent".to_string());
    store.register(&agent_active).unwrap();
    store.register(&agent_dead).unwrap();

    // Both agents work on epics
    store
        .add_working_epic("agent-active", "epic-active")
        .unwrap();
    store.add_working_epic("agent-dead", "epic-orphan").unwrap();

    // list_all_working_epics returns both
    let all_epics = store.list_all_working_epics().unwrap();
    assert_eq!(all_epics.len(), 2);

    // While both agents are active, no orphaned epics
    let orphaned = store.list_orphaned_working_epics().unwrap();
    assert_eq!(orphaned.len(), 0);

    // Mark one agent as stale
    store.mark_stale("agent-dead").unwrap();

    // Now the stale agent's epic should be orphaned
    let orphaned = store.list_orphaned_working_epics().unwrap();
    assert_eq!(orphaned.len(), 1);
    assert!(orphaned.contains(&"epic-orphan".to_string()));

    // Active agent's epic should NOT be in orphaned list
    assert!(!orphaned.contains(&"epic-active".to_string()));
}

#[test]
fn test_worker_can_takeover_supervisor_task() {
    let (_temp, store) = create_test_store();

    // Create supervisor agent
    let supervisor = Agent::new("supervisor-1".to_string(), "Supervisor".to_string());
    store.register(&supervisor).unwrap();

    // Create worker agent with supervisor as parent
    let mut worker = Agent::new("worker-1".to_string(), "Worker".to_string());
    worker.parent_id = Some("supervisor-1".to_string());
    store.register(&worker).unwrap();

    // Supervisor claims a task
    let result = store
        .try_claim("task-1", "supervisor-1", 600, Some("planning"))
        .unwrap();
    assert!(matches!(result, ClaimResult::Success(_)));

    // Verify supervisor has the lease
    let lease = store.get_lease("task-1").unwrap().unwrap();
    assert_eq!(lease.agent_id, "supervisor-1");

    // Worker should be able to take over the task from their supervisor
    let result = store
        .try_claim("task-1", "worker-1", 600, Some("executing"))
        .unwrap();
    assert!(matches!(result, ClaimResult::Success(_)));

    // Verify worker now has the lease
    let lease = store.get_lease("task-1").unwrap().unwrap();
    assert_eq!(lease.agent_id, "worker-1");
    assert_eq!(lease.epoch, 2); // Epoch incremented

    // Check lease history shows transfer
    let history = store.get_lease_history("task-1", None).unwrap();
    let transfer_event = history.iter().find(|e| e.event_type == "transferred");
    assert!(transfer_event.is_some(), "Should have a transfer event");
    let transfer = transfer_event.unwrap();
    assert_eq!(transfer.agent_id, "supervisor-1");
}

#[test]
fn test_non_child_cannot_takeover_task() {
    let (_temp, store) = create_test_store();

    // Create two independent agents (no parent relationship)
    let agent1 = Agent::new("agent-1".to_string(), "Agent 1".to_string());
    let agent2 = Agent::new("agent-2".to_string(), "Agent 2".to_string());
    store.register(&agent1).unwrap();
    store.register(&agent2).unwrap();

    // Agent 1 claims a task
    let result = store.try_claim("task-1", "agent-1", 600, None).unwrap();
    assert!(matches!(result, ClaimResult::Success(_)));

    // Agent 2 should NOT be able to take over (no parent relationship)
    let result = store.try_claim("task-1", "agent-2", 600, None).unwrap();
    assert!(matches!(result, ClaimResult::AlreadyClaimed { .. }));

    // Verify agent 1 still has the lease
    let lease = store.get_lease("task-1").unwrap().unwrap();
    assert_eq!(lease.agent_id, "agent-1");
}

#[test]
fn test_list_failed_startup_detects_unconfirmed_agents() {
    let (temp, store) = create_test_store();

    // Register an agent (startup_confirmed defaults to 0)
    let agent = Agent::new("agent-crashed".to_string(), "Crashed Worker".to_string());
    store.register(&agent).unwrap();

    // Backdate registered_at so the agent appears old enough to be detected
    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let old_time = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    conn.execute(
        "UPDATE agents SET registered_at = ?, last_heartbeat = ? WHERE id = ?",
        params![old_time, old_time, "agent-crashed"],
    )
    .unwrap();

    // Should appear as failed startup (registered > 60s ago, never heartbeated)
    let failed = store.list_failed_startup(60).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].id, "agent-crashed");
}

#[test]
fn test_list_failed_startup_ignores_confirmed_agents() {
    let (temp, store) = create_test_store();

    // Register and heartbeat (confirms startup)
    let agent = Agent::new("agent-alive".to_string(), "Alive Worker".to_string());
    store.register(&agent).unwrap();
    store.heartbeat("agent-alive").unwrap();

    // Backdate registered_at
    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let old_time = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    conn.execute(
        "UPDATE agents SET registered_at = ? WHERE id = ?",
        params![old_time, "agent-alive"],
    )
    .unwrap();

    // Should NOT appear as failed startup (heartbeat confirmed startup)
    let failed = store.list_failed_startup(60).unwrap();
    assert!(failed.is_empty());
}

#[test]
fn test_list_failed_startup_ignores_recent_registrations() {
    let (_temp, store) = create_test_store();

    // Register an agent just now (no heartbeat yet, but within grace period)
    let agent = Agent::new("agent-new".to_string(), "New Worker".to_string());
    store.register(&agent).unwrap();

    // Should NOT appear (registered less than 60s ago — still within grace period)
    let failed = store.list_failed_startup(60).unwrap();
    assert!(failed.is_empty());
}

#[test]
fn test_heartbeat_sets_startup_confirmed() {
    let (temp, store) = create_test_store();

    let agent = Agent::new("agent-confirm".to_string(), "Confirm Test".to_string());
    store.register(&agent).unwrap();

    // Before heartbeat: startup_confirmed = 0
    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let confirmed: i64 = conn
        .query_row(
            "SELECT startup_confirmed FROM agents WHERE id = ?",
            params!["agent-confirm"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(confirmed, 0);

    // After heartbeat: startup_confirmed = 1
    store.heartbeat("agent-confirm").unwrap();
    let confirmed: i64 = conn
        .query_row(
            "SELECT startup_confirmed FROM agents WHERE id = ?",
            params!["agent-confirm"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(confirmed, 1);
}

#[test]
fn test_re_registration_preserves_startup_confirmed() {
    let (temp, store) = create_test_store();

    // Register agent, heartbeat to confirm startup
    let agent = Agent::new(
        "agent-reregister".to_string(),
        "ReRegister Test".to_string(),
    );
    store.register(&agent).unwrap();
    store.heartbeat("agent-reregister").unwrap();

    // Verify startup_confirmed = 1
    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let confirmed: i64 = conn
        .query_row(
            "SELECT startup_confirmed FROM agents WHERE id = ?",
            params!["agent-reregister"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(confirmed, 1);

    // Re-register (simulates SessionStart hook re-firing)
    let agent2 = Agent::new(
        "agent-reregister".to_string(),
        "ReRegister Test v2".to_string(),
    );
    store.register(&agent2).unwrap();

    // startup_confirmed must still be 1 (not reset to 0)
    let confirmed: i64 = conn
        .query_row(
            "SELECT startup_confirmed FROM agents WHERE id = ?",
            params!["agent-reregister"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        confirmed, 1,
        "Re-registration must not reset startup_confirmed"
    );

    // Backdate registered_at and verify NOT detected as failed startup
    let old_time = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    conn.execute(
        "UPDATE agents SET registered_at = ? WHERE id = ?",
        params![old_time, "agent-reregister"],
    )
    .unwrap();
    let failed = store.list_failed_startup(60).unwrap();
    assert!(
        failed.is_empty(),
        "Confirmed agent must not appear as failed startup after re-registration"
    );
}

#[test]
fn test_re_registration_preserves_role_and_agent_type_authority() {
    let (_temp, store) = create_test_store();
    let mut worker = Agent::new("agent-authority".to_string(), "worker".to_string());
    worker.role = AgentRole::Worker;
    worker.agent_type = AgentType::Worker;
    store.register(&worker).unwrap();

    let mut forged = Agent::new("agent-authority".to_string(), "supervisor".to_string());
    forged.role = AgentRole::Supervisor;
    forged.agent_type = AgentType::Primary;
    store.register(&forged).unwrap();

    let persisted = store.get("agent-authority").unwrap();
    assert_eq!(persisted.role, AgentRole::Worker);
    assert_eq!(persisted.agent_type, AgentType::Worker);
    assert_eq!(
        persisted.name, "supervisor",
        "non-authority metadata still refreshes"
    );
}

#[test]
fn test_revive_sets_startup_confirmed() {
    let (temp, store) = create_test_store();

    let agent = Agent::new("agent-revive".to_string(), "Revive Test".to_string());
    store.register(&agent).unwrap();

    // Mark stale (startup_confirmed stays 0)
    store.mark_stale("agent-revive").unwrap();

    // Revive — should set startup_confirmed = 1
    store.revive("agent-revive").unwrap();

    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let confirmed: i64 = conn
        .query_row(
            "SELECT startup_confirmed FROM agents WHERE id = ?",
            params!["agent-revive"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        confirmed, 1,
        "Revived agent must have startup_confirmed = 1"
    );

    // Backdate and verify NOT detected as failed startup
    let old_time = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    conn.execute(
        "UPDATE agents SET registered_at = ? WHERE id = ?",
        params![old_time, "agent-revive"],
    )
    .unwrap();
    let failed = store.list_failed_startup(60).unwrap();
    assert!(
        failed.is_empty(),
        "Revived agent must not be detected as failed startup"
    );
}

#[test]
fn test_lease_release_atomically_decrements_active_tasks() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-atomic".to_string(), "Atomic Test".to_string());
    store.register(&agent).unwrap();

    // Claim two tasks
    store
        .try_claim("task-a", "agent-atomic", 600, None)
        .unwrap();
    store
        .try_claim("task-b", "agent-atomic", 600, None)
        .unwrap();

    let agent = store.get("agent-atomic").unwrap();
    assert_eq!(agent.active_tasks, 2);

    // Release one lease — should atomically decrement active_tasks
    store.release_lease("task-a", "agent-atomic").unwrap();

    let agent = store.get("agent-atomic").unwrap();
    assert_eq!(
        agent.active_tasks, 1,
        "active_tasks should decrement atomically on release"
    );

    // Verify lease status changed
    let lease = store.get_lease("task-a").unwrap();
    assert!(lease.is_none(), "Released lease should not be active");

    // Verify release was logged
    let history = store.get_lease_history("task-a", Some(1)).unwrap();
    assert_eq!(history[0].event_type, "released");
}

#[test]
fn test_lease_release_for_task_atomically_decrements_active_tasks() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-fortask".to_string(), "ForTask Test".to_string());
    store.register(&agent).unwrap();

    // Claim a task
    store
        .try_claim("task-close", "agent-fortask", 600, None)
        .unwrap();

    let agent = store.get("agent-fortask").unwrap();
    assert_eq!(agent.active_tasks, 1);

    // Release via release_lease_for_task (used when closing tasks)
    let released = store
        .release_lease_for_task("task-close", "Task closed")
        .unwrap();
    assert!(released, "Should return true when a lease was released");

    let agent = store.get("agent-fortask").unwrap();
    assert_eq!(
        agent.active_tasks, 0,
        "active_tasks should decrement atomically on task-close release"
    );

    // Verify release was logged with a dedicated "Task closed" reason
    let history = store.get_lease_history("task-close", Some(1)).unwrap();
    assert_eq!(history[0].event_type, "released");
    assert_eq!(history[0].reason.as_deref(), Some("Task closed"));
    assert_eq!(history[0].previous_agent_id, None);

    // Release again — should return false (no active lease)
    let released = store
        .release_lease_for_task("task-close", "Task closed")
        .unwrap();
    assert!(!released, "Should return false when no active lease exists");
}

#[test]
fn test_lease_release_for_task_records_awaiting_merge_park_reason() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-park".to_string(), "Park Test".to_string());
    store.register(&agent).unwrap();

    store
        .try_claim("task-park", "agent-park", 600, None)
        .unwrap();

    let released = store
        .release_lease_for_task("task-park", "MERGE REQUIRED: parked awaiting_merge")
        .unwrap();
    assert!(released, "Should return true when a lease was released");

    let history = store.get_lease_history("task-park", Some(1)).unwrap();
    assert_eq!(history[0].event_type, "released");
    assert_eq!(
        history[0].reason.as_deref(),
        Some("MERGE REQUIRED: parked awaiting_merge")
    );
    assert_ne!(
        history[0].reason.as_deref(),
        Some("Task closed"),
        "parked MERGE REQUIRED close rejection must not look like a successful close"
    );
    assert_eq!(history[0].previous_agent_id, None);
}

#[test]
fn test_lease_history_reads_legacy_release_reason_from_previous_agent_id() {
    let (_temp, store) = create_test_store();
    {
        let conn = store.lock_conn().unwrap();
        conn.execute(
            "INSERT INTO task_lease_history
             (task_id, agent_id, event_type, epoch, timestamp, details, previous_agent_id, reason)
             VALUES (?, ?, 'released', 1, ?, NULL, ?, NULL)",
            params![
                "task-legacy-release",
                "agent-legacy",
                Utc::now().to_rfc3339(),
                "Task closed"
            ],
        )
        .unwrap();
    }

    let history = store
        .get_lease_history("task-legacy-release", Some(1))
        .unwrap();
    assert_eq!(history[0].reason.as_deref(), Some("Task closed"));
    assert_eq!(
        history[0].previous_agent_id.as_deref(),
        Some("Task closed"),
        "raw legacy field remains available for serialized compatibility"
    );
}

#[test]
fn test_agent_unregister_releases_leases_atomically() {
    let (_temp, store) = create_test_store();

    let agent = Agent::new("agent-unreg".to_string(), "Unreg Test".to_string());
    store.register(&agent).unwrap();

    // Claim tasks
    store
        .try_claim("task-u1", "agent-unreg", 600, None)
        .unwrap();
    store
        .try_claim("task-u2", "agent-unreg", 600, None)
        .unwrap();

    // Unregister — should atomically release leases and delete agent
    store.unregister("agent-unreg").unwrap();

    // Agent should be gone
    assert!(store.get("agent-unreg").is_err());

    // Leases should be released (not active)
    let lease1 = store.get_lease("task-u1").unwrap();
    assert!(
        lease1.is_none(),
        "Lease should be released after unregister"
    );
    let lease2 = store.get_lease("task-u2").unwrap();
    assert!(
        lease2.is_none(),
        "Lease should be released after unregister"
    );
}
