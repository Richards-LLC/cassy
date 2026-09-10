//! Durable registry for agent-launched long-running servers (cas-7c93, GH #87).
//!
//! Containment ([`super::cgroup`], [`super::process_groups`]) makes worker
//! teardown total: every descendant dies, including the ones that left the
//! process group via `setsid`. That is correct for the accidental `npm run dev
//! &` an agent forgot about — and wrong for the dev server a task is supposed
//! to leave running, or that several workers share.
//!
//! This registry is the sanctioned way to run the second kind. A server started
//! through [`start`] is recorded with its pid, port, cwd, owning task and
//! worker, so a supervisor can answer "what is listening and who started it"
//! without `ps`/`lsof` archaeology — and, when registered `shared`, is placed
//! outside the worker's containment scope so teardown does not take it down.
//!
//! Two containment tiers, mirroring [`super::cgroup`]'s:
//!
//! - **Process group.** Every server is spawned into its own session
//!   (`setsid`), so `server_stop` can signal wrappers and watchers as one
//!   unit without reaching the worker. A private server still dies with the
//!   worker because its cgroup remains nested under the worker scope.
//! - **cgroup v2.** A shared server is moved into its own leaf scope, so the
//!   `cgroup.kill` half — which by design has no escape hatch — does not reach
//!   it either. A private server keeps the worker's inherited scope.
//!
//! Registration is therefore not a flag on a record; it is what decides where
//! the process lives. An unregistered process cannot accidentally acquire
//! survival, and a registered-private one does not silently gain it.
//!
//! Nothing here resurrects anything. A record whose pid is gone is marked
//! [`ServerState::Dead`] and stays that way; the registry reports history, it
//! does not restart servers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SERVER_DIR: &str = "factory-servers";
const LOG_DIR: &str = "logs";

/// How long to wait for the launcher shell to publish the server's pid.
const PID_PUBLISH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Grace between SIGTERM and SIGKILL on [`stop`].
const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Lifecycle state of a registry entry.
///
/// `Running` is a claim about the last observation, not a live fact — always
/// resolve through [`liveness`] before acting on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerState {
    /// Started and believed alive.
    Running,
    /// Deliberately stopped through [`stop`].
    Stopped,
    /// Exited on its own (or was killed by something else). Terminal: a dead
    /// entry is never marked running again, whatever later occupies its pid.
    Dead,
}

impl ServerState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ServerState::Running => "running",
            ServerState::Stopped => "stopped",
            ServerState::Dead => "dead",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, ServerState::Stopped | ServerState::Dead)
    }
}

/// Live verdict on a record's pid, resolved against `/proc` right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerLiveness {
    /// The original process is still running.
    Live,
    /// The process is gone (or a zombie awaiting reaping).
    Gone,
    /// The pid exists but belongs to a *different* process — pid reuse. Never
    /// signal it: the fingerprint is the only thing standing between this
    /// registry and killing an innocent bystander.
    Replaced,
    /// No fingerprint was recorded, so identity cannot be proven.
    Unverifiable,
}

/// A registered server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegisteredServer {
    /// Stable registry id (`srv-<hex>`); the handle `server_stop` takes.
    pub id: String,
    /// Operator-facing label, unique-ish but not authoritative.
    pub name: String,
    /// The shell command as the agent wrote it.
    pub command: String,
    pub cwd: PathBuf,
    /// Pid of the server itself — not of the launcher shell.
    pub pid: u32,
    /// Process group the server ended up in, read after launch. This is the
    /// launcher's fresh session in the normal case, of which the server is a
    /// member but usually *not* the leader (it is backgrounded from that
    /// shell). Signalling therefore targets this recorded pgid: `killpg(pid)`
    /// on a non-leader names a process group that does not exist and silently
    /// kills nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
    /// `/proc` start-time fingerprint, the guard against pid reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_starttime: Option<u64>,
    /// Port the caller said this server would listen on. Advisory: the
    /// authoritative answer comes from [`listening_ports`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_worker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_session: Option<String>,
    /// Whether this server was placed outside worker containment.
    pub shared: bool,
    /// Its own cgroup scope, when the host delegates a writable v2 tree.
    /// Private scopes are children of the worker scope; shared scopes are
    /// siblings of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<PathBuf>,
    /// Combined stdout/stderr capture — never inherited, because the MCP
    /// server talks protocol over stdio and a chatty dev server would corrupt
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<PathBuf>,
    pub started_at: DateTime<Utc>,
    pub state: ServerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Why the entry left `Running`, for the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_detail: Option<String>,
}

/// What the caller asked [`start`] for.
#[derive(Debug, Clone)]
pub(crate) struct ServerSpec {
    pub name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub expected_port: Option<u16>,
    pub owner_task: Option<String>,
    pub owner_worker: Option<String>,
    pub factory_session: Option<String>,
    pub shared: bool,
}

/// Result of [`stop`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    /// Signalled and observed to die.
    Stopped { pid: u32, ports: Vec<u16> },
    /// Already gone; the record was reconciled to `Dead`.
    AlreadyGone,
    /// Refused: the pid now belongs to another process (pid reuse), or its
    /// identity cannot be proven. Nothing was signalled.
    RefusedUnverified(ServerLiveness),
}

fn registry_dir(cas_root: &Path) -> PathBuf {
    cas_root.join(SERVER_DIR)
}

fn record_path(cas_root: &Path, id: &str) -> PathBuf {
    registry_dir(cas_root).join(format!("{id}.json"))
}

fn log_dir(cas_root: &Path) -> PathBuf {
    registry_dir(cas_root).join(LOG_DIR)
}

/// Registry ids are filenames: keep them to a charset that cannot escape the
/// registry directory or collide with the `.json` suffix.
fn sanitize_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

/// `srv-<name>-<pid>-<nanos>`: readable in a directory listing, and unique
/// without a counter shared across processes.
fn generate_id(name: &str, pid: u32) -> String {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let stem = sanitize_component(name);
    let stem = if stem.is_empty() { "server" } else { &stem };
    format!("srv-{stem}-{pid}-{unique:08x}")
}

pub(crate) fn write_record(cas_root: &Path, record: &RegisteredServer) -> io::Result<()> {
    let dir = registry_dir(cas_root);
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(record_path(cas_root, &record.id), json)
}

/// Every record, newest first. Unreadable/corrupt files are skipped rather
/// than failing the whole listing — one bad file must not blind a supervisor
/// to the rest.
pub(crate) fn list(cas_root: &Path) -> io::Result<Vec<RegisteredServer>> {
    let dir = registry_dir(cas_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<RegisteredServer>(&contents) {
            Ok(record) => records.push(record),
            Err(error) => tracing::warn!(
                path = %path.display(),
                error = %error,
                "cas-7c93: skipping unreadable server registry record"
            ),
        }
    }
    records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(records)
}

/// Reconcile every record against reality and persist the result.
///
/// This is the only thing that moves an entry out of `Running`, and it moves
/// it one way only: a pid that is gone (or reused by another process) becomes
/// `Dead`. A record that already reached a terminal state is never revisited,
/// so a recycled pid can never resurrect it.
pub(crate) fn refresh(cas_root: &Path) -> io::Result<Vec<RegisteredServer>> {
    let mut records = list(cas_root)?;
    for record in &mut records {
        if record.state.is_terminal() {
            continue;
        }
        let detail = match liveness(record) {
            ServerLiveness::Live | ServerLiveness::Unverifiable => continue,
            ServerLiveness::Gone => "process exited".to_string(),
            ServerLiveness::Replaced => {
                format!("pid {} was reused by another process", record.pid)
            }
        };
        record.state = ServerState::Dead;
        record.ended_at = Some(Utc::now());
        record.ended_detail = Some(detail);
        write_record(cas_root, record)?;
    }
    Ok(records)
}

/// Resolve a registry id, or an exact name, to a record. Ids win over names.
pub(crate) fn find(cas_root: &Path, handle: &str) -> io::Result<Option<RegisteredServer>> {
    let records = list(cas_root)?;
    Ok(records
        .iter()
        .find(|record| record.id == handle)
        .or_else(|| records.iter().find(|record| record.name == handle))
        .cloned())
}

/// Is the recorded process still the process we started?
pub(crate) fn liveness(record: &RegisteredServer) -> ServerLiveness {
    let Some(expected) = record.pid_starttime else {
        return if crate::mcp::daemon::pid_alive(record.pid) {
            ServerLiveness::Unverifiable
        } else {
            ServerLiveness::Gone
        };
    };
    match crate::mcp::daemon::read_pid_starttime(record.pid) {
        // A zombie still has a `/proc` entry and the original start time, but
        // it is not a running server — the launcher shell is gone, so nothing
        // will reap it promptly.
        Some(actual) if actual == expected => {
            if is_zombie(record.pid) {
                ServerLiveness::Gone
            } else {
                ServerLiveness::Live
            }
        }
        Some(_) => ServerLiveness::Replaced,
        None => ServerLiveness::Gone,
    }
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    parse_zombie_state(&stat)
}

/// State field of `/proc/<pid>/stat` == `Z`.
///
/// The `comm` field is parenthesized and may itself contain spaces and
/// parens, so the state is read as the first token after the *last* `)` —
/// splitting on whitespace from the left mis-parses `(my (weird) server)`.
#[cfg(target_os = "linux")]
fn parse_zombie_state(stat: &str) -> bool {
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| state == "Z")
}

#[cfg(not(target_os = "linux"))]
fn is_zombie(_pid: u32) -> bool {
    false
}

/// TCP ports this server is listening on right now, as observed through
/// `/proc`. Empty when nothing is bound, the process is gone, or the host
/// does not expose the information.
pub(crate) fn listening_ports(record: &RegisteredServer) -> Vec<u16> {
    super::cgroup::listening_ports_for_pid_public(record.pid)
}

/// Launch a server and register it.
///
/// The command runs under `sh -c` from a launcher shell that publishes the
/// server's pid and exits immediately, so the server is reparented to init
/// rather than becoming a child of the MCP process — it must outlive the tool
/// call that created it, and must not turn into a zombie nobody reaps.
///
/// stdout/stderr go to a log file, never to the caller's: the MCP server
/// speaks protocol over stdio, and a dev server's banner would corrupt it.
pub(crate) fn start(cas_root: &Path, spec: &ServerSpec) -> io::Result<RegisteredServer> {
    let scope_ops = super::cgroup::SystemScopeOps;
    start_with_scope_ops(cas_root, spec, &scope_ops)
}

/// Launch a server with an explicit containment implementation.
///
/// Production uses [`SystemScopeOps`]. Tests inject an in-memory implementation
/// so child-process assertions can remain focused on process-group behavior
/// without writing the test runner's cgroup or reaping unrelated processes.
#[cfg(test)]
pub(super) fn start_with_scope_ops(
    cas_root: &Path,
    spec: &ServerSpec,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<RegisteredServer> {
    start_inner(cas_root, spec, scope_ops)
}

#[cfg(not(test))]
fn start_with_scope_ops(
    cas_root: &Path,
    spec: &ServerSpec,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<RegisteredServer> {
    start_inner(cas_root, spec, scope_ops)
}

fn start_inner(
    cas_root: &Path,
    spec: &ServerSpec,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<RegisteredServer> {
    if spec.command.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "server command is empty",
        ));
    }
    if !spec.cwd.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("server cwd does not exist: {}", spec.cwd.display()),
        ));
    }

    fs::create_dir_all(log_dir(cas_root))?;
    let stamp = Utc::now().timestamp_millis();
    let log_path = log_dir(cas_root).join(format!("{}-{stamp}.log", {
        let stem = sanitize_component(&spec.name);
        if stem.is_empty() {
            "server".to_string()
        } else {
            stem
        }
    }));

    // The pid handshake file lives in the registry's own directory rather than
    // the system temp dir: `/tmp` is tmpfs on many hosts, and the registry
    // directory is already guaranteed writable here.
    let pid_dir = registry_dir(cas_root).join(".pid");
    fs::create_dir_all(&pid_dir)?;
    let pid_file = pid_dir.join(format!("{stamp}-{}.pid", std::process::id()));
    let launcher_pid_file = pid_dir.join(format!("{stamp}-{}.launcher", std::process::id()));
    let launch_file = pid_dir.join(format!("{stamp}-{}.go", std::process::id()));
    let _pid_file_guard = ScopedFile(pid_file.clone());
    let _launcher_pid_file_guard = ScopedFile(launcher_pid_file.clone());
    let _launch_file_guard = ScopedFile(launch_file.clone());

    // The launcher first publishes its own pid and waits. That barrier is
    // load-bearing: Cassy moves the launcher into the server's dedicated cgroup
    // before it may fork the real command, so PTY wrappers that immediately
    // call setsid cannot escape containment in the gap between spawn and
    // add_pid. Once released, `$!` is the server itself; the launcher publishes
    // it and exits.
    let script = format!(
        "exec >>'{log}' 2>&1; printf '%s' \"$$\" > '{launcher_pid_file}'; \
         while [ ! -f '{launch_file}' ]; do sleep 0.01; done; \
         {command} & printf '%s' \"$!\" > '{pid_file}'",
        log = log_path.display(),
        command = spec.command,
        pid_file = pid_file.display(),
        launcher_pid_file = launcher_pid_file.display(),
        launch_file = launch_file.display(),
    );

    let mut launcher = Command::new("sh");
    launcher
        .arg("-c")
        .arg(&script)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Every server gets its own session/process group. Private servers still
    // remain in the worker's cgroup, while shared servers are moved to a
    // sibling cgroup below; the group boundary is what lets server_stop reach
    // wrappers and nested watchers without signalling the worker.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` between fork and exec. The freshly forked child has
        // a distinct pid and is not a process-group leader, so this is safe
        // and async-signal-safe on Linux and macOS.
        unsafe {
            launcher.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = launcher.spawn()?;
    let launcher_pid = match read_published_pid(&launcher_pid_file) {
        Ok(pid) => pid,
        Err(error) => {
            #[cfg(unix)]
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(error);
        }
    };
    // Every registered server gets a dedicated scope when cgroup v2 is
    // delegated. Private scopes stay below the worker so teardown still owns
    // them; shared scopes are true siblings so teardown cannot reach them.
    let cgroup = if spec.shared {
        scope_ops.join_shared_scope(
            spec.factory_session.as_deref().unwrap_or("no-session"),
            &spec.name,
            launcher_pid,
        )
    } else {
        let scope = scope_ops.create_private_server_scope(
            spec.factory_session.as_deref().unwrap_or("no-session"),
            &spec.name,
        );
        match scope {
            Some(dir) => match scope_ops.add_pid(&dir, launcher_pid) {
                Ok(()) => Some(dir),
                Err(error) => {
                    tracing::warn!(
                        server = %spec.name,
                        launcher_pid,
                        shared = spec.shared,
                        error = %error,
                        "cas-44d2: server launcher could not join its dedicated cgroup; \
                         falling back to process-tree containment"
                    );
                    scope_ops.remove_scope(&dir);
                    None
                }
            },
            None => None,
        }
    };

    if let Err(error) = fs::write(&launch_file, b"go\n") {
        // The launcher has not forked the workload yet. Kill this exact child
        // before returning so a failed registration cannot become an orphan.
        #[cfg(unix)]
        unsafe {
            libc::kill(launcher_pid as libc::pid_t, libc::SIGKILL);
        }
        if let Some(ref dir) = cgroup {
            let _ = scope_ops.kill_scope(dir);
            scope_ops.remove_scope(dir);
        }
        let _ = child.wait();
        return Err(error);
    }

    // The launcher exits as soon as it has published the pid; waiting here is
    // what keeps it from becoming a zombie.
    let status = child.wait()?;
    if !status.success() {
        if let Some(ref dir) = cgroup {
            let _ = scope_ops.kill_scope(dir);
            scope_ops.remove_scope(dir);
        }
        return Err(io::Error::other(format!(
            "server launcher exited with {status}; see {}",
            log_path.display()
        )));
    }

    let pid = match read_published_pid(&pid_file) {
        Ok(pid) => pid,
        Err(error) => {
            if let Some(ref dir) = cgroup {
                let _ = scope_ops.kill_scope(dir);
                scope_ops.remove_scope(dir);
            }
            return Err(error);
        }
    };

    let pgid = process_group_of(pid);
    let record = RegisteredServer {
        id: generate_id(&spec.name, pid),
        name: spec.name.clone(),
        command: spec.command.clone(),
        cwd: spec.cwd.clone(),
        pid,
        pgid,
        pid_starttime: crate::mcp::daemon::read_pid_starttime(pid),
        expected_port: spec.expected_port,
        owner_task: spec.owner_task.clone(),
        owner_worker: spec.owner_worker.clone(),
        factory_session: spec.factory_session.clone(),
        shared: spec.shared,
        cgroup,
        log_path: Some(log_path),
        started_at: Utc::now(),
        state: ServerState::Running,
        ended_at: None,
        ended_detail: None,
    };
    write_record(cas_root, &record)?;
    Ok(record)
}

/// Removes the pid handshake file when `start` returns, by any path.
struct ScopedFile(PathBuf);

impl Drop for ScopedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn read_published_pid(pid_file: &Path) -> io::Result<u32> {
    let deadline = std::time::Instant::now() + PID_PUBLISH_TIMEOUT;
    loop {
        if let Ok(contents) = fs::read_to_string(pid_file) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return Ok(pid);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "server launcher never published a pid",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Stop a registered server and mark the record.
///
/// Signalling is fingerprint-gated: an entry whose pid has been reused, or
/// whose identity cannot be proven, is refused rather than killed. That
/// discipline is why the registry can be trusted to hold pids for hours.
///
/// Every server launched by this registry owns a fresh session/process group.
/// Stop therefore signals that group first, allowing wrappers such as
/// `pnpm start:dev -> nest --watch -> node` to receive the same signal. The
/// original process tree is also fingerprinted and checked after escalation,
/// because a child can deliberately call `setsid` and leave the group.
pub(crate) fn stop(cas_root: &Path, record: &RegisteredServer) -> io::Result<StopOutcome> {
    let scope_ops = super::cgroup::SystemScopeOps;
    stop_with_scope_ops(cas_root, record, &scope_ops)
}

#[cfg(test)]
pub(super) fn stop_with_scope_ops(
    cas_root: &Path,
    record: &RegisteredServer,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<StopOutcome> {
    stop_inner(cas_root, record, scope_ops)
}

#[cfg(not(test))]
fn stop_with_scope_ops(
    cas_root: &Path,
    record: &RegisteredServer,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<StopOutcome> {
    stop_inner(cas_root, record, scope_ops)
}

fn stop_inner(
    cas_root: &Path,
    record: &RegisteredServer,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<StopOutcome> {
    let mut record = record.clone();
    let outcome = match liveness(&record) {
        ServerLiveness::Live => {
            let ports = listening_ports(&record);
            terminate_server(&record, scope_ops)?;
            StopOutcome::Stopped {
                pid: record.pid,
                ports,
            }
        }
        ServerLiveness::Gone if record.cgroup.is_some() => {
            // The registered wrapper may exit before a detached descendant.
            // Its dedicated scope remains authoritative even after reparenting,
            // so drain it before claiming the workload was already gone.
            terminate_server(&record, scope_ops)?;
            StopOutcome::AlreadyGone
        }
        ServerLiveness::Gone => {
            return Err(io::Error::other(format!(
                "registered pid {} for server '{}' is gone, but this legacy record has no \
                 containment scope; server_stop cannot prove that no detached descendants \
                 survived",
                record.pid, record.name
            )));
        }
        other => {
            // Do not touch the record's state on a refusal beyond marking it
            // dead: the server we started is provably no longer there, but
            // whatever holds the pid now is not ours to kill.
            record.state = ServerState::Dead;
            record.ended_at = Some(Utc::now());
            record.ended_detail = Some(format!(
                "refused to signal pid {}: {}",
                record.pid,
                match other {
                    ServerLiveness::Replaced => "pid reused by another process",
                    _ => "identity could not be verified",
                }
            ));
            write_record(cas_root, &record)?;
            return Ok(StopOutcome::RefusedUnverified(other));
        }
    };

    record.state = ServerState::Stopped;
    record.ended_at = Some(Utc::now());
    record.ended_detail = Some(match &outcome {
        StopOutcome::AlreadyGone => "process was already gone".to_string(),
        _ => "stopped by server_stop".to_string(),
    });
    write_record(cas_root, &record)?;
    Ok(outcome)
}

/// Terminate the registered workload and prove that the target and every
/// descendant observed before shutdown are gone.
///
/// A dedicated cgroup is authoritative when available: it includes every
/// descendant even after `setsid`, and `kill_scope` reaches children that are
/// no longer in the process group. Hosts without delegated cgroup v2 use the
/// dedicated process group plus fingerprinted descendant cleanup.
fn terminate_server(
    record: &RegisteredServer,
    scope_ops: &dyn super::cgroup::ScopeOps,
) -> io::Result<()> {
    let initial = process_snapshot();
    if let Some(ref dir) = record.cgroup {
        scope_ops.kill_scope(dir)?;
        scope_ops.remove_scope(dir);
        if scope_ops.cgroup_kill_is_authoritative() {
            return verify_no_survivors(record, &initial, false);
        }
    }

    #[cfg(unix)]
    return terminate_unix_processes(record, &initial);

    #[cfg(not(unix))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "cannot prove termination of server '{}' descendants on this platform",
            record.name
        ),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessDetails {
    pid: u32,
    ppid: u32,
    pgid: Option<u32>,
    starttime: Option<u64>,
    command: String,
    zombie: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    starttime: u64,
}

impl ProcessDetails {
    fn identity(&self) -> Option<ProcessIdentity> {
        self.starttime.map(|starttime| ProcessIdentity {
            pid: self.pid,
            starttime,
        })
    }

    fn is_live(&self) -> bool {
        !self.zombie && self.starttime.is_some()
    }
}

/// Return the currently live descendants of a registered server. The count is
/// deliberately ancestry-based rather than just a process-group count: a
/// watcher that calls `setsid` is still a descendant and must be visible.
pub(crate) fn live_descendant_count(record: &RegisteredServer) -> usize {
    descendants_from_snapshot(record.pid, &process_snapshot())
        .into_iter()
        .filter(ProcessDetails::is_live)
        .count()
}

#[cfg(target_os = "linux")]
fn process_snapshot() -> Vec<ProcessDetails> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (comm, state, ppid, pgid, starttime) = parse_linux_proc_stat(&stat)?;
            let command = fs::read(format!("/proc/{pid}/cmdline"))
                .ok()
                .map(|raw| {
                    raw.split(|byte| *byte == 0)
                        .filter(|part| !part.is_empty())
                        .map(|part| String::from_utf8_lossy(part).into_owned())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|command| !command.is_empty())
                .unwrap_or(comm);
            Some(ProcessDetails {
                pid,
                ppid,
                pgid: Some(pgid),
                starttime: Some(starttime),
                command,
                zombie: state == 'Z',
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn parse_linux_proc_stat(stat: &str) -> Option<(String, char, u32, u32, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_string();
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    let starttime = fields.nth(16)?.parse().ok()?;
    Some((comm, state, ppid, pgid, starttime))
}

#[cfg(target_os = "macos")]
fn process_snapshot() -> Vec<ProcessDetails> {
    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,command="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let ppid = fields.next()?.parse::<u32>().ok()?;
            let pgid = fields.next()?.parse::<u32>().ok()?;
            let command = fields.collect::<Vec<_>>().join(" ");
            Some(ProcessDetails {
                pid,
                ppid,
                pgid: Some(pgid),
                starttime: crate::mcp::daemon::read_pid_starttime(pid),
                command,
                zombie: false,
            })
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_snapshot() -> Vec<ProcessDetails> {
    Vec::new()
}

fn descendants_from_snapshot(pid: u32, snapshot: &[ProcessDetails]) -> Vec<ProcessDetails> {
    let mut descendants = Vec::new();
    let mut parents = vec![pid];
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = parents.pop() {
        for process in snapshot.iter().filter(|process| process.ppid == parent) {
            if !seen.insert(process.pid) {
                continue;
            }
            descendants.push(process.clone());
            parents.push(process.pid);
        }
    }
    descendants
}

fn process_group_members(pgid: u32, snapshot: &[ProcessDetails]) -> Vec<ProcessDetails> {
    snapshot
        .iter()
        .filter(|process| process.pgid == Some(pgid) && process.is_live())
        .cloned()
        .collect()
}

fn initial_identities(
    record: &RegisteredServer,
    snapshot: &[ProcessDetails],
) -> Vec<ProcessIdentity> {
    let mut identities = Vec::new();
    if let Some(starttime) = record.pid_starttime {
        identities.push(ProcessIdentity {
            pid: record.pid,
            starttime,
        });
    }
    for process in descendants_from_snapshot(record.pid, snapshot) {
        if let Some(identity) = process.identity()
            && !identities.contains(&identity)
        {
            identities.push(identity);
        }
    }
    identities
}

fn remaining_processes(
    record: &RegisteredServer,
    initial: &[ProcessDetails],
    include_group: bool,
) -> Vec<ProcessDetails> {
    let snapshot = process_snapshot();
    let initial_identities = initial_identities(record, initial);
    let mut survivors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |process: ProcessDetails| {
        if process.is_live() && seen.insert((process.pid, process.starttime)) {
            survivors.push(process);
        }
    };

    for identity in initial_identities {
        if let Some(current) = snapshot.iter().find(|current| {
            current.pid == identity.pid
                && current.starttime == Some(identity.starttime)
                && current.is_live()
        }) {
            add(current.clone());
        }
    }
    if include_group && let Some(pgid) = record.pgid {
        for process in process_group_members(pgid, &snapshot) {
            add(process);
        }
    }
    if crate::mcp::daemon::read_pid_starttime(record.pid) == record.pid_starttime {
        for process in descendants_from_snapshot(record.pid, &snapshot) {
            add(process);
        }
    }
    survivors
}

fn survivor_detail(record: &RegisteredServer, survivors: &[ProcessDetails]) -> String {
    if survivors.is_empty() {
        return format!("server '{}'", record.name);
    }
    survivors
        .iter()
        .map(|process| format!("pid {} ({})", process.pid, process.command))
        .collect::<Vec<_>>()
        .join(", ")
}

fn verify_no_survivors(
    record: &RegisteredServer,
    initial: &[ProcessDetails],
    include_group: bool,
) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let survivors = remaining_processes(record, initial, include_group);
        if survivors.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "server_stop could not terminate surviving process(es): {}",
                survivor_detail(record, &survivors)
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn process_identity_live(process: ProcessIdentity) -> bool {
    crate::mcp::daemon::read_pid_starttime(process.pid) == Some(process.starttime)
        && !process_is_zombie(process.pid)
}

#[cfg(unix)]
fn signal_fingerprinted(process: ProcessIdentity, signal: libc::c_int) -> io::Result<()> {
    if !process_identity_live(process) {
        return Ok(());
    }
    // SAFETY: the pid's start-time fingerprint was revalidated immediately
    // above; ESRCH is an ordinary exit race.
    let rc = unsafe { libc::kill(process.pid as libc::pid_t, signal) };
    if rc == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
fn process_is_zombie(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parse_linux_proc_stat(&stat).map(|(_, state, _, _, _)| state == 'Z'))
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_is_zombie(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn process_group_is_safe(record: &RegisteredServer) -> bool {
    let Some(pgid) = record.pgid else {
        return false;
    };
    if Some(pgid) == process_group_of(std::process::id()) {
        return false;
    }
    if liveness(record) == ServerLiveness::Live {
        return process_group_of(record.pid) == Some(pgid);
    }
    false
}

#[cfg(unix)]
fn signal_process_group(record: &RegisteredServer, signal: libc::c_int) -> io::Result<bool> {
    if !process_group_is_safe(record) {
        return Ok(false);
    }
    let pgid = record.pgid.expect("safe process group has a pgid");
    signal_process_group_id(pgid, signal)
}

#[cfg(unix)]
fn signal_process_group_id(pgid: u32, signal: libc::c_int) -> io::Result<bool> {
    // SAFETY: process_group_is_safe checked that this is the server's own
    // group, not the Cassy worker's group, immediately before the initial
    // signal. A later escalation reuses the same dedicated group id so a
    // wrapper that exits on SIGTERM cannot hide a still-running child.
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, signal) };
    if rc == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(true)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn terminate_unix_processes(
    record: &RegisteredServer,
    initial_snapshot: &[ProcessDetails],
) -> io::Result<()> {
    let identities = initial_identities(record, initial_snapshot);
    let grouped = signal_process_group(record, libc::SIGTERM)?;
    let grace_deadline = std::time::Instant::now() + STOP_GRACE;
    while std::time::Instant::now() < grace_deadline {
        if remaining_processes(record, initial_snapshot, grouped).is_empty() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    if grouped {
        let pgid = record
            .pgid
            .expect("grouped termination requires a recorded process group");
        let _ = signal_process_group_id(pgid, libc::SIGKILL)?;
    }
    // Group signalling catches the normal watcher tree. Fingerprinted pid
    // cleanup catches children that deliberately escaped with setsid, and is
    // also the safe fallback for legacy records that share the worker group.
    for process in identities.iter().rev() {
        signal_fingerprinted(*process, libc::SIGKILL)?;
    }
    verify_no_survivors(record, initial_snapshot, grouped)
}

/// The process group `pid` belongs to, when the platform can tell us.
#[cfg(unix)]
fn process_group_of(pid: u32) -> Option<u32> {
    // SAFETY: read-only process-table query; -1 on failure.
    let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
    (pgid > 0).then_some(pgid as u32)
}

#[cfg(not(unix))]
fn process_group_of(_pid: u32) -> Option<u32> {
    None
}

/// Which process(es) [`stop`] may signal for this record.
///
/// New records always have a group created by `setsid`, so both private and
/// shared servers use the group target. Records from before GH #796 can still
/// point at the worker's group; those remain pid-only to protect the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, not(target_os = "linux")))]
pub(crate) enum SignalTarget {
    Pid(u32),
    ProcessGroup(u32),
}

#[cfg(any(test, not(target_os = "linux")))]
pub(crate) fn signal_target(record: &RegisteredServer) -> SignalTarget {
    match record.pgid {
        Some(pgid) if Some(pgid) != process_group_of(std::process::id()) => {
            SignalTarget::ProcessGroup(pgid)
        }
        _ => SignalTarget::Pid(record.pid),
    }
}

/// How long a stopped/dead entry stays visible as history before it is pruned.
///
/// Long enough to answer "what did that task leave running, and what happened
/// to it?" the next morning; short enough that the registry does not become an
/// unbounded log.
const HISTORY_RETENTION_HOURS: i64 = 24;

/// Drop terminal records older than [`HISTORY_RETENTION_HOURS`].
///
/// Only ever terminal ones: an entry still claiming `Running` is never pruned,
/// however old, because forgetting a live server is exactly the ambient-orphan
/// state this registry exists to end.
pub(crate) fn prune_history(cas_root: &Path, records: &[RegisteredServer]) -> io::Result<usize> {
    let cutoff = Utc::now() - chrono::Duration::hours(HISTORY_RETENTION_HOURS);
    let mut pruned = 0;
    for record in records {
        if !record.state.is_terminal() {
            continue;
        }
        let ended = record.ended_at.unwrap_or(record.started_at);
        if ended < cutoff {
            forget(cas_root, &record.id)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Drop a terminal record from the registry (history pruning).
pub(crate) fn forget(cas_root: &Path, id: &str) -> io::Result<()> {
    match fs::remove_file(record_path(cas_root, id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "server_registry_tests.rs"]
mod tests;
