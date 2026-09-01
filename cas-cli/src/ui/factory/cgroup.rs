//! cgroup v2 containment for factory worker processes (cas-99f5, GH #86).
//!
//! Process-group containment ([`super::process_groups`]) reaches every
//! descendant that stays in the worker's process group — which is most of
//! them. It cannot reach a descendant that leaves the group on purpose:
//! `setsid(2)`, or Node's `child_process.spawn(..., { detached: true })`,
//! which Playwright's `webServer` and a great many `npm run dev` wrappers use.
//! Those become session leaders of their own and survive `killpg`, which is
//! exactly the orphaned dev server that keeps a port bound after teardown.
//!
//! A cgroup has no such escape hatch: membership is inherited across fork and
//! `setsid` alike, and only an explicit write to another cgroup's `cgroup.procs`
//! can change it. So each worker gets its own leaf cgroup when the host gives us
//! a writable, delegated cgroup v2 tree, and teardown kills the cgroup first and
//! the process group second.
//!
//! Availability is a host property, not a build-time one — an unprivileged
//! container, cgroup v1, or a non-delegated tree all mean "no cgroup tier".
//! Every entry point degrades to `None` with a logged note, leaving PGID
//! containment as the floor.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::sync::Mutex;

/// A process reaped by containment teardown, described for the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReapedProcess {
    pub pid: u32,
    pub comm: String,
    /// TCP ports this process was listening on, when determinable. This is the
    /// detail that matters in practice: "port 5173 is finally free".
    pub ports: Vec<u16>,
}

/// The cgroup operations used by detached workloads.
///
/// Keeping the filesystem and signal boundary behind this seam lets unit tests
/// model containment without ever moving or killing a process in the test
/// runner's own session. Production callers use [`SystemScopeOps`]; tests use
/// [`FakeScopeOps`].
pub(crate) trait ScopeOps {
    /// Whether `kill_scope` actually terminates every member of the scope.
    /// Test doubles report `false` so callers continue through their
    /// fingerprinted process fallback after recording a synthetic reap.
    fn cgroup_kill_is_authoritative(&self) -> bool {
        true
    }

    fn create_scope(&self, factory_session: &str, worker_name: &str) -> Option<PathBuf>;
    fn create_server_scope(&self, factory_session: &str, server_name: &str) -> Option<PathBuf>;
    fn create_private_server_scope(
        &self,
        factory_session: &str,
        server_name: &str,
    ) -> Option<PathBuf>;
    fn join_shared_scope(
        &self,
        factory_session: &str,
        workload_name: &str,
        pid: u32,
    ) -> Option<PathBuf>;
    fn add_pid(&self, dir: &Path, pid: u32) -> io::Result<()>;
    fn kill_scope(&self, dir: &Path) -> io::Result<Vec<ReapedProcess>>;
    fn remove_scope(&self, dir: &Path);
}

/// The real cgroup implementation used outside tests.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SystemScopeOps;

impl ScopeOps for SystemScopeOps {
    fn create_scope(&self, factory_session: &str, worker_name: &str) -> Option<PathBuf> {
        create_scope(factory_session, worker_name)
    }

    fn create_server_scope(&self, factory_session: &str, server_name: &str) -> Option<PathBuf> {
        create_server_scope(factory_session, server_name)
    }

    fn create_private_server_scope(
        &self,
        factory_session: &str,
        server_name: &str,
    ) -> Option<PathBuf> {
        create_private_server_scope(factory_session, server_name)
    }

    fn join_shared_scope(
        &self,
        factory_session: &str,
        workload_name: &str,
        pid: u32,
    ) -> Option<PathBuf> {
        join_shared_scope(factory_session, workload_name, pid)
    }

    fn add_pid(&self, dir: &Path, pid: u32) -> io::Result<()> {
        add_pid(dir, pid)
    }

    fn kill_scope(&self, dir: &Path) -> io::Result<Vec<ReapedProcess>> {
        kill_scope(dir)
    }

    fn remove_scope(&self, dir: &Path) {
        remove_scope(dir)
    }
}

/// In-memory containment model for tests.
///
/// It records scope membership and reaps synthetic process entries, but never
/// writes `/sys/fs/cgroup`, reads the host process table, sends a signal, or
/// changes the current process's session.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FakeScopeOps {
    root: tempfile::TempDir,
    scopes: Mutex<HashMap<PathBuf, Vec<ReapedProcess>>>,
}

#[cfg(test)]
impl Default for FakeScopeOps {
    fn default() -> Self {
        Self {
            root: tempfile::tempdir().expect("fake containment root"),
            scopes: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
impl FakeScopeOps {
    fn new_scope(&self, kind: &str, factory_session: &str, name: &str) -> Option<PathBuf> {
        let dir = self
            .root
            .path()
            .join(prefixed_scope_name(kind, factory_session, name));
        std::fs::create_dir_all(&dir).ok()?;
        self.scopes.lock().ok()?.entry(dir.clone()).or_default();
        Some(dir)
    }

    pub(crate) fn scope_members(&self, dir: &Path) -> Vec<ReapedProcess> {
        self.scopes
            .lock()
            .ok()
            .and_then(|scopes| scopes.get(dir).cloned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl ScopeOps for FakeScopeOps {
    fn cgroup_kill_is_authoritative(&self) -> bool {
        false
    }

    fn create_scope(&self, factory_session: &str, worker_name: &str) -> Option<PathBuf> {
        self.new_scope("cas-worker", factory_session, worker_name)
    }

    fn create_server_scope(&self, factory_session: &str, server_name: &str) -> Option<PathBuf> {
        self.new_scope("cas-server", factory_session, server_name)
    }

    fn create_private_server_scope(
        &self,
        factory_session: &str,
        server_name: &str,
    ) -> Option<PathBuf> {
        self.new_scope("cas-private-server", factory_session, server_name)
    }

    fn join_shared_scope(
        &self,
        factory_session: &str,
        workload_name: &str,
        pid: u32,
    ) -> Option<PathBuf> {
        let dir = self.create_server_scope(factory_session, workload_name)?;
        self.add_pid(&dir, pid).ok()?;
        Some(dir)
    }

    fn add_pid(&self, dir: &Path, pid: u32) -> io::Result<()> {
        let mut scopes = self
            .scopes
            .lock()
            .map_err(|_| io::Error::other("fake containment lock poisoned"))?;
        let members = scopes
            .get_mut(dir)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fake scope missing"))?;
        members.push(ReapedProcess {
            pid,
            comm: "fake-process".to_owned(),
            ports: Vec::new(),
        });
        Ok(())
    }

    fn kill_scope(&self, dir: &Path) -> io::Result<Vec<ReapedProcess>> {
        let mut scopes = self
            .scopes
            .lock()
            .map_err(|_| io::Error::other("fake containment lock poisoned"))?;
        Ok(scopes
            .get_mut(dir)
            .map(std::mem::take)
            .unwrap_or_default())
    }

    fn remove_scope(&self, dir: &Path) {
        if let Ok(mut scopes) = self.scopes.lock() {
            scopes.remove(dir);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The unified cgroup v2 mount point. Every cgroup v2 system mounts here.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Parse this process's cgroup v2 path from `/proc/self/cgroup`.
///
/// The v2 entry is the line with an empty hierarchy id and empty controller
/// list (`0::<path>`). Legacy v1 lines are ignored: they name controllers we
/// cannot use for whole-subtree kills.
fn parse_own_cgroup_path(proc_self_cgroup: &str) -> Option<String> {
    proc_self_cgroup.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        if hierarchy == "0" && controllers.is_empty() && path.starts_with('/') {
            Some(path.to_string())
        } else {
            None
        }
    })
}

/// Read `cgroup.procs`-style content into pids, tolerating trailing junk.
fn parse_procs(contents: &str) -> Vec<u32> {
    contents
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect()
}

/// Map socket inode -> listening port from `/proc/net/tcp{,6}` content.
///
/// Column layout: `sl local_address rem_address st ... inode`. `st == 0A` is
/// TCP_LISTEN; only listeners can be squatting on a port.
fn parse_listening_ports(net_tcp: &str) -> HashMap<u64, u16> {
    let mut ports = HashMap::new();
    for line in net_tcp.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || fields[3] != "0A" {
            continue;
        }
        let Some((_, port_hex)) = fields[1].rsplit_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if let Ok(inode) = fields[9].parse::<u64>() {
            ports.insert(inode, port);
        }
    }
    ports
}

/// Extract the inode from an fd symlink target like `socket:[12345]`.
fn socket_inode(link_target: &str) -> Option<u64> {
    link_target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// Directory name for a worker's containment scope.
///
/// Sanitized because the name lands on the filesystem: anything that is not
/// clearly safe becomes `-`, so a hostile or merely unusual session/worker name
/// cannot escape the parent directory.
fn scope_name(factory_session: &str, worker_name: &str) -> String {
    prefixed_scope_name("cas-worker", factory_session, worker_name)
}

/// Scope directory name for any containment kind.
///
/// `kind` separates the namespaces: a worker scope (`cas-worker-…`, reaped at
/// teardown) and a shared server's own scope (`cas-server-…`, deliberately not
/// reaped — cas-7c93 / GH #87) must never collide, or teardown would take the
/// server with it.
fn prefixed_scope_name(kind: &str, factory_session: &str, name: &str) -> String {
    let sanitize = |value: &str| -> String {
        value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    format!("{kind}-{}-{}", sanitize(factory_session), sanitize(name))
}

/// Human-readable teardown summary: what died, and what it was holding.
pub(crate) fn describe_reaped(reaped: &[ReapedProcess]) -> String {
    if reaped.is_empty() {
        return "no surviving processes".to_string();
    }
    reaped
        .iter()
        .map(|proc| {
            if proc.ports.is_empty() {
                format!("{} ({})", proc.pid, proc.comm)
            } else {
                let ports = proc
                    .ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{} ({}, listening on {})", proc.pid, proc.comm, ports)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The cgroup directory this process belongs to.
#[cfg(target_os = "linux")]
fn own_cgroup_dir() -> Option<PathBuf> {
    let own = parse_own_cgroup_path(&std::fs::read_to_string("/proc/self/cgroup").ok()?)?;
    Some(PathBuf::from(CGROUP_ROOT).join(own.trim_start_matches('/')))
}

#[cfg(not(target_os = "linux"))]
fn own_cgroup_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
pub(super) fn own_scope_for_test() -> Option<PathBuf> {
    own_cgroup_dir()
}

/// Return the calling process's cgroup scope for durable process records.
///
/// This is intentionally best effort: hosts without delegated cgroup v2
/// support still get the process-group containment tier.
pub(crate) fn current_scope() -> Option<PathBuf> {
    own_cgroup_dir()
}

/// The containment root in which worker and shared-server scopes are siblings.
///
/// An MCP server invoked by a worker already runs *inside* that worker's
/// `cas-worker-*` scope. Treating its current cgroup as the root nests a
/// supposedly shared server below the worker, so the next worker teardown
/// kills it. Ascend exactly one level for a recognized Cassy worker scope; never
/// ascend arbitrary host cgroups.
fn containment_root(own: &Path) -> PathBuf {
    let is_worker_scope = own
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("cas-worker-"));
    if is_worker_scope {
        own.parent().unwrap_or(own).to_path_buf()
    } else {
        own.to_path_buf()
    }
}

/// Prove that `parent` is a writable delegated cgroup v2 tree.
fn writable_scope_parent(parent: PathBuf) -> Option<PathBuf> {
    if !parent.join("cgroup.controllers").exists() {
        return None;
    }
    let probe = parent.join(".cas-containment-probe");
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            Some(parent)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_dir(&probe);
            Some(parent)
        }
        Err(_) => None,
    }
}

/// The containment root, if the host provides a writable cgroup v2 tree.
///
/// Worker scopes and shared-server scopes are created here as siblings.
///
/// Writability is proven by actually creating and removing a probe directory —
/// a delegated tree is not implied by the mount existing, and guessing wrong
/// means every worker spawn logs a failure.
#[cfg(target_os = "linux")]
fn writable_parent() -> Option<PathBuf> {
    writable_scope_parent(containment_root(&own_cgroup_dir()?))
}

#[cfg(not(target_os = "linux"))]
fn writable_parent() -> Option<PathBuf> {
    None
}

/// Create a leaf cgroup for a worker and return its directory.
///
/// `None` means the host has no usable cgroup v2 delegation; the caller keeps
/// PGID containment and logs the downgrade once per worker.
pub(crate) fn create_scope(factory_session: &str, worker_name: &str) -> Option<PathBuf> {
    create_named_scope(scope_name(factory_session, worker_name), worker_name)
}

/// Create a leaf cgroup for a **shared registered server** (cas-7c93, GH #87).
///
/// Deliberately a sibling of the worker scopes rather than a child: worker
/// teardown kills its own scope's entire subtree, so a server nested under it
/// would die with the worker no matter what the registry says. This is the
/// placement that makes "registered-shared servers survive teardown" true.
pub(crate) fn create_server_scope(factory_session: &str, server_name: &str) -> Option<PathBuf> {
    create_named_scope(
        prefixed_scope_name("cas-server", factory_session, server_name),
        server_name,
    )
}

/// Place a detached, shared workload in a sibling scope before it forks or
/// execs the real workload. The caller must establish a launch barrier first;
/// cgroup membership is inherited by every later descendant.
pub(crate) fn join_shared_scope(
    factory_session: &str,
    workload_name: &str,
    pid: u32,
) -> Option<PathBuf> {
    let dir = create_server_scope(factory_session, workload_name)?;
    match add_pid(&dir, pid) {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(
                workload = %workload_name,
                pid,
                cgroup = %dir.display(),
                error = %error,
                "cas-8716: detached workload could not join its shared cgroup; \
                 falling back to process-group containment"
            );
            remove_scope(&dir);
            None
        }
    }
}

/// Create a leaf cgroup for a **private registered server**.
///
/// Unlike a shared server, this scope is deliberately nested below the
/// worker's current scope. Explicit `server_stop` can kill just this subtree,
/// while worker teardown still kills it as part of the worker subtree.
pub(crate) fn create_private_server_scope(
    factory_session: &str,
    server_name: &str,
) -> Option<PathBuf> {
    let parent = writable_scope_parent(own_cgroup_dir()?)?;
    create_named_scope_in(
        &parent,
        prefixed_scope_name("cas-private-server", factory_session, server_name),
        server_name,
    )
}

fn create_named_scope(scope: String, label: &str) -> Option<PathBuf> {
    let parent = writable_parent()?;
    create_named_scope_in(&parent, scope, label)
}

fn create_named_scope_in(parent: &Path, scope: String, label: &str) -> Option<PathBuf> {
    let dir = parent.join(scope);
    match std::fs::create_dir(&dir) {
        Ok(()) => Some(dir),
        // A reused worker name in the same session: adopt the existing scope
        // rather than failing the spawn.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Some(dir),
        Err(error) => {
            tracing::warn!(
                scope_for = %label,
                dir = %dir.display(),
                error = %error,
                "cas-99f5: could not create cgroup scope; falling back to process-group containment"
            );
            None
        }
    }
}

/// Move a process (and therefore every descendant it later forks) into the
/// worker's cgroup.
pub(crate) fn add_pid(dir: &Path, pid: u32) -> io::Result<()> {
    std::fs::write(dir.join("cgroup.procs"), format!("{pid}\n"))
}

/// Describe every process currently in the cgroup, best effort.
fn snapshot(dir: &Path) -> Vec<ReapedProcess> {
    let listening = listening_ports_by_inode();
    let mut processes = Vec::new();
    snapshot_into(dir, &listening, &mut processes);
    processes
}

fn snapshot_into(dir: &Path, listening: &HashMap<u64, u16>, processes: &mut Vec<ReapedProcess>) {
    if let Ok(contents) = std::fs::read_to_string(dir.join("cgroup.procs")) {
        processes.extend(parse_procs(&contents).into_iter().map(|pid| {
            ReapedProcess {
                pid,
                comm: std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .map(|comm| comm.trim().to_string())
                    .unwrap_or_else(|_| "unknown".to_string()),
                ports: listening_ports_for_pid(pid, &listening),
            }
        }));
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for child in entries.flatten().filter_map(|entry| {
        entry
            .file_type()
            .ok()
            .filter(|kind| kind.is_dir())
            .map(|_| entry.path())
    }) {
        snapshot_into(&child, listening, processes);
    }
}

fn listening_ports_by_inode() -> HashMap<u64, u16> {
    let mut map = HashMap::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            map.extend(parse_listening_ports(&contents));
        }
    }
    map
}

/// TCP ports `pid` is listening on right now (cas-7c93, GH #87).
///
/// The registry needs the same `/proc` answer teardown reports, so
/// `server_list` says "listening on 5173" from observation rather than from
/// what the caller claimed at registration time.
pub(crate) fn listening_ports_for_pid_public(pid: u32) -> Vec<u16> {
    listening_ports_for_pid(pid, &listening_ports_by_inode())
}

fn listening_ports_for_pid(pid: u32, listening: &HashMap<u64, u16>) -> Vec<u16> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    let mut ports: Vec<u16> = entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.path()).ok()?;
            let inode = socket_inode(target.to_str()?)?;
            listening.get(&inode).copied()
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Kill everything in the worker's cgroup, including descendants that left the
/// process group, and report what died.
///
/// Prefers `cgroup.kill` (kernel 5.14+), which terminates the whole subtree
/// atomically — no fork races. Falls back to SIGKILL per pid.
pub(crate) fn kill_scope(dir: &Path) -> io::Result<Vec<ReapedProcess>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let reaped = snapshot(dir);

    let kill_file = dir.join("cgroup.kill");
    if kill_file.exists() {
        std::fs::write(&kill_file, "1\n")?;
    } else {
        #[cfg(unix)]
        for proc in &reaped {
            // SAFETY: pid read from this cgroup's own procs list moments ago;
            // SIGKILL to a stale pid fails harmlessly with ESRCH.
            unsafe {
                libc::kill(proc.pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let survivors = snapshot(dir);
        if survivors.is_empty() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "cgroup {} still contains surviving processes: {}",
                dir.display(),
                describe_reaped(&survivors)
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(reaped)
}

/// Remove the (expected-empty) scope directory once its processes are gone.
pub(crate) fn remove_scope(dir: &Path) {
    for _ in 0..20 {
        remove_empty_descendant_scopes(dir);
        match std::fs::remove_dir(dir) {
            Ok(()) => return,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            // EBUSY: the kernel has not finished reaping the killed members yet.
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
    tracing::warn!(
        dir = %dir.display(),
        "cas-99f5: worker cgroup scope could not be removed; it may still hold processes"
    );
}

/// Remove empty child cgroups before their Cassy-owned parent. Private server
/// scopes are nested under worker scopes, so worker teardown legitimately
/// leaves an empty hierarchy rather than a leaf.
fn remove_empty_descendant_scopes(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for child in entries.flatten().filter_map(|entry| {
        entry
            .file_type()
            .ok()
            .filter(|kind| kind.is_dir())
            .map(|_| entry.path())
    }) {
        remove_empty_descendant_scopes(&child);
        let _ = std::fs::remove_dir(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_cgroup_path_reads_the_v2_line_only() {
        let v2_only = "0::/user.slice/user-1000.slice/session-3.scope\n";
        assert_eq!(
            parse_own_cgroup_path(v2_only).as_deref(),
            Some("/user.slice/user-1000.slice/session-3.scope")
        );

        let hybrid = "12:pids:/user.slice\n1:name=systemd:/user.slice/session-3.scope\n\
                      0::/user.slice/user-1000.slice/app.slice/app-konsole.scope\n";
        assert_eq!(
            parse_own_cgroup_path(hybrid).as_deref(),
            Some("/user.slice/user-1000.slice/app.slice/app-konsole.scope"),
            "v1 controller lines must never be mistaken for the v2 path"
        );

        let v1_only = "12:pids:/user.slice\n1:name=systemd:/user.slice\n";
        assert_eq!(
            parse_own_cgroup_path(v1_only),
            None,
            "a cgroup v1 host has no v2 tier and must degrade to PGID containment"
        );
    }

    #[test]
    fn procs_parsing_tolerates_blank_and_partial_reads() {
        assert_eq!(parse_procs("123\n456\n"), vec![123, 456]);
        assert_eq!(parse_procs(""), Vec::<u32>::new());
        assert_eq!(parse_procs("123\n\nnot-a-pid\n789\n"), vec![123, 789]);
    }

    #[test]
    fn listening_ports_are_read_from_proc_net_tcp() {
        // Real shape: sl, local_address, rem_address, st, tx/rx, tr/when,
        // retrnsmt, uid, timeout, inode.
        let net_tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: 00000000:1451 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 987654 1 0000 100\n\
             1: 0100007F:C1B7 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 111111 1 0000 100\n";
        let ports = parse_listening_ports(net_tcp);

        assert_eq!(
            ports.get(&987654).copied(),
            Some(5201),
            "0x1451 = 5201 must be reported for the listening socket"
        );
        assert!(
            !ports.contains_key(&111111),
            "an established (st=01) connection is not squatting on a port"
        );
    }

    #[test]
    fn socket_inode_is_extracted_from_the_fd_link() {
        assert_eq!(socket_inode("socket:[987654]"), Some(987654));
        assert_eq!(socket_inode("/dev/pts/3"), None);
        assert_eq!(socket_inode("socket:[]"), None);
    }

    #[test]
    fn scope_names_cannot_escape_the_parent_directory() {
        let name = scope_name("../../evil", "worker/../..");
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains(".."), "{name}");
        assert_eq!(
            scope_name("cas-src-mighty-crane-74", "cosmic-crow-41"),
            "cas-worker-cas-src-mighty-crane-74-cosmic-crow-41"
        );
    }

    #[test]
    fn shared_scope_root_ascends_out_of_a_worker_scope_only() {
        let worker = Path::new("/delegated/session/cas-worker-factory-worker-a");
        assert_eq!(
            containment_root(worker),
            PathBuf::from("/delegated/session")
        );

        let ordinary = Path::new("/delegated/session");
        assert_eq!(containment_root(ordinary), ordinary);

        let lookalike = Path::new("/delegated/session/not-cas-worker-a");
        assert_eq!(containment_root(lookalike), lookalike);
    }

    #[test]
    fn reap_summary_names_pids_comms_and_ports() {
        let summary = describe_reaped(&[
            ReapedProcess {
                pid: 4242,
                comm: "node".into(),
                ports: vec![5173, 24678],
            },
            ReapedProcess {
                pid: 4243,
                comm: "esbuild".into(),
                ports: vec![],
            },
        ]);
        assert!(
            summary.contains("4242 (node, listening on 5173,24678)"),
            "{summary}"
        );
        assert!(summary.contains("4243 (esbuild)"), "{summary}");
        assert_eq!(describe_reaped(&[]), "no surviving processes");
    }

    /// GH #86's containment contract: a descendant that calls `setsid` leaves
    /// the worker's process group, but remains a member of its cgroup. Keep
    /// this unit test hermetic by exercising the in-memory scope model rather
    /// than moving or killing a real process in the test runner's cgroup.
    #[test]
    fn fake_cgroup_reaps_a_descendant_that_escaped_the_process_group() {
        let fake = FakeScopeOps::default();
        let dir = fake
            .create_scope("containment-test", "escapee-host")
            .expect("fake scope");
        // Synthetic IDs stand in for a leader and its setsid descendant. No
        // Command, setsid, cgroup write, or signal is involved in this test.
        fake.add_pid(&dir, 4242).unwrap();
        let reaped = fake.kill_scope(&dir).unwrap();
        assert!(
            reaped.iter().any(|proc| proc.pid == 4242),
            "teardown must report the escaped descendant: {reaped:?}"
        );
        assert!(fake.kill_scope(&dir).unwrap().is_empty());
        fake.remove_scope(&dir);
    }

    /// Teardown of a scope that is already empty (or was never created) is a
    /// no-op, not an error — every teardown path calls it unconditionally.
    #[test]
    fn killing_a_missing_scope_is_a_no_op() {
        let fake = FakeScopeOps::default();
        let missing = fake.root.path().join("never-created");
        assert_eq!(fake.kill_scope(&missing).unwrap(), Vec::new());
        fake.remove_scope(&missing);
    }
}
