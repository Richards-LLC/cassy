use chrono::Utc;

use crate::daemon::decay::{
    apply_memory_decay, auto_prune, run_consolidation, run_entity_summary_update,
};
use crate::daemon::indexing::generate_bm25_index;
use crate::daemon::observation::process_observations;
use crate::daemon::{DaemonConfig, DaemonRunResult};
use crate::error::CasError;

/// A stale heartbeat is not enough to kill a factory worker.  Codex has no
/// lifecycle hooks, so a worker may remain busy while its heartbeat path is
/// unavailable; a process that identifies itself by argv or `CAS_AGENT_NAME`
/// is independent liveness evidence and wins over the stale timestamp.
///
/// Non-worker agents retain the historical heartbeat-only cleanup policy.
pub(crate) fn heartbeat_stale_agent_should_be_reaped(
    agent: &crate::types::Agent,
    find_live_worker_pid: impl FnOnce(&str) -> Option<u32>,
) -> bool {
    agent.role != crate::types::AgentRole::Worker || find_live_worker_pid(&agent.name).is_none()
}

fn heartbeat_stale_agent_has_live_process(agent: &crate::types::Agent) -> bool {
    !heartbeat_stale_agent_should_be_reaped(agent, |worker_name| {
        crate::cli::factory::wedged::find_worker_pid(
            &crate::cli::factory::wedged::RealProcessTable,
            worker_name,
        )
        .filter(|pid| crate::mcp::daemon::pid_alive(*pid))
    })
}

/// Run a single maintenance cycle.
pub fn run_maintenance(config: &DaemonConfig) -> Result<DaemonRunResult, CasError> {
    use crate::store::{open_agent_store, open_event_store, open_recording_store, open_store};

    let started_at = Utc::now();
    let mut errors = Vec::new();
    let mut observations_processed = 0;
    let mut consolidations_applied = 0;
    let mut entries_pruned = 0;
    let mut decay_applied = 0;
    let mut entries_indexed = 0;
    let mut indexing_errors = Vec::new();
    let mut agents_cleaned = 0;
    let mut agents_purged = 0;
    let mut tasks_interrupted = 0;
    let mut worktrees_cleaned = 0;
    let mut events_pruned = 0;
    let mut lease_history_pruned = 0;
    let mut recordings_pruned = 0;
    let mut trace_archives_evicted = 0;

    let store = open_store(&config.cas_root)?;

    if config.process_observations {
        match process_observations(&store, config) {
            Ok(count) => observations_processed = count,
            Err(error) => errors.push(format!("Observation processing failed: {error}")),
        }
    }

    if config.index_bm25 {
        match generate_bm25_index(&store, config) {
            Ok(result) => {
                entries_indexed = result.indexed;
                for (id, error) in result.errors {
                    indexing_errors.push(format!("{id}: {error}"));
                }
            }
            Err(error) => errors.push(format!("BM25 indexing failed: {error}")),
        }
    }

    if config.apply_decay {
        match apply_memory_decay(&store) {
            Ok(count) => decay_applied = count,
            Err(error) => errors.push(format!("Memory decay failed: {error}")),
        }
    }

    if config.consolidate_memories {
        match run_consolidation(&store, config) {
            Ok(count) => consolidations_applied = count,
            Err(error) => errors.push(format!("Consolidation failed: {error}")),
        }
    }

    if config.auto_prune {
        match auto_prune(&store) {
            Ok(count) => entries_pruned = count,
            Err(error) => errors.push(format!("Auto-prune failed: {error}")),
        }
    }

    let mut entity_summaries_updated = 0;
    if config.update_entity_summaries {
        match run_entity_summary_update(&store, &config.cas_root) {
            Ok(count) => entity_summaries_updated = count,
            Err(error) => errors.push(format!("Entity summary update failed: {error}")),
        }
    }

    if let Ok(agent_store) = open_agent_store(&config.cas_root) {
        // Detect agents that registered but never confirmed startup (90s grace period).
        // These are agents where the MCP server registered in the DB but the Claude Code
        // process never actually started (worktree setup failure, spawn crash, etc.).
        // Grace period is 90s (not 60s) to accommodate the known first-MCP-call timeout.
        if let Ok(failed_startup_agents) = agent_store.list_failed_startup(90) {
            for agent in &failed_startup_agents {
                if heartbeat_stale_agent_has_live_process(agent) {
                    tracing::warn!(
                        worker = %agent.name,
                        agent_id = %agent.id,
                        "heartbeat stale but live factory worker process found; skipping reap"
                    );
                    continue;
                }
                let agent_id = agent.id.clone();

                // Re-check: a heartbeat may have arrived between list_failed_startup
                // and now, confirming the agent. Skip if it's no longer unconfirmed.
                if let Ok(fresh) = agent_store.get(&agent_id) {
                    if !fresh.is_alive() {
                        continue; // Already stale/shutdown
                    }
                }

                // Capture leases before mark_stale revokes them (cas-2e81).
                let held_tasks = agent_store.list_agent_leases(&agent_id).unwrap_or_default();
                let held_ids: Vec<String> = held_tasks.iter().map(|l| l.task_id.clone()).collect();
                if agent_store.mark_stale(&agent_id).is_ok() {
                    agents_cleaned += 1;
                    // cas-2e81: park orphaned InProgress + emit worker_died.
                    let summary =
                        crate::mcp::tools::service::orphan_recovery::recover_worker_vanished(
                            &config.cas_root,
                            agent_store.as_ref(),
                            agent,
                            &held_ids,
                            "daemon maintenance: failed startup",
                        );
                    tasks_interrupted += summary.recovered_task_ids.len();
                }
            }
        }

        if let Ok(stale_agents) = agent_store.list_stale(600) {
            for agent in &stale_agents {
                if heartbeat_stale_agent_has_live_process(agent) {
                    tracing::warn!(
                        worker = %agent.name,
                        agent_id = %agent.id,
                        "heartbeat stale but live factory worker process found; skipping reap"
                    );
                    continue;
                }
                let held_tasks = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
                let held_ids: Vec<String> = held_tasks.iter().map(|l| l.task_id.clone()).collect();
                let agent_id = agent.id.clone();

                if agent_store.mark_stale(&agent_id).is_ok() {
                    agents_cleaned += 1;
                    // cas-2e81: park orphaned InProgress + emit worker_died
                    // (replaces note-only annotation that left status stuck).
                    let summary =
                        crate::mcp::tools::service::orphan_recovery::recover_worker_vanished(
                            &config.cas_root,
                            agent_store.as_ref(),
                            agent,
                            &held_ids,
                            "daemon maintenance: heartbeat stale",
                        );
                    tasks_interrupted += summary.recovered_task_ids.len();
                }
            }
        }

        // Reclaim expired leases; park tasks when holder is already dead.
        let expired: Vec<(String, String)> = agent_store
            .list_active_leases()
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.is_expired())
            .map(|l| (l.task_id, l.agent_id))
            .collect();
        let _ = agent_store.reclaim_expired_leases();
        if !expired.is_empty() {
            let summaries =
                crate::mcp::tools::service::orphan_recovery::recover_expired_leases_for_dead_holders(
                    &config.cas_root,
                    agent_store.as_ref(),
                    &expired,
                    600,
                );
            for s in summaries {
                tasks_interrupted += s.recovered_task_ids.len();
            }
        }

        if config.agent_purge_age_hours > 0 {
            let purge_cutoff =
                Utc::now() - chrono::Duration::hours(config.agent_purge_age_hours as i64);
            if let Ok(all_agents) = agent_store.list(None) {
                for agent in all_agents {
                    if matches!(
                        agent.status,
                        crate::types::AgentStatus::Stale | crate::types::AgentStatus::Shutdown
                    ) && agent.last_heartbeat < purge_cutoff
                        && agent_store.unregister(&agent.id).is_ok()
                    {
                        agents_purged += 1;
                    }
                }
            }
        }
    }

    // Archive old events (30-day live retention).  The archive is written
    // before the live rows are removed, so a failed archive leaves the rows
    // available for the next maintenance cycle.
    if config.auto_prune {
        if let Ok(event_store) = open_event_store(&config.cas_root) {
            match event_store.archive_old(&config.cas_root.join("archive"), 30) {
                Ok(count) => events_pruned = count,
                Err(error) => errors.push(format!("Event archiving failed: {error}")),
            }
        }
    }

    // Clean up old lease history (30-day retention)
    if config.auto_prune {
        if let Ok(agent_store) = open_agent_store(&config.cas_root) {
            match agent_store.cleanup_lease_history(30) {
                Ok(count) => lease_history_pruned = count,
                Err(error) => errors.push(format!("Lease history cleanup failed: {error}")),
            }
        }
    }

    // Archive old recordings (30-day live retention, including agents/events)
    // before deleting their live rows.
    if config.auto_prune {
        if let Ok(recording_store) = open_recording_store(&config.cas_root) {
            match recording_store.archive_old(&config.cas_root.join("archive"), 30) {
                Ok(count) => recordings_pruned = count,
                Err(error) => errors.push(format!("Recording archiving failed: {error}")),
            }
        }
    }

    if config.auto_prune {
        match cas_store::enforce_trace_archive_size(&config.cas_root, config.archive_max_bytes) {
            Ok(eviction) => trace_archives_evicted = eviction.files_evicted,
            Err(error) => errors.push(format!("Trace archive size cap failed: {error}")),
        }
    }

    // WAL checkpoint to prevent unbounded WAL file growth
    {
        let db_path = config.cas_root.join("cas.db");
        if db_path.exists() {
            match cas_store::shared_db::shared_connection(&db_path) {
                Ok(conn) => {
                    if let Ok(conn) = conn.lock() {
                        if let Err(error) =
                            conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
                        {
                            errors.push(format!("WAL checkpoint failed: {error}"));
                        }
                    }
                }
                Err(error) => errors.push(format!("WAL checkpoint connection failed: {error}")),
            }
        }
    }

    match cleanup_orphaned_worktrees(config) {
        Ok(count) => worktrees_cleaned = count,
        Err(error) => errors.push(format!("Worktree cleanup failed: {error}")),
    }

    let ended_at = Utc::now();
    let duration_secs = (ended_at - started_at).num_milliseconds() as f64 / 1000.0;

    Ok(DaemonRunResult {
        started_at,
        ended_at,
        duration_secs,
        observations_processed,
        consolidations_applied,
        entries_pruned,
        decay_applied,
        entries_indexed,
        indexing_errors,
        entity_summaries_updated,
        events_pruned,
        lease_history_pruned,
        recordings_pruned,
        trace_archives_evicted,
        agents_cleaned,
        agents_purged,
        tasks_interrupted,
        worktrees_cleaned,
        errors,
    })
}

/// Clean up orphaned worktrees.
fn cleanup_orphaned_worktrees(config: &DaemonConfig) -> Result<usize, CasError> {
    use crate::config::Config;
    use crate::store::{open_agent_store, open_task_store, open_worktree_store};
    use crate::types::{AgentStatus, TaskStatus};
    use crate::worktree::{WorktreeConfig, WorktreeManager};

    let cas_config = Config::load(&config.cas_root)?;
    let wt_config = cas_config.worktrees();

    if !wt_config.enabled {
        return Ok(0);
    }

    let worktree_store = open_worktree_store(&config.cas_root)?;
    let task_store = open_task_store(&config.cas_root)?;
    let agent_store = open_agent_store(&config.cas_root)?;

    let active_worktrees = worktree_store.list_active()?;
    let mut cleaned = 0;

    for mut worktree in active_worktrees {
        let mut is_orphan = !worktree.path.exists();

        if !is_orphan {
            if let Some(epic_id) = &worktree.epic_id {
                if let Ok(epic) = task_store.get(epic_id) {
                    if matches!(epic.status, TaskStatus::Closed) {
                        is_orphan = true;
                    }
                }
            }
        }

        if !is_orphan {
            if let Some(agent_id) = &worktree.created_by_agent {
                if let Ok(agent) = agent_store.get(agent_id) {
                    if matches!(agent.status, AgentStatus::Stale | AgentStatus::Shutdown) {
                        is_orphan = true;
                    }
                }
            }
        }

        if !is_orphan {
            continue;
        }

        if worktree.path.exists() {
            let manager_config = WorktreeConfig {
                enabled: wt_config.enabled,
                base_path: wt_config.base_path.clone(),
                branch_prefix: wt_config.branch_prefix.clone(),
                auto_merge: wt_config.auto_merge,
                cleanup_on_close: wt_config.cleanup_on_close,
                promote_entries_on_merge: wt_config.promote_entries_on_merge,
            };

            if let Ok(cwd) = std::env::current_dir() {
                if let Ok(manager) = WorktreeManager::new(&cwd, manager_config) {
                    if manager.abandon(&mut worktree, true).is_ok() {
                        worktree.mark_abandoned();
                        worktree.mark_removed();
                        let _ = worktree_store.update(&worktree);
                        cleaned += 1;
                        continue;
                    }
                }
            }
        }

        worktree.mark_abandoned();
        worktree.mark_removed();
        let _ = worktree_store.update(&worktree);
        cleaned += 1;
    }

    Ok(cleaned)
}

/// Run daemon once (for testing or one-shot mode).
pub fn run_once(config: &DaemonConfig) -> Result<DaemonRunResult, CasError> {
    run_maintenance(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_named_codex_worker_with_live_process_is_not_reaped() {
        let mut worker = crate::types::Agent::new(
            "codex-kind-owl-71-session".to_string(),
            "kind-owl-71".to_string(),
        );
        worker.role = crate::types::AgentRole::Worker;
        worker
            .metadata
            .insert("worker_cli".to_string(), "codex".to_string());
        worker.metadata.insert(
            "worker_account_dir".to_string(),
            "/home/operator/.codex-support@example.test".to_string(),
        );

        assert!(
            !heartbeat_stale_agent_should_be_reaped(&worker, |name| {
                (name == "kind-owl-71").then_some(4242)
            }),
            "a stale heartbeat alone must not park a named-account Codex worker while its argv/env-identifiable process is live"
        );
    }

    #[test]
    fn stale_worker_without_live_process_is_reaped() {
        let mut worker = crate::types::Agent::new("dead-worker".to_string(), "dead-owl".to_string());
        worker.role = crate::types::AgentRole::Worker;
        assert!(heartbeat_stale_agent_should_be_reaped(&worker, |_| None));
    }

    /// Incident-shaped cas-66fd regression: a Codex worker on a named account
    /// can go 10+ minutes without a heartbeat because Codex has no hooks, but
    /// its live process still carries `CAS_AGENT_NAME`. Maintenance must not
    /// revoke its lease or park its task on heartbeat staleness alone.
    #[cfg(unix)]
    #[test]
    fn maintenance_keeps_named_account_codex_worker_with_live_env_identity() {
        use crate::store::{init_cas_dir, open_agent_store};
        use std::process::Command;

        let temp = tempfile::tempdir().expect("temp CAS root");
        let cas_root = init_cas_dir(temp.path()).expect("initialize CAS root");
        let worker_name = format!("cas-66fd-live-{}", std::process::id());
        let mut child = Command::new("sleep")
            .arg("60")
            .env("CAS_AGENT_NAME", &worker_name)
            .spawn()
            .expect("spawn identifiable worker process");

        let store = open_agent_store(&cas_root).expect("open agent store");
        let mut worker = crate::types::Agent::new(
            "cas-66fd-live-session".to_string(),
            worker_name.clone(),
        );
        worker.role = crate::types::AgentRole::Worker;
        worker.last_heartbeat = Utc::now() - chrono::Duration::seconds(601);
        worker.metadata.insert("worker_cli".to_string(), "codex".to_string());
        worker.metadata.insert(
            "worker_account_dir".to_string(),
            "/home/operator/.codex-support@example.test".to_string(),
        );
        store.register(&worker).expect("register stale worker");

        let result = run_maintenance(&DaemonConfig {
            cas_root: cas_root.clone(),
            process_observations: false,
            consolidate_memories: false,
            auto_prune: false,
            apply_decay: false,
            update_entity_summaries: false,
            index_code: false,
            index_bm25: false,
            agent_purge_age_hours: 0,
            ..DaemonConfig::default()
        })
        .expect("maintenance succeeds");

        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(result.agents_cleaned, 0);
        assert_eq!(
            store.get(&worker.id).expect("read worker after maintenance").status,
            crate::types::AgentStatus::Active,
            "a live env-identified Codex worker must not be reaped solely for heartbeat age"
        );
    }
}
