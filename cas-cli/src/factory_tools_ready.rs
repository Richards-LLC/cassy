//! A Codex factory worker can be sent its assignment before its Cassy MCP
//! server has answered `tools/list`, so the wake arrives while the tool
//! catalog holds no `task` or `coordination` (GH #2128). The server records
//! the moment it first lists its tools; the daemon holds the worker's wake
//! until that record names a live server. No timer stands in for it.
//!
//! Only a launch the daemon started is held: it writes an awaiting record
//! before the harness starts. A worker with no record (one whose server listed
//! its tools before an upgraded daemon adopted it) never lists again, so
//! holding it would hold its wakes forever (GH #2185).
//!
//! Readiness belongs to each listed front, not the backend that answered. The
//! scalar record stays readable by old daemons; its file revision scopes the
//! companion acknowledgments so an old writer or reset cannot revive them.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fs2::FileExt;
use serde::{Deserialize, Serialize};

const DIR: &str = "factory-tools-ready";
const MAX_FRONTS: usize = 32;

/// Request ids the stable front uses for its own catalog probe, which runs before
/// it answers the client's `initialize`. The client holds no tools at that point.
pub(crate) const FRONT_PROBE_PREFIX: &str = "cassy-front-";

/// Set by the front on each backend it starts: the pid that outlives backend swaps.
pub(crate) const FRONT_PID_ENV: &str = "CAS_FRONT_PID";

fn marker(cas_root: &Path, session: &str, worker: &str) -> Option<PathBuf> {
    let safe = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (safe(session) && safe(worker) && !worker.starts_with('.')).then(|| cas_root.join(DIR).join(session).join(worker))
}

#[derive(Deserialize, PartialEq, Serialize)]
struct Revision {
    modified: SystemTime,
    #[cfg(unix)]
    inode: u64,
}

impl Revision {
    fn read(file: &File) -> io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Self {
            modified: metadata.modified()?,
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

#[derive(Deserialize, Serialize)]
struct Front {
    pid: u32,
    starttime: Option<u64>,
}

impl Front {
    fn new(pid: u32) -> Self {
        Self { pid, starttime: crate::mcp::daemon::read_pid_starttime(pid) }
    }

    fn is_live(&self, pid_alive: &impl Fn(u32) -> bool) -> bool {
        pid_alive(self.pid) && self.starttime.is_none_or(|starttime|
            crate::mcp::daemon::pid_matches_fingerprint(self.pid, starttime))
    }
}

#[derive(Deserialize, Serialize)]
struct Acknowledgments {
    revision: Revision,
    fronts: Vec<Front>,
}

fn read_projection(path: &Path) -> io::Result<(Option<u32>, Revision)> {
    let mut file = File::open(path)?;
    let mut value = String::new();
    (&mut file).take(32).read_to_string(&mut value)?;
    Ok((value.trim().parse().ok(), Revision::read(&file)?))
}

fn acknowledgments(path: &Path, pid: u32, revision: &Revision) -> Option<Vec<Front>> {
    let file = File::open(companion(path, "fronts")).ok()?;
    let state: Acknowledgments = serde_json::from_reader(file.take(8192)).ok()?;
    (state.revision == *revision && state.fronts.len() <= MAX_FRONTS
        && state.fronts.first().is_some_and(|front| front.pid == pid)).then_some(state.fronts)
}

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let worker = path.file_name().expect("worker marker has a name").to_string_lossy();
    path.with_file_name(format!(".{worker}.{suffix}"))
}

fn lock_marker(path: &Path) -> io::Result<File> {
    std::fs::create_dir_all(path.parent().expect("marker has a parent"))?;
    let lock = OpenOptions::new().create(true).truncate(false).write(true)
        .open(companion(path, "lock"))?;
    FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<File> {
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().expect("marker has a parent"))?;
    staged.write_all(bytes)?;
    staged.persist(path).map_err(|error| error.error)
}

/// Called by the worker's MCP server when it lists its tools.
pub(crate) fn record_tools_listed(cas_root: &Path, session: &str, worker: &str, server_pid: u32) -> std::io::Result<()> {
    // A standby server answers tools/list before any store exists and must not create one.
    let Some(path) = marker(cas_root, session, worker).filter(|_| cas_root.is_dir()) else { return Ok(()) };
    let _lock = lock_marker(&path)?;
    let pid_alive = crate::mcp::daemon::pid_alive;
    if !pid_alive(server_pid) {
        return Ok(());
    }
    let current = match read_projection(&path) {
        Ok(projection) => Some(projection),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut fronts = match &current {
        Some((Some(pid), revision)) => acknowledgments(&path, *pid, revision)
            .unwrap_or_else(|| vec![Front::new(*pid)]),
        _ => Vec::new(),
    };
    fronts.retain(|front| front.is_live(&pid_alive));
    if !fronts.iter().any(|front| front.pid == server_pid) {
        if fronts.len() == MAX_FRONTS {
            return Err(io::Error::other("worker readiness front limit reached"));
        }
        fronts.push(Front::new(server_pid));
    }
    let primary = fronts[0].pid;
    let revision = match current {
        Some((Some(pid), revision)) if pid == primary => revision,
        _ => Revision::read(&write_atomically(&path, primary.to_string().as_bytes())?)?,
    };
    let bytes = serde_json::to_vec(&Acknowledgments { revision, fronts })?;
    write_atomically(&companion(&path, "fronts"), &bytes)?;
    Ok(())
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

/// A new or recycled worker starts awaiting its listing, so a record from its
/// predecessor's still-live server cannot release its first wake early.
pub(crate) fn await_listing(cas_root: &Path, session: &str, worker: &str) {
    let Some(path) = marker(cas_root, session, worker).filter(|_| cas_root.is_dir()) else { return };
    let written = lock_marker(&path).and_then(|_lock| write_atomically(&path, AWAITING.as_bytes()));
    if let Err(error) = written {
        tracing::warn!(%error, worker, "could not record that this worker awaits its tools listing");
    }
}

const AWAITING: &str = "awaiting";

/// Whether the worker's current MCP server has listed its tools. An awaiting
/// record, or one left by a server that has since exited, does not count; no
/// record at all is a worker this daemon did not launch, whose tools are listed.
pub(crate) fn tools_listed(cas_root: &Path, session: &str, worker: &str, pid_alive: impl Fn(u32) -> bool) -> bool {
    let Some(path) = marker(cas_root, session, worker) else { return true };
    let Ok((pid, revision)) = read_projection(&path) else { return true };
    let Some(pid) = pid else { return false };
    match acknowledgments(&path, pid, &revision) {
        Some(fronts) => fronts.iter().any(|front| front.is_live(&pid_alive)),
        None => pid_alive(pid),
    }
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
pub(crate) fn path_for_test(cas_root: &Path, session: &str, worker: &str) -> PathBuf {
    marker(cas_root, session, worker).expect("fixture names are valid")
}
