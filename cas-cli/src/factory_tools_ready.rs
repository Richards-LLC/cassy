//! A Codex factory worker can be sent its assignment before its Cassy MCP
//! server has answered `tools/list`, so the wake arrives while the tool
//! catalog holds no `task` or `coordination` (GH #2128). The server records
//! the moment it first lists its tools; the daemon holds the worker's wake
//! until that record names a live server. No timer stands in for it.
//!
//! The record names the stable front, not the backend that answered: a hot
//! update replaces the backend under a live client that never lists again.
use std::path::{Path, PathBuf};

const DIR: &str = "factory-tools-ready";

/// Request ids the stable front uses for its own catalog probe, which runs before
/// it answers the client's `initialize`. The client holds no tools at that point.
pub(crate) const FRONT_PROBE_PREFIX: &str = "cassy-front-";

/// Set by the front on each backend it starts: the pid that outlives backend swaps.
pub(crate) const FRONT_PID_ENV: &str = "CAS_FRONT_PID";

fn marker(cas_root: &Path, session: &str, worker: &str) -> Option<PathBuf> {
    let safe = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (safe(session) && safe(worker) && !worker.starts_with('.')).then(|| cas_root.join(DIR).join(session).join(worker))
}

/// Called by the worker's MCP server when it lists its tools.
pub(crate) fn record_tools_listed(cas_root: &Path, session: &str, worker: &str, server_pid: u32) -> std::io::Result<()> {
    // A standby server answers tools/list before any store exists and must not create one.
    let Some(path) = marker(cas_root, session, worker).filter(|_| cas_root.is_dir()) else { return Ok(()) };
    std::fs::create_dir_all(path.parent().expect("marker has a parent"))?;
    let staged = path.with_extension(format!("pid{server_pid}"));
    std::fs::write(&staged, server_pid.to_string())?;
    std::fs::rename(staged, path)
}

/// A `tools/list` was answered. Only the external client's listing counts: the
/// front's private probe precedes the client's own tools (GH #2128).
pub(crate) fn note_tools_listed(cas_root: &Path, request_id: &str) {
    if !request_id.starts_with(FRONT_PROBE_PREFIX) {
        record_from_environment(cas_root);
    }
}

/// The factory worker identity this process runs as, if it is one.
pub(crate) fn record_from_environment(cas_root: &Path) {
    let (Ok(session), Ok(worker)) = (std::env::var("CAS_FACTORY_SESSION"), std::env::var("CAS_AGENT_NAME")) else { return };
    if std::env::var("CAS_AGENT_ROLE").as_deref() != Ok("worker") {
        return;
    }
    record_from_environment_as(cas_root, &session, &worker, std::process::id());
}

/// Record the listing for a backend running as `own_pid`. The liveness pid is the
/// front's when the front started this backend, so a swap to a new backend keeps
/// the record live; a server with no front names itself.
fn record_from_environment_as(cas_root: &Path, session: &str, worker: &str, own_pid: u32) {
    let pid = std::env::var(FRONT_PID_ENV).ok().and_then(|pid| pid.parse().ok()).unwrap_or(own_pid);
    if let Err(error) = record_tools_listed(cas_root, session, worker, pid) {
        tracing::warn!(%error, "could not record that this worker's tools are listed");
    }
}

/// A new or recycled worker starts with no record, so a record from its predecessor's
/// still-live server cannot release its first wake early.
pub(crate) fn clear(cas_root: &Path, session: &str, worker: &str) {
    if let Some(path) = marker(cas_root, session, worker) {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether the worker's current MCP server has listed its tools. A record left
/// by a server that has since exited (a recycled worker) does not count.
pub(crate) fn tools_listed(cas_root: &Path, session: &str, worker: &str, pid_alive: impl Fn(u32) -> bool) -> bool {
    marker(cas_root, session, worker)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .is_some_and(pid_alive)
}

/// A backend that is not the front's own process lists tools: `backend_pid` is that
/// backend's pid, which a hot update may retire before the client lists again.
#[cfg(test)]
pub(crate) fn note_listed_by_backend_for_test(cas_root: &Path, request_id: &str, backend_pid: u32) {
    if !request_id.starts_with(FRONT_PROBE_PREFIX)
        && let (Ok(session), Ok(worker)) = (std::env::var("CAS_FACTORY_SESSION"), std::env::var("CAS_AGENT_NAME"))
    {
        record_from_environment_as(cas_root, &session, &worker, backend_pid);
    }
}

#[cfg(test)]
pub(crate) fn forget_for_test(cas_root: &Path, session: &str, worker: &str) {
    std::fs::remove_file(marker(cas_root, session, worker).expect("fixture names are valid")).expect("the fixture had listed its tools");
}
