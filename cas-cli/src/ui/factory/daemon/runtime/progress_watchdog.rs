//! cas-ddae (GH #1500): the factory progress watchdog.
//!
//! Liveness measures the process, not progress. Three overnight stalls (GH
//! #1302, the Nov 3→4 8h19m stall, and GH #1500's 7 h wait on a silent live
//! peer) each had green heartbeats while nothing moved. Every earlier fix
//! repaired one wake path; none asked "did anything happen?".
//!
//! This sweep answers that question from durable evidence only:
//! a change to any task assigned to this session's agents (status, notes,
//! close or merge), a new commit on one of its workers' lanes, or a new
//! non-daemon message from one of its agents (including replies to peers and
//! the operator). Heartbeats, liveness and MCP reads never count, and the
//! daemon's own relays and fired reminders are excluded.
//!
//! With work in flight (an assigned open, in-progress, blocked or
//! awaiting-merge task, or a pending peer ask) and no progress for N minutes,
//! it wakes the supervisor once through the durable worker-attention relay;
//! still nothing at 2N, it pages the operator once through the operator-turn
//! route (Commander). Both are keyed on the episode, so each fires once until
//! something moves. This is also the supervisor's time backstop: a turn that
//! ends with only event-based wakes is still covered.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// The reminder filter key of a peer ask's deadline (cas-ddae).
pub(crate) const PEER_ASK_FILTER_KEY: &str = "peer_ask";
const WAITING_LINES: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct WatchdogState {
    fingerprint: String,
    /// Start of the current no-progress span: the sweep that last saw
    /// movement, or the first sweep with work in flight.
    since: Option<DateTime<Utc>>,
    woke: bool,
    paged: bool,
}

/// What one sweep did, for the daemon log and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepOutcome {
    Disabled,
    Unreadable,
    NothingInFlight,
    Moving,
    Quiet { minutes: i64 },
    WokeSupervisor { minutes: i64 },
    PagedOperator { minutes: i64 },
}

struct Observation {
    fingerprint: String,
    waiting: Vec<String>,
    supervisor: String,
}

fn state_path(cas_dir: &Path, session: &str) -> PathBuf {
    let file: String = session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    cas_dir.join("progress-watchdog").join(format!("{file}.json"))
}

fn load_state(path: &Path) -> WatchdogState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_state(path: &Path, state: &WatchdogState) {
    let Ok(bytes) = serde_json::to_vec(state) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)).is_err() {
        tracing::warn!(path = %path.display(), "progress watchdog state write failed");
    }
}

/// Read the session's in-flight work and a fingerprint of everything that
/// counts as progress. `None` when a store is unreadable: uncertainty is
/// neither progress nor a stall.
fn observe(cas_dir: &Path, session: &str) -> Option<Observation> {
    use cas_types::{AgentRole, TaskStatus};

    let agents = crate::store::open_agent_store(cas_dir).ok()?.list(None).ok()?;
    let agents: Vec<_> = agents
        .into_iter()
        .filter(|agent| agent.factory_session.as_deref() == Some(session))
        .collect();
    let supervisor = agents
        .iter()
        .find(|agent| agent.role == AgentRole::Supervisor)
        .map(|agent| agent.name.clone())
        .unwrap_or_else(|| "the supervisor".to_string());
    let identities: BTreeSet<String> = agents
        .iter()
        .flat_map(|agent| [agent.name.clone(), agent.id.clone()])
        .collect();
    let names: Vec<String> = agents.iter().map(|agent| agent.name.clone()).collect();

    let mut fingerprint = Vec::new();
    let mut waiting = Vec::new();
    let mut tasks = crate::store::open_task_store_local(cas_dir).ok()?.list(None).ok()?;
    tasks.retain(|task| task.assignee.as_ref().is_some_and(|who| identities.contains(who)));
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    for task in &tasks {
        fingerprint.push(format!("t:{}:{}:{}", task.id, task.status, task.updated_at.to_rfc3339()));
        if matches!(
            task.status,
            TaskStatus::Open | TaskStatus::InProgress | TaskStatus::Blocked | TaskStatus::AwaitingMerge
        ) {
            let title: String = task.title.chars().take(70).collect();
            waiting.push(format!(
                "{} {} ({}): {title}",
                task.id,
                task.status,
                task.assignee.as_deref().unwrap_or_default()
            ));
        }
    }

    let reminders = crate::store::open_reminder_store(cas_dir).ok()?.list_all_pending().ok()?;
    for reminder in reminders.iter().filter(|r| r.session_id.as_deref() == Some(session)) {
        let Some(filter) = reminder.trigger_filter.as_ref() else { continue };
        let Some(ask) = filter.get(PEER_ASK_FILTER_KEY).and_then(|v| v.as_i64()) else { continue };
        let peer = filter.get("peer").and_then(|v| v.as_str()).unwrap_or("a peer");
        waiting.push(format!("ask #{ask} to {peer}, unanswered"));
    }

    let queue = crate::store::open_prompt_queue_store(cas_dir).ok()?;
    let latest = queue.latest_agent_message_id(&names, session).ok()?;
    fingerprint.push(format!("m:{}", latest.unwrap_or(0)));
    fingerprint.extend(lane_heads(cas_dir, &agents));

    Some(Observation { fingerprint: fingerprint.join("\n"), waiting, supervisor })
}

/// `refs/heads/factory/<worker>[-*]` tips for this session's workers. A repo
/// git cannot read contributes nothing (task and message evidence remain).
fn lane_heads(cas_dir: &Path, agents: &[cas_types::Agent]) -> Vec<String> {
    let Some(repo) = cas_dir.parent() else { return Vec::new() };
    let patterns: Vec<String> = agents
        .iter()
        .filter(|agent| agent.role == cas_types::AgentRole::Worker)
        .flat_map(|agent| {
            [format!("refs/heads/factory/{}", agent.name), format!("refs/heads/factory/{}-*", agent.name)]
        })
        .collect();
    if patterns.is_empty() || !repo.join(".git").exists() {
        return Vec::new();
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .args(&patterns)
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| format!("l:{line}"))
            .collect(),
        _ => Vec::new(),
    }
}

fn waiting_text(waiting: &[String]) -> String {
    let mut lines: Vec<String> = waiting.iter().take(WAITING_LINES).map(|line| format!("- {line}")).collect();
    if waiting.len() > WAITING_LINES {
        lines.push(format!("- and {} more", waiting.len() - WAITING_LINES));
    }
    lines.join("\n")
}

/// One watchdog sample for `session` at `now`. `interval_secs` is N; 0 disables.
pub(crate) fn sweep(cas_dir: &Path, session: &str, interval_secs: u64, now: DateTime<Utc>) -> SweepOutcome {
    if interval_secs == 0 {
        return SweepOutcome::Disabled;
    }
    let path = state_path(cas_dir, session);
    let mut state = load_state(&path);
    let Some(observed) = observe(cas_dir, session) else {
        return SweepOutcome::Unreadable;
    };
    // A project hold is the operator's own stop; it is not a stall.
    let in_flight = !observed.waiting.is_empty() && !cas_store::project_hold::is_held(cas_dir);
    let moved = state.fingerprint != observed.fingerprint;
    if !in_flight || moved || state.since.is_none() {
        let outcome = if in_flight { SweepOutcome::Moving } else { SweepOutcome::NothingInFlight };
        let fresh = WatchdogState { fingerprint: observed.fingerprint, since: Some(now), woke: false, paged: false };
        if fresh != state {
            save_state(&path, &fresh);
        }
        return outcome;
    }
    let since = state.since.expect("checked above");
    let quiet_secs = (now - since).num_seconds().max(0);
    let minutes = quiet_secs / 60;
    let n = interval_secs as i64;
    let episode = since.to_rfc3339();
    let waiting = waiting_text(&observed.waiting);
    let mut outcome = SweepOutcome::Quiet { minutes };

    if quiet_secs >= n && !state.woke {
        let detail = format!(
            "Nothing has moved in this factory for {minutes} min with work in flight: no task change, lane commit, merge or message from any agent here. Heartbeats and liveness do not count.\nWaiting:\n{waiting}\nFind who owes the next step and chase it, take it over, or tell the operator. If nothing moves by {} min, the operator is paged.",
            2 * n / 60
        );
        let key = format!("progress-stall:{episode}");
        let relayed = super::lifecycle::enqueue_worker_attention_relay_detail_with_key(
            cas_dir,
            "progress_stalled",
            "supervisor",
            None,
            Some(quiet_secs as u64),
            |_| detail.clone(),
            &episode,
            Some(&key),
        );
        if matches!(relayed, super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }) {
            state.woke = true;
            outcome = SweepOutcome::WokeSupervisor { minutes };
        }
    }
    if quiet_secs >= 2 * n && !state.paged {
        let summary = format!("Factory {session}: nothing moved for {minutes} min");
        let message = format!(
            "Nothing has moved in factory session {session} for {minutes} min, though work is in flight and heartbeats look healthy. Supervisor {} was woken at {} min and nothing has moved since.\nWaiting:\n{waiting}\nCheck the supervisor's pane, chase the peer, or take the work over.",
            observed.supervisor,
            n / 60
        );
        super::lifecycle::record_operator_page(
            cas_dir,
            "progress-watchdog",
            &summary,
            message,
            &format!("progress-stall-page:{session}:{episode}"),
        );
        state.paged = true;
        outcome = SweepOutcome::PagedOperator { minutes };
    }
    save_state(&path, &state);
    if !matches!(outcome, SweepOutcome::Quiet { .. }) {
        tracing::warn!(target: "cas::coordination", stage = "progress_watchdog", ?outcome, session, "factory has work in flight and no progress");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};

    fn agent(cas_dir: &Path, id: &str, name: &str, role: AgentRole, session: &str) {
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.role = role;
        agent.status = AgentStatus::Active;
        agent.factory_session = Some(session.to_string());
        crate::store::open_agent_store(cas_dir).unwrap().register(&agent).unwrap();
    }

    /// GH #1500 / #1302: an overnight with a live, heartbeating but silent
    /// peer and an idle supervisor whose only wakes are event-based. The
    /// supervisor is woken once at N, the operator is paged once at 2N,
    /// heartbeats never reset the clock, and real movement starts a new
    /// episode.
    #[test]
    fn an_overnight_silent_live_peer_wakes_the_supervisor_at_n_and_pages_the_operator_at_2n_gh1500() {
        const SESSION: &str = "overnight-silent-peer";
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", SESSION)]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        agent(&cas_dir, "supervisor-id", "supervisor", AgentRole::Supervisor, SESSION);
        agent(&cas_dir, "worker-id", "busy-otter", AgentRole::Worker, SESSION);
        agent(&cas_dir, "peer-id", "lively-cheetah-57", AgentRole::Supervisor, "peer-session");
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut gate = Task::new("cas-gate".into(), "Release batch awaiting peer review".into());
        gate.status = TaskStatus::AwaitingMerge;
        gate.assignee = Some("busy-otter".into());
        tasks.add(&gate).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let ask = queue.enqueue_with_session("supervisor", "lively-cheetah-57", "ETA on your review?", SESSION).unwrap();
        // The supervisor's only wake is event-based: it can never fire.
        let reminders = crate::store::open_reminder_store(&cas_dir).unwrap();
        reminders
            .create_with_scope("supervisor-id", None, "CI done", cas_store::ReminderTriggerType::Event, None,
                Some("task_completed"), Some(&serde_json::json!({ "task_id": "cas-gate" })), 0, Some(SESSION), None, false, None)
            .unwrap();
        let rows = |target: &str| queue.peek_all(50).unwrap().into_iter().filter(|r| r.target == target).collect::<Vec<_>>();

        let t0 = Utc::now() + chrono::Duration::minutes(1);
        let at = |mins: i64| t0 + chrono::Duration::minutes(mins);
        let sweep = |mins: i64| super::sweep(&cas_dir, SESSION, 45 * 60, at(mins));
        assert_eq!(sweep(0), SweepOutcome::Moving);
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        for mins in [10, 30, 44] {
            agents.heartbeat("worker-id").unwrap();
            agents.heartbeat("peer-id").unwrap();
            assert_eq!(sweep(mins), SweepOutcome::Quiet { minutes: mins }, "heartbeats are not progress");
        }
        assert_eq!(rows("supervisor").len(), 0);

        assert_eq!(sweep(45), SweepOutcome::WokeSupervisor { minutes: 45 });
        let woke = rows("supervisor");
        assert_eq!(woke.len(), 1, "{woke:?}");
        assert!(woke[0].prompt.contains("cas-gate") && woke[0].prompt.contains("45 min"), "{}", woke[0].prompt);
        assert!(crate::prompt_revalidation::parse_blocker_envelope(&woke[0].prompt).is_some());
        assert_eq!(sweep(60), SweepOutcome::Quiet { minutes: 60 }, "the wake itself is not progress");
        assert!(rows("operator").is_empty());

        assert_eq!(sweep(90), SweepOutcome::PagedOperator { minutes: 90 });
        assert_eq!(sweep(7 * 60), SweepOutcome::Quiet { minutes: 420 });
        let paged = rows("operator");
        assert_eq!(paged.len(), 1, "one page per episode: {paged:?}");
        assert!(paged[0].prompt.contains("cas-gate") && paged[0].prompt.contains("supervisor"), "{}", paged[0].prompt);
        assert_eq!(rows("supervisor").len(), 1, "one wake per episode");

        // The peer finally answers and the supervisor acts: a new episode.
        assert!(!queue.ask_answered(ask).unwrap());
        queue.enqueue_with_session("lively-cheetah-57", "supervisor", "approved", "peer-session").unwrap();
        assert!(queue.ask_answered(ask).unwrap());
        queue.enqueue_with_session("supervisor", "busy-otter", "merging now", SESSION).unwrap();
        assert_eq!(sweep(7 * 60 + 1), SweepOutcome::Moving);
        assert_eq!(sweep(7 * 60 + 46), SweepOutcome::WokeSupervisor { minutes: 45 });
    }
}
