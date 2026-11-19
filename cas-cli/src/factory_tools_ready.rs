//! A Codex factory worker can be sent its assignment before its Cassy MCP
//! server has answered `tools/list`, so the wake arrives while the tool
//! catalog holds no `task` or `coordination` (GH #2128). The server records
//! the moment it first lists its tools; the daemon holds the worker's wake
//! until that record names a live server. No timer stands in for it.
use std::path::{Path, PathBuf};

const DIR: &str = "factory-tools-ready";

fn marker(cas_root: &Path, session: &str, worker: &str) -> Option<PathBuf> {
    let safe = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (safe(session) && safe(worker) && !worker.starts_with('.')).then(|| cas_root.join(DIR).join(session).join(worker))
}

/// Called by the worker's MCP server when it lists its tools.
pub(crate) fn record_tools_listed(cas_root: &Path, session: &str, worker: &str, server_pid: u32) -> std::io::Result<()> {
    let Some(path) = marker(cas_root, session, worker) else { return Ok(()) };
    std::fs::create_dir_all(path.parent().expect("marker has a parent"))?;
    let staged = path.with_extension(format!("pid{server_pid}"));
    std::fs::write(&staged, server_pid.to_string())?;
    std::fs::rename(staged, path)
}

/// The factory worker identity this process runs as, if it is one.
pub(crate) fn record_from_environment(cas_root: &Path) {
    let (Ok(session), Ok(worker)) = (std::env::var("CAS_FACTORY_SESSION"), std::env::var("CAS_AGENT_NAME")) else { return };
    if std::env::var("CAS_AGENT_ROLE").as_deref() != Ok("worker") {
        return;
    }
    if let Err(error) = record_tools_listed(cas_root, &session, &worker, std::process::id()) {
        tracing::warn!(%error, "could not record that this worker's tools are listed");
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

#[cfg(test)]
pub(crate) fn forget_for_test(cas_root: &Path, session: &str, worker: &str) {
    std::fs::remove_file(marker(cas_root, session, worker).expect("fixture names are valid")).expect("the fixture had listed its tools");
}
