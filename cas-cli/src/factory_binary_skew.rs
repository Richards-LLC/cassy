//! Read-only comparison of live factory MCP processes with the installed CLI.
//! Old daemons already write binary epochs, so an update can diagnose them
//! without waiting for those processes to learn new telemetry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cas_store::{AgentStore, EPOCH_KIND_DAEMON_START, HistoryStore, SqliteHistoryStore};
use cas_types::{Agent, AgentRole};
use chrono::{DateTime, Utc};

use crate::store::open_agent_store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerVersion {
    pub name: String,
    pub role: AgentRole,
    pub session: Option<String>,
    pub pid: u32,
    pub version: Option<String>,
    pub older: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub installed: String,
    pub servers: Vec<ServerVersion>,
}

fn version_triplet(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.trim_start_matches('v').splitn(4, ['.', '-', '+']);
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn live_local_agent(agent: &Agent, now: DateTime<Utc>, local_machine: &str) -> Option<u32> {
    if !matches!(agent.role, AgentRole::Supervisor | AgentRole::Worker)
        || !agent.is_alive()
        || agent
            .machine_id
            .as_deref()
            .is_some_and(|machine| machine != local_machine)
        || (now - agent.last_heartbeat).num_seconds() > 120
    {
        return None;
    }
    let pid = agent.pid?;
    let fingerprint = agent.pid_starttime.or_else(|| {
        agent
            .metadata
            .get(crate::mcp::daemon::PID_STARTTIME_KEY)?
            .parse()
            .ok()
    })?;
    crate::mcp::daemon::pid_matches_fingerprint(pid, fingerprint).then_some(pid)
}

/// The supplied installed version lets tests exercise the real stores and
/// worker-status handler without modifying the process-global PATH.
pub(crate) fn snapshot_with_installed(
    cas_root: &Path,
    installed: &str,
    now: DateTime<Utc>,
) -> Result<Snapshot, String> {
    let agent_store = open_agent_store(cas_root).map_err(|e| format!("agent roster: {e}"))?;
    let agents = agent_store
        .list(None)
        .map_err(|e| format!("agent roster: {e}"))?;
    let history = SqliteHistoryStore::open(cas_root).map_err(|e| format!("binary epochs: {e}"))?;
    let epochs = history
        .list_epochs(None, 4096)
        .map_err(|e| format!("binary epochs: {e}"))?;
    let local_machine = Agent::get_or_generate_machine_id();
    let mut latest = BTreeMap::new();
    for epoch in epochs {
        if epoch.epoch_kind != EPOCH_KIND_DAEMON_START {
            continue;
        }
        let Some(pid) = epoch.pid.and_then(|pid| u32::try_from(pid).ok()) else {
            continue;
        };
        // A dead process's historical version must never be assigned to a
        // recycled PID. The daemon advances this observation every 30 s.
        let observed = epoch.ended_at.as_deref().unwrap_or(&epoch.started_at);
        let fresh = DateTime::parse_from_rfc3339(observed)
            .ok()
            .is_some_and(|at| (now - at.with_timezone(&Utc)).num_seconds().abs() <= 120);
        if fresh {
            latest.insert(pid, epoch);
        }
    }
    let mut seen = BTreeSet::new();
    let mut servers = Vec::new();
    for agent in agents {
        let Some(pid) = live_local_agent(&agent, now, &local_machine) else {
            continue;
        };
        if !seen.insert(pid) {
            continue;
        }
        let version = latest.get(&pid).and_then(|epoch| {
            let observed = epoch.ended_at.as_deref().unwrap_or(&epoch.started_at);
            DateTime::parse_from_rfc3339(observed)
                .ok()
                .filter(|at| at.with_timezone(&Utc) >= agent.registered_at)
                .and_then(|_| epoch.version.clone())
        });
        let older = version
            .as_deref()
            .and_then(version_triplet)
            .zip(version_triplet(installed))
            .is_some_and(|(running, installed)| running < installed);
        servers.push(ServerVersion {
            name: agent.name,
            role: agent.role,
            session: agent.factory_session,
            pid,
            version,
            older,
        });
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Snapshot {
        installed: installed.to_string(),
        servers,
    })
}

fn installed_version() -> Option<String> {
    static CACHE: OnceLock<Mutex<Option<(String, Instant, Option<String>)>>> = OnceLock::new();
    let path = std::env::var("PATH").unwrap_or_default();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((cached_path, checked_at, version)) = guard.as_ref() {
            if cached_path == &path && checked_at.elapsed() < Duration::from_secs(10) {
                return version.clone();
            }
        }
    }
    let version = crate::bounded_process::run_command(
        Command::new("cas").arg("--version"),
        crate::bounded_process::Deadline::after(Duration::from_millis(500)),
        Duration::from_millis(500),
    )
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| {
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .find(|word| version_triplet(word).is_some())
            .map(|word| word.trim_start_matches('v').to_string())
    });
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((path, Instant::now(), version.clone()));
    }
    version
}

pub(crate) fn snapshot(cas_root: &Path) -> Result<Snapshot, String> {
    let installed = installed_version().ok_or("installed cas version unavailable")?;
    snapshot_with_installed(cas_root, &installed, Utc::now())
}

pub(crate) fn render(snapshot: &Snapshot, session: Option<&str>) -> String {
    let mut lines = vec![format!(
        "Factory MCP versions (installed {}):",
        snapshot.installed
    )];
    for server in snapshot
        .servers
        .iter()
        .filter(|row| session.is_none_or(|s| row.session.as_deref() == Some(s)))
    {
        let state = if server.older {
            "STALE — recycle this session"
        } else if server.version.is_some() {
            "current"
        } else {
            "unknown"
        };
        lines.push(format!(
            "  {} ({:?}, pid {}): {} [{state}]",
            server.name,
            server.role,
            server.pid,
            server.version.as_deref().unwrap_or("unknown")
        ));
    }
    lines.join("\n")
}

pub(crate) fn supervisor_banner(cas_root: &Path, session: Option<&str>) -> Option<String> {
    let snapshot = match snapshot(cas_root) {
        Ok(snapshot) => snapshot,
        Err(reason) => return Some(format!("⚠ Factory MCP versions unavailable: {reason}")),
    };
    let rows: Vec<_> = snapshot
        .servers
        .iter()
        .filter(|row| session.is_none_or(|s| row.session.as_deref() == Some(s)))
        .collect();
    if rows.is_empty() {
        return None;
    }
    let versions = render(&snapshot, session);
    Some(if rows.iter().any(|row| row.older) {
        format!(
            "⚠ Factory MCP servers older than installed Cassy.\n{versions}\nRecycle affected sessions after saving worker state."
        )
    } else {
        versions
    })
}

/// Called by the newly installed post-swap child. Idempotency is per release
/// and factory session; a retry cannot spam the supervisor.
pub(crate) fn notify_after_install(cas_root: &Path, installed: &str) -> Result<usize, String> {
    let snapshot = snapshot_with_installed(cas_root, installed, Utc::now())?;
    let agent_store = open_agent_store(cas_root).map_err(|e| format!("agent roster: {e}"))?;
    let agents = agent_store
        .list(None)
        .map_err(|e| format!("agent roster: {e}"))?;
    let sessions: BTreeSet<_> = snapshot
        .servers
        .iter()
        .filter(|row| row.older)
        .filter_map(|row| row.session.as_deref())
        .collect();
    let mut sent = 0;
    for session in sessions {
        let names = snapshot
            .servers
            .iter()
            .filter(|row| row.older && row.session.as_deref() == Some(session))
            .map(|row| {
                format!(
                    "{} (pid {}, {})",
                    row.name,
                    row.pid,
                    row.version.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let recipient = crate::supervisor_notices::resolve(&agents, None, Some(session));
        let detail = format!(
            "Cassy {installed} is installed, but these factory MCP servers in {session} still run older versions: {names}. Save worker state, then recycle affected workers; restart the supervisor session to load the installed binary. No process was recycled automatically."
        );
        crate::supervisor_notices::enqueue(
            cas_root,
            &recipient,
            "factory_binary_skew",
            &detail,
            &format!("{installed}:{session}"),
        )
        .map_err(|e| format!("supervisor notice: {e}"))?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{HistoryStore, SqliteHistoryStore, SupervisorQueueStore};

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Agent) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut worker =
            Agent::new_with_role("worker-old".into(), "old-worker".into(), AgentRole::Worker);
        worker.factory_session = Some("factory-release".into());
        worker.pid = Some(std::process::id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut worker, std::process::id());
        let store = open_agent_store(&root).unwrap();
        store.register(&worker).unwrap();
        let mut epoch = crate::history::epochs::current_daemon_epoch(&Utc::now().to_rfc3339());
        epoch.version = Some("9.99.1".into());
        SqliteHistoryStore::open(&root)
            .unwrap()
            .record_epoch(&epoch)
            .unwrap();
        (dir, root, worker)
    }

    #[test]
    fn worker_status_flags_live_old_mcp_against_installed_binary() {
        let (_dir, root, worker) = fixture();
        let snapshot = snapshot_with_installed(&root, "9.99.2", Utc::now()).unwrap();
        let status = render(&snapshot, Some("factory-release"));
        assert!(status.contains("installed 9.99.2"), "{status}");
        assert!(status.contains("old-worker (Worker"), "{status}");
        assert!(status.contains("9.99.1 [STALE"), "{status}");

        // A dead/reused PID cannot borrow a historical version and generate
        // a false alert for another process.
        let mut reused = worker;
        reused.pid_starttime = Some(u64::MAX);
        open_agent_store(&root).unwrap().update(&reused).unwrap();
        assert!(
            snapshot_with_installed(&root, "9.99.2", Utc::now())
                .unwrap()
                .servers
                .is_empty()
        );
    }

    #[test]
    fn post_install_sends_one_supervisor_notice_with_stale_session_names() {
        let (_dir, root, _worker) = fixture();
        let mut supervisor = Agent::new_with_role(
            "supervisor-old".into(),
            "release-supervisor".into(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-release".into());
        open_agent_store(&root)
            .unwrap()
            .register(&supervisor)
            .unwrap();
        assert_eq!(notify_after_install(&root, "9.99.2").unwrap(), 1);
        assert_eq!(notify_after_install(&root, "9.99.2").unwrap(), 1);
        let queue = crate::store::open_supervisor_queue_store(&root).unwrap();
        let rows = queue.list_pending(&supervisor.id).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a release retry must not duplicate the notice"
        );
        let payload = &rows[0].payload;
        assert!(payload.contains("old-worker"), "{payload}");
        assert!(payload.contains("factory-release"), "{payload}");
        assert!(payload.contains("recycle affected workers"), "{payload}");
    }
}
