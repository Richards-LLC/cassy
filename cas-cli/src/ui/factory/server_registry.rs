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
//! - **Process group.** A shared server is spawned into its own session
//!   (`setsid`), so the `killpg` half of teardown cannot reach it. A private
//!   server keeps the worker's process group and dies with it.
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
#[cfg(not(target_os = "linux"))]
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
    /// Process group the server ended up in, read after launch.
    ///
    /// For a shared server this is the launcher's new session, of which the
    /// server is a member but usually *not* the leader (it is backgrounded
    /// from that shell). Signalling therefore has to target this recorded
    /// pgid: `killpg(pid)` on a non-leader names a process group that does not
    /// exist and silently kills nothing.
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
    // load-bearing: CAS moves the launcher into the server's dedicated cgroup
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

    // A shared server must leave the worker's process group, or the `killpg`
    // half of teardown reaches it regardless of any registry state. A private
    // one deliberately stays, so it dies with its worker.
    #[cfg(unix)]
    if spec.shared {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` between fork and exec — the same call portable_pty
        // makes for every worker pane. Async-signal-safe.
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
    let scope = if spec.shared {
        super::cgroup::create_server_scope(
            spec.factory_session.as_deref().unwrap_or("no-session"),
            &spec.name,
        )
    } else {
        super::cgroup::create_private_server_scope(
            spec.factory_session.as_deref().unwrap_or("no-session"),
            &spec.name,
        )
    };
    let cgroup = match scope {
        Some(dir) => match super::cgroup::add_pid(&dir, launcher_pid) {
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
                super::cgroup::remove_scope(&dir);
                None
            }
        },
        None => None,
    };

    if let Err(error) = fs::write(&launch_file, b"go\n") {
        // The launcher has not forked the workload yet. Kill this exact child
        // before returning so a failed registration cannot become an orphan.
        #[cfg(unix)]
        unsafe {
            libc::kill(launcher_pid as libc::pid_t, libc::SIGKILL);
        }
        if let Some(ref dir) = cgroup {
            let _ = super::cgroup::kill_scope(dir);
            super::cgroup::remove_scope(dir);
        }
        let _ = child.wait();
        return Err(error);
    }

    // The launcher exits as soon as it has published the pid; waiting here is
    // what keeps it from becoming a zombie.
    let status = child.wait()?;
    if !status.success() {
        if let Some(ref dir) = cgroup {
            let _ = super::cgroup::kill_scope(dir);
            super::cgroup::remove_scope(dir);
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
                let _ = super::cgroup::kill_scope(dir);
                super::cgroup::remove_scope(dir);
            }
            return Err(error);
        }
    };

    let record = RegisteredServer {
        id: generate_id(&spec.name, pid),
        name: spec.name.clone(),
        command: spec.command.clone(),
        cwd: spec.cwd.clone(),
        pid,
        pgid: process_group_of(pid),
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
/// A shared server leads its own session, so its whole process group is
/// signalled — `npm run dev` is a wrapper whose real server is a child. A
/// private server shares the worker's group, so only its own pid is
/// signalled: `killpg` there would take the worker down with it.
pub(crate) fn stop(cas_root: &Path, record: &RegisteredServer) -> io::Result<StopOutcome> {
    let mut record = record.clone();
    let outcome = match liveness(&record) {
        ServerLiveness::Live => {
            let ports = listening_ports(&record);
            terminate_server(&record)?;
            StopOutcome::Stopped {
                pid: record.pid,
                ports,
            }
        }
        ServerLiveness::Gone if record.cgroup.is_some() => {
            // The registered wrapper may exit before a detached descendant.
            // Its dedicated scope remains authoritative even after reparenting,
            // so drain it before claiming the workload was already gone.
            terminate_server(&record)?;
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

/// Terminate the registered workload and prove that the target is gone.
///
/// A dedicated cgroup is authoritative when available: it includes every
/// descendant even after `setsid`, and `kill_scope` now refuses success while
/// members remain. Older records and hosts without delegated cgroup v2 use a
/// platform fallback below.
fn terminate_server(record: &RegisteredServer) -> io::Result<()> {
    if let Some(ref dir) = record.cgroup {
        super::cgroup::kill_scope(dir)?;
        super::cgroup::remove_scope(dir);
        return verify_record_gone(record);
    }

    #[cfg(target_os = "linux")]
    {
        return terminate_linux_process_tree(record);
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    {
        signal_server(record);
        return verify_record_gone(record);
    }

    #[cfg(not(unix))]
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "cannot prove termination of server '{}' descendants on this platform",
                record.name
            ),
        ));
    }
}

fn verify_record_gone(record: &RegisteredServer) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if !matches!(liveness(record), ServerLiveness::Live) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "server_stop could not prove termination; surviving registered pid {} ({})",
                record.pid, record.name
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    starttime: u64,
}

/// Discover and freeze the full Linux descendant tree before killing it.
///
/// Freezing is what closes the fork/reparent race: once the root and every
/// discovered descendant has received SIGSTOP, another scan must be stable
/// before any parent is killed. Every signal is start-time fingerprinted so a
/// recycled pid is never touched.
#[cfg(target_os = "linux")]
fn terminate_linux_process_tree(record: &RegisteredServer) -> io::Result<()> {
    let mut tree = Vec::<ProcessIdentity>::new();
    let root_starttime = record.pid_starttime.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to stop server '{}' without a start-time fingerprint",
                record.name
            ),
        )
    })?;
    tree.push(ProcessIdentity {
        pid: record.pid,
        starttime: root_starttime,
    });
    signal_fingerprinted(tree[0], libc::SIGSTOP)?;

    for _ in 0..8 {
        let before = tree.len();
        let roots: Vec<u32> = tree.iter().map(|proc| proc.pid).collect();
        for pid in roots {
            collect_linux_descendants(pid, &mut tree)?;
        }
        for proc in &tree {
            signal_fingerprinted(*proc, libc::SIGSTOP)?;
        }
        if tree.len() == before {
            break;
        }
    }

    // One final scan after all known members are frozen. Any child found here
    // was forked during the preceding pass; freeze it and include it too.
    let roots: Vec<u32> = tree.iter().map(|proc| proc.pid).collect();
    for pid in roots {
        collect_linux_descendants(pid, &mut tree)?;
    }
    for proc in &tree {
        signal_fingerprinted(*proc, libc::SIGSTOP)?;
    }

    // Children first makes the survivor report easier to interpret and avoids
    // relying on reparenting behavior. SIGKILL cannot be ignored.
    for proc in tree.iter().rev() {
        signal_fingerprinted(*proc, libc::SIGKILL)?;
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let survivors: Vec<_> = tree
            .iter()
            .copied()
            .filter(|proc| process_identity_live(*proc))
            .collect();
        if survivors.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let detail = survivors
                .iter()
                .map(|proc| proc.pid.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(io::Error::other(format!(
                "server_stop could not terminate surviving descendant pid(s): {detail}"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
fn collect_linux_descendants(pid: u32, tree: &mut Vec<ProcessIdentity>) -> io::Result<()> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    let children = match fs::read_to_string(&path) {
        Ok(children) => children,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("cannot inspect descendants through {path}: {error}"),
            ));
        }
    };
    for child in children
        .split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
    {
        if tree.iter().any(|known| known.pid == child) {
            continue;
        }
        if let Some(starttime) = crate::mcp::daemon::read_pid_starttime(child) {
            let identity = ProcessIdentity {
                pid: child,
                starttime,
            };
            // Freeze on discovery, before recursing. If the child forked in
            // the small scan-to-stop gap, the next scan of this now-frozen
            // parent finds that last child deterministically.
            signal_fingerprinted(identity, libc::SIGSTOP)?;
            tree.push(identity);
            collect_linux_descendants(child, tree)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_identity_live(proc: ProcessIdentity) -> bool {
    crate::mcp::daemon::read_pid_starttime(proc.pid) == Some(proc.starttime) && !is_zombie(proc.pid)
}

#[cfg(target_os = "linux")]
fn signal_fingerprinted(proc: ProcessIdentity, signal: libc::c_int) -> io::Result<()> {
    if !process_identity_live(proc) {
        return Ok(());
    }
    // SAFETY: the pid's start-time fingerprint was revalidated immediately
    // above; ESRCH is an ordinary exit race.
    let rc = unsafe { libc::kill(proc.pid as libc::pid_t, signal) };
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
/// A shared server leads (or belongs to) a session of its own, so its whole
/// group is fair game — `npm run dev` is a wrapper whose real server is a
/// child, and killing only the wrapper leaves the port bound. A private server
/// sits in the *worker's* group, so only its own pid may be signalled:
/// `killpg` there would take the worker down with it.
///
/// Pure, so the "never killpg a private server's group" rule is testable
/// without spawning anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, not(target_os = "linux")))]
pub(crate) enum SignalTarget {
    Pid(u32),
    ProcessGroup(u32),
}

#[cfg(any(test, not(target_os = "linux")))]
pub(crate) fn signal_target(record: &RegisteredServer) -> SignalTarget {
    match (record.shared, record.pgid) {
        // Only signal the group when it is genuinely the server's own, not the
        // caller's — a shared server whose setsid failed must not take its
        // launcher's group with it.
        (true, Some(pgid)) if Some(pgid) != process_group_of(std::process::id()) => {
            SignalTarget::ProcessGroup(pgid)
        }
        _ => SignalTarget::Pid(record.pid),
    }
}

/// SIGTERM, brief grace, then SIGKILL to whatever is still there.
#[cfg(all(unix, not(target_os = "linux")))]
fn signal_server(record: &RegisteredServer) {
    let send = |signal: libc::c_int| {
        // SAFETY: identity was fingerprint-validated by the caller immediately
        // above; a signal to an already-dead target fails harmlessly with
        // ESRCH.
        unsafe {
            match signal_target(record) {
                SignalTarget::ProcessGroup(pgid) => libc::killpg(pgid as libc::pid_t, signal),
                SignalTarget::Pid(pid) => libc::kill(pid as libc::pid_t, signal),
            }
        };
    };

    send(libc::SIGTERM);
    let deadline = std::time::Instant::now() + STOP_GRACE;
    while std::time::Instant::now() < deadline {
        if !matches!(liveness(record), ServerLiveness::Live) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    send(libc::SIGKILL);
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
