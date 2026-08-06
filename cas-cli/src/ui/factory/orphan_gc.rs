//! Orphan process GC (cas-b7dd, GH #88).
//!
//! Containment (cas-99f5) stops *future* orphans; the server registry
//! (cas-7c93) tracks what agents start deliberately. This module cleans up
//! what already exists on a machine: processes still running inside a
//! worktree after whatever started them died, and registered servers whose
//! owning session is gone. Today the only remedy is `ps`/`lsof` archaeology
//! followed by a hand-typed `kill`.
//!
//! # Why this is the most dangerous code in the factory
//!
//! Every other GC in CAS marks a record, renames a directory, or abandons a
//! queue row. This one sends signals to processes it did not start. Two
//! failure modes matter, and they pull in opposite directions:
//!
//! - **Killing something alive and wanted** — a developer's editor, a shared
//!   staging server, a test runner. Unrecoverable and infuriating.
//! - **Reporting a kill that killed nothing** — the `killpg`-on-a-non-leader
//!   trap the registry documents. The operator believes the port is free, and
//!   it is not.
//!
//! So: candidacy is narrow and evidence-based, every refusal is *named* in the
//! report rather than silently filtered, and the destructive path is gated
//! behind `force=true` AND an explicit `dry_run=false` — the same double gate
//! the target-cache GC uses, because a killed process is at least as
//! unrecoverable as a deleted cache.
//!
//! # Adoption is not `ppid == 1`
//!
//! The obvious orphan test — "reparented to init" — is wrong on any host with
//! a `child_subreaper` between the process and pid 1, which is every host
//! running `systemd --user`, and many containers. Measured on the development
//! host before writing this: a deliberately orphaned `sleep` was adopted by
//! `systemd --user` (pid 3251), not by pid 1. A `ppid == 1` rule would have
//! reported "no orphans" forever while ports stayed squatted.
//!
//! [`ParentState::Reaped`] therefore means pid 1, a parent that no longer
//! exists, *or* an init-like manager (`REAPER_COMMS`) that is itself a direct
//! child of pid 1. That last clause is deliberately narrow: a user's
//! interactive shell has a terminal emulator as its parent, never pid 1, so an
//! editor or `cargo test` running inside a worktree is classified `Alive` and
//! is never a candidate.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use super::server_registry::{RegisteredServer, ServerLiveness};

/// `comm` values of init-like managers that adopt orphans. A parent matching
/// one of these *and* sitting directly under pid 1 means the original parent
/// is gone — the process was adopted, not spawned, by it.
const REAPER_COMMS: &[&str] = &[
    "systemd",
    "init",
    "launchd",
    "dumb-init",
    "tini",
    "docker-init",
    "s6-svscan",
    "catatonit",
];

/// Command fragments that mark a process as a dev server. Advisory only: it
/// enriches the report and never widens candidacy, because "looks like a dev
/// server" is a guess while "sitting in a worktree with no live owner" is
/// evidence.
const DEV_SERVER_PATTERNS: &[&str] = &[
    "node",
    "vite",
    "tsc",
    "next",
    "playwright",
    "webpack",
    "esbuild",
    "nodemon",
    "npm",
    "pnpm",
    "yarn",
    "bun",
];

/// `comm` values of build tools whose **only** possible parent is `cargo`, so
/// that an adopted one provably has no reader left (cas-4614, GH #107).
///
/// This list is load-bearing, not advisory like `DEV_SERVER_PATTERNS`: it
/// overrides the live-process-group spare. The justification is narrow and
/// must stay narrow. `rustc` and `rustdoc` are spawned by `cargo` and by
/// nothing else, and they deliver their result to that parent; once it is
/// reaped they cannot deliver to anyone.
///
/// **`cargo` itself is deliberately NOT in this list.** Its parent is a
/// launcher shell that exits *by design* in every background idiom — `cargo
/// build &`, `nohup`, `setsid`, a detached harness step — and its result is
/// artifacts in `CARGO_TARGET_DIR` plus an exit code in a log, neither
/// delivered to the parent. An adopted `cargo` is routinely a live, working
/// build. Including it would have made "adopted by systemd inside a worktree"
/// — the normal shape of a backgrounded build on a factory host — sufficient
/// to kill someone's in-flight compile. The GH #107 incident this exists for
/// was an orphaned `rustc`, so nothing is lost by excluding `cargo`.
///
/// Matched against `comm` (the kernel's 15-char executable name), not the
/// command line, so a wrapper merely named after a build tool is not caught.
/// `comm` is not authenticated — a process can set it via `prctl` or argv[0] —
/// but the only thing spoofing it buys is being selected for reaping, so
/// exact-match on `comm` is sufficient here.
const BUILD_TOOL_COMMS: &[&str] = &["rustc", "rustdoc"];

/// What the parent of a worktree process tells us about ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentState {
    /// A real parent is still running — something owns this process.
    Alive,
    /// Adopted by pid 1 or an init-like manager: the original parent died.
    Reaped,
    /// The recorded parent no longer exists at all.
    Gone,
}

impl ParentState {
    fn is_adopted(self) -> bool {
        matches!(self, ParentState::Reaped | ParentState::Gone)
    }
}

/// What GC may do with one candidate. Every non-reapable variant carries the
/// reason so the report can state it instead of dropping the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrphanDisposition {
    /// Evidence is sufficient and nothing claims it: reap under the gate.
    Reapable,
    /// Registered `shared`: deliberately outlives its session (GH #88 says
    /// cleanup must respect the flag).
    SparedShared,
    /// A live factory session or process group still owns it.
    SparedLiveOwner,
    /// No `/proc` start-time fingerprint: identity cannot be proven, so the
    /// pid may already belong to something else. Never signal.
    RefusedUnverifiable,
    /// The fingerprint proves the pid was recycled onto a different process.
    RefusedReplaced,
    /// The process is already gone; only its record needs clearing.
    RecordOnly,
}

impl OrphanDisposition {
    pub(crate) fn label(self) -> &'static str {
        match self {
            OrphanDisposition::Reapable => "reapable",
            OrphanDisposition::SparedShared => "spared (shared)",
            OrphanDisposition::SparedLiveOwner => "spared (live owner)",
            OrphanDisposition::RefusedUnverifiable => "refused (no fingerprint)",
            OrphanDisposition::RefusedReplaced => "refused (pid reused)",
            OrphanDisposition::RecordOnly => "record only (process gone)",
        }
    }

    pub(crate) fn is_reapable(self) -> bool {
        self == OrphanDisposition::Reapable
    }
}

/// Disposition for a registry entry (GH #88 classes 2 and 3).
///
/// Pure so the whole matrix is testable without starting servers. Order is
/// load-bearing: identity is checked before ownership, because a pid we cannot
/// prove must never be signalled *whatever* the session state says.
pub(crate) fn registry_disposition(
    liveness: ServerLiveness,
    shared: bool,
    session_is_live: bool,
) -> OrphanDisposition {
    match liveness {
        // Terminal, and there is nothing to signal — the entry is the leftover.
        ServerLiveness::Gone => OrphanDisposition::RecordOnly,
        ServerLiveness::Replaced => OrphanDisposition::RefusedReplaced,
        ServerLiveness::Unverifiable => OrphanDisposition::RefusedUnverifiable,
        ServerLiveness::Live if session_is_live => OrphanDisposition::SparedLiveOwner,
        ServerLiveness::Live if shared => OrphanDisposition::SparedShared,
        ServerLiveness::Live => OrphanDisposition::Reapable,
    }
}

/// Disposition for a process found inside a worktree (GH #88 class 1).
///
/// `is_build_tool` (cas-4614, GH #107) narrows one specific over-sparing case.
/// Sharing a process group with a live worker normally means the worker still
/// owns the process, so we spare it. That inference does not hold for a
/// `rustc`/`rustdoc` whose parent has been reaped: `cargo` is the only thing
/// that ever spawns one or reads its result, so once the parent is gone the
/// child is waste no matter whose process group it sits in. Observed for real
/// — a `rustc` orphaned by a killed `cargo` survived 57 minutes inside a live
/// worker's group and blocked a later build in a different
/// `CARGO_TARGET_DIR`; the report called it "spared (live owner)", which read
/// as "something owns this" when nothing did.
///
/// See `BUILD_TOOL_COMMS` for why `cargo` itself is excluded: an adopted
/// `cargo` is routinely a live backgrounded build, and killing those is worse
/// than the leak this fixes.
///
/// This never widens candidacy on its own: `parent.is_adopted()` is still
/// checked first, so a build tool belonging to a *running* cargo (parent
/// `Alive`) is not a candidate at all.
pub(crate) fn worktree_process_disposition(
    parent: ParentState,
    has_fingerprint: bool,
    owned_by_live_factory: bool,
    is_build_tool: bool,
) -> Option<OrphanDisposition> {
    if !parent.is_adopted() {
        // Something still owns it. Not a candidate at all — this is what keeps
        // a developer's shell or editor out of the report.
        return None;
    }
    if owned_by_live_factory && !is_build_tool {
        return Some(OrphanDisposition::SparedLiveOwner);
    }
    if !has_fingerprint {
        return Some(OrphanDisposition::RefusedUnverifiable);
    }
    Some(OrphanDisposition::Reapable)
}

/// True when `comm` names a build tool that cannot outlive its parent
/// usefully. See `BUILD_TOOL_COMMS`.
pub(crate) fn looks_like_build_tool(comm: &str) -> bool {
    BUILD_TOOL_COMMS.iter().any(|tool| comm == *tool)
}

/// True when `command` looks like a dev server (advisory annotation only).
pub(crate) fn looks_like_dev_server(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    DEV_SERVER_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
}

/// A process still running inside a worktree with no live owner.
#[derive(Debug, Clone)]
pub(crate) struct OrphanProcess {
    pub pid: u32,
    pub comm: String,
    pub cwd: PathBuf,
    pub ppid: u32,
    pub parent: ParentState,
    /// `/proc` start-time fingerprint, revalidated immediately before any kill.
    pub starttime: Option<u64>,
    pub ports: Vec<u16>,
    pub dev_server: bool,
    /// `comm` names a build tool (`rustc`/`rustdoc` — deliberately not
    /// `cargo`) whose parent has been reaped; see `BUILD_TOOL_COMMS`. Unlike
    /// `dev_server` this feeds the disposition, so it is recorded rather than
    /// recomputed at render.
    pub build_tool: bool,
    pub disposition: OrphanDisposition,
}

/// A registry entry whose owning session is gone (or whose process is).
#[derive(Debug, Clone)]
pub(crate) struct OrphanServer {
    pub record: RegisteredServer,
    pub liveness: ServerLiveness,
    pub ports: Vec<u16>,
    pub session_is_live: bool,
    pub disposition: OrphanDisposition,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OrphanReport {
    pub processes: Vec<OrphanProcess>,
    pub servers: Vec<OrphanServer>,
}

impl OrphanReport {
    pub(crate) fn is_empty(&self) -> bool {
        self.processes.is_empty() && self.servers.is_empty()
    }

    pub(crate) fn reapable_count(&self) -> usize {
        self.processes
            .iter()
            .filter(|p| p.disposition.is_reapable())
            .count()
            + self
                .servers
                .iter()
                .filter(|s| s.disposition.is_reapable())
                .count()
    }

    /// Ports held by reapable candidates — the operator-facing fact ("which
    /// port will be free").
    pub(crate) fn squatted_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .processes
            .iter()
            .filter(|p| p.disposition.is_reapable())
            .flat_map(|p| p.ports.iter().copied())
            .chain(
                self.servers
                    .iter()
                    .filter(|s| s.disposition.is_reapable())
                    .flat_map(|s| s.ports.iter().copied()),
            )
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    /// Report body shared by `gc_report` and the session-start banner.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        if !self.processes.is_empty() {
            out.push_str("\nOrphan processes in worktrees:\n");
            for p in &self.processes {
                let ports = if p.ports.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", ports {}",
                        p.ports
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                };
                // Name the evidence, not just the verdict: "why is this an
                // orphan" is the first question an operator asks before
                // agreeing to kill something.
                let parent = match p.parent {
                    ParentState::Reaped => format!("adopted (ppid {})", p.ppid),
                    ParentState::Gone => "parent exited".to_string(),
                    ParentState::Alive => format!("parent {} alive", p.ppid),
                };
                out.push_str(&format!(
                    "  - pid {} ({}{}{}{}) cwd {} — {}, {}\n",
                    p.pid,
                    p.comm,
                    if p.dev_server { ", dev server" } else { "" },
                    // Say why a build tool is reapable even inside a live
                    // worker's group, so the verdict does not look arbitrary.
                    if p.build_tool {
                        ", build tool with no parent to report to"
                    } else {
                        ""
                    },
                    ports,
                    p.cwd.display(),
                    parent,
                    p.disposition.label(),
                ));
            }
        }
        if !self.servers.is_empty() {
            out.push_str("\nRegistered servers from dead sessions:\n");
            for s in &self.servers {
                let ports = if s.ports.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", ports {}",
                        s.ports
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                };
                let liveness = match s.liveness {
                    ServerLiveness::Live => "process live",
                    ServerLiveness::Gone => "process gone",
                    ServerLiveness::Replaced => "pid now belongs to another process",
                    ServerLiveness::Unverifiable => "no start-time fingerprint",
                };
                out.push_str(&format!(
                    "  - {} [{}] pid {}{} (session {}{}) — {}, {}\n",
                    s.record.name,
                    s.record.id,
                    s.record.pid,
                    ports,
                    s.record.factory_session.as_deref().unwrap_or("none"),
                    if s.session_is_live {
                        ", live"
                    } else {
                        ", gone"
                    },
                    liveness,
                    s.disposition.label(),
                ));
            }
        }
        if self.reapable_count() > 0 {
            let ports = self.squatted_ports();
            if !ports.is_empty() {
                out.push_str(&format!(
                    "\nPorts held by reapable orphans: {}\n",
                    ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push_str(
                "Reclaim with gc_cleanup force=true dry_run=false (both are required; \
                 CAS revalidates each process's fingerprint immediately before signalling).\n",
            );
        }
        out
    }
}

/// Result of a cleanup pass. `killed`/`records_cleared` are zero on a dry run
/// by construction — nothing in `cleanup` acts unless `authorized`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CleanupSummary {
    pub killed: Vec<u32>,
    pub records_cleared: Vec<String>,
    pub skipped: usize,
    pub would_kill: usize,
    pub errors: Vec<String>,
}

/// Scan for orphan candidates.
///
/// `live_sessions` are factory sessions currently running; `protected_pgids`
/// are process groups of live workers (their descendants are owned, not
/// orphaned).
pub(crate) fn scan(
    cas_root: &Path,
    live_sessions: &HashSet<String>,
    protected_pgids: &HashSet<u32>,
) -> OrphanReport {
    let servers = scan_registry(cas_root, live_sessions);
    // A registry pid is never also a dead-parent candidate: registered servers
    // are *deliberately* reparented (the launcher shell exits by design), so
    // they would all look adopted. They are governed by the registry's own
    // rules — including the `shared` flag — not by this class.
    let registry_pids: HashSet<u32> = servers.iter().map(|s| s.record.pid).collect();
    let processes = scan_worktree_processes(cas_root, protected_pgids, &registry_pids);
    OrphanReport { processes, servers }
}

fn scan_registry(cas_root: &Path, live_sessions: &HashSet<String>) -> Vec<OrphanServer> {
    let records = super::server_registry::list(cas_root).unwrap_or_default();
    records
        .into_iter()
        .filter(|record| {
            // Only entries this GC could act on: a running-state record whose
            // session is gone, or any record whose process has died.
            let session_is_live = record
                .factory_session
                .as_deref()
                .is_some_and(|session| live_sessions.contains(session));
            !session_is_live
        })
        .filter_map(|record| {
            let liveness = super::server_registry::liveness(&record);
            let disposition = registry_disposition(liveness, record.shared, false);
            // A `stopped`/`dead` record with nothing to clean is just history.
            if disposition == OrphanDisposition::RecordOnly
                && record.state != super::server_registry::ServerState::Running
            {
                return None;
            }
            let ports = if liveness == ServerLiveness::Live {
                super::server_registry::listening_ports(&record)
            } else {
                Vec::new()
            };
            Some(OrphanServer {
                record,
                liveness,
                ports,
                session_is_live: false,
                disposition,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn scan_worktree_processes(
    cas_root: &Path,
    protected_pgids: &HashSet<u32>,
    registry_pids: &HashSet<u32>,
) -> Vec<OrphanProcess> {
    let worktrees = cas_root.join("worktrees");
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let self_pid = std::process::id();
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid || registry_pids.contains(&pid) {
            continue;
        }
        // `read_link` on /proc/<pid>/cwd fails for processes we do not own,
        // which is the desired filter: GC never touches another user's work.
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        if !cwd.starts_with(&worktrees) {
            continue;
        }
        let Some(stat) = read_proc_stat(pid) else {
            continue;
        };
        if stat.state == 'Z' {
            // A zombie holds no ports and cannot be killed; its parent must
            // reap it. Reporting it as an orphan would promise a kill that
            // does nothing.
            continue;
        }
        let owned_by_live_factory = protected_pgids.contains(&stat.pgid);
        let parent = parent_state(stat.ppid);
        let starttime = crate::mcp::daemon::read_pid_starttime(pid);
        let build_tool = looks_like_build_tool(&stat.comm);
        let Some(disposition) = worktree_process_disposition(
            parent,
            starttime.is_some(),
            owned_by_live_factory,
            build_tool,
        ) else {
            continue;
        };
        let command = read_proc_cmdline(pid).unwrap_or_else(|| stat.comm.clone());
        found.push(OrphanProcess {
            pid,
            comm: stat.comm,
            cwd,
            ppid: stat.ppid,
            parent,
            starttime,
            ports: super::cgroup::listening_ports_for_pid_public(pid),
            dev_server: looks_like_dev_server(&command),
            build_tool,
            disposition,
        });
    }
    found.sort_by_key(|p| p.pid);
    found
}

#[cfg(not(target_os = "linux"))]
fn scan_worktree_processes(
    _cas_root: &Path,
    _protected_pgids: &HashSet<u32>,
    _registry_pids: &HashSet<u32>,
) -> Vec<OrphanProcess> {
    // No portable `/proc` equivalent; the registry class still works.
    Vec::new()
}

/// Classify a parent pid. See the module header on why `ppid == 1` alone is
/// not enough.
#[cfg(target_os = "linux")]
pub(crate) fn parent_state(ppid: u32) -> ParentState {
    if ppid <= 1 {
        return ParentState::Reaped;
    }
    let Some(stat) = read_proc_stat(ppid) else {
        return ParentState::Gone;
    };
    let adopted_by_manager = stat.ppid == 1 && REAPER_COMMS.iter().any(|comm| stat.comm == *comm);
    if adopted_by_manager {
        ParentState::Reaped
    } else {
        ParentState::Alive
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn parent_state(ppid: u32) -> ParentState {
    if ppid <= 1 {
        ParentState::Reaped
    } else {
        ParentState::Alive
    }
}

#[derive(Debug, Clone)]
struct ProcStat {
    comm: String,
    state: char,
    ppid: u32,
    pgid: u32,
}

#[cfg(target_os = "linux")]
fn read_proc_stat(pid: u32) -> Option<ProcStat> {
    parse_proc_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// `comm` is parenthesized and may itself contain spaces and parens, so every
/// field after it is read from the *last* `)` — the same rule
/// `process_groups.rs` and `server_registry.rs` use.
fn parse_proc_stat(stat: &str) -> Option<ProcStat> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_string();
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    Some(ProcStat {
        comm,
        state,
        ppid,
        pgid,
    })
}

#[cfg(target_os = "linux")]
fn read_proc_cmdline(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.trim().is_empty()).then_some(text)
}

/// Act on a scanned report.
///
/// `authorized` is the caller's double gate (`force=true` AND an explicit
/// `dry_run=false`). When false this is a pure preview: it counts what it
/// *would* do and touches nothing.
///
/// Every kill revalidates the fingerprint immediately before signalling, so a
/// pid recycled between scan and execution is skipped rather than killed —
/// the same revalidate-at-execution contract the worktree and target-cache
/// GCs follow.
pub(crate) fn cleanup(cas_root: &Path, report: &OrphanReport, authorized: bool) -> CleanupSummary {
    let mut summary = CleanupSummary::default();

    for process in &report.processes {
        if !process.disposition.is_reapable() {
            summary.skipped += 1;
            continue;
        }
        if !authorized {
            summary.would_kill += 1;
            continue;
        }
        match kill_pid_fingerprinted(process.pid, process.starttime) {
            Ok(true) => summary.killed.push(process.pid),
            Ok(false) => summary.skipped += 1,
            Err(error) => summary.errors.push(format!("pid {}: {error}", process.pid)),
        }
    }

    for server in &report.servers {
        match server.disposition {
            OrphanDisposition::RecordOnly => {
                if !authorized {
                    summary.would_kill += 1;
                    continue;
                }
                match super::server_registry::forget(cas_root, &server.record.id) {
                    Ok(()) => summary.records_cleared.push(server.record.id.clone()),
                    Err(error) => summary
                        .errors
                        .push(format!("registry {}: {error}", server.record.id)),
                }
            }
            OrphanDisposition::Reapable => {
                if !authorized {
                    summary.would_kill += 1;
                    continue;
                }
                // Route through the registry's own stop path: it re-checks the
                // fingerprint and signals the recorded pgid where required.
                // `killpg` on a non-leader names a group that does not exist
                // and silently kills nothing — the registry documents this,
                // and a "cleanup" that reports success while freeing no port
                // is worse than no cleanup at all.
                match super::server_registry::stop(cas_root, &server.record) {
                    Ok(outcome) => {
                        summary.killed.push(server.record.pid);
                        tracing::info!(
                            server = %server.record.name,
                            id = %server.record.id,
                            pid = server.record.pid,
                            outcome = ?outcome,
                            "cas-b7dd: reaped orphan server from a dead session"
                        );
                    }
                    Err(error) => summary
                        .errors
                        .push(format!("server {}: {error}", server.record.id)),
                }
            }
            _ => summary.skipped += 1,
        }
    }

    summary
}

/// SIGKILL a pid only if its `/proc` start time still matches the fingerprint
/// captured at scan time. Returns `Ok(false)` when the pid was recycled or has
/// already exited.
fn kill_pid_fingerprinted(pid: u32, expected_starttime: Option<u64>) -> io::Result<bool> {
    let Some(expected) = expected_starttime else {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to signal a process with no start-time fingerprint",
        ));
    };
    match crate::mcp::daemon::read_pid_starttime(pid) {
        Some(actual) if actual == expected => {}
        // Recycled onto another process, or already gone. Either way, do not
        // signal: this is the check that stops GC killing a bystander.
        _ => return Ok(false),
    }

    #[cfg(unix)]
    {
        // SAFETY: read-only signal to a pid whose identity was just
        // revalidated against its durable start-time fingerprint.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        if rc != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn server_record(pid: u32, shared: bool, session: Option<&str>) -> RegisteredServer {
        RegisteredServer {
            id: format!("srv-{pid}"),
            name: "dev".to_string(),
            command: "npm run dev".to_string(),
            cwd: PathBuf::from("/tmp"),
            pid,
            pgid: Some(pid),
            pid_starttime: Some(42),
            expected_port: Some(5173),
            owner_task: None,
            owner_worker: None,
            factory_session: session.map(str::to_string),
            shared,
            cgroup: None,
            log_path: None,
            started_at: Utc::now(),
            state: super::super::server_registry::ServerState::Running,
            ended_at: None,
            ended_detail: None,
        }
    }

    /// Plant a genuine orphan: a shell that backgrounds a child and exits, so
    /// the child is adopted by init or the host's subreaper. Returns its pid.
    ///
    /// This is the real thing, not a simulation — the whole point of the AC.
    ///
    /// The background child's stdio is redirected to `/dev/null` deliberately.
    /// Without it the child inherits the shell's stdout *pipe*, and
    /// `Command::output()` blocks until every writer closes it — i.e. until
    /// the `sleep` finishes. The first draft of this helper took 300 seconds
    /// per test and then asserted against an orphan that had already exited.
    ///
    /// Linux-only, like the two tests that call it: it polls `read_proc_stat`,
    /// which reads `/proc` and does not exist on other targets (GH #93).
    #[cfg(target_os = "linux")]
    fn plant_orphan(cwd: &Path) -> u32 {
        let output = Command::new("sh")
            .arg("-c")
            .arg("sleep 120 >/dev/null 2>&1 </dev/null & echo $!")
            .current_dir(cwd)
            .output()
            .expect("spawn orphan");
        let pid: u32 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("orphan pid");
        // Wait for the launcher shell to exit and the child to be adopted.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(
                parent_state(read_proc_stat(pid).map(|s| s.ppid).unwrap_or(1)),
                ParentState::Reaped | ParentState::Gone
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        pid
    }

    #[cfg(target_os = "linux")] // only the /proc-based tests use it
    fn kill_if_alive(pid: u32) {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }

    /// Kills a planted process on drop, so a failed assertion cannot leak it.
    ///
    /// Without this the planted process outlives the test for its full sleep.
    /// That matters most for the `rustc`-named decoy: production now reaps
    /// processes with exactly that `comm`, and a concurrent run of this same
    /// suite would find a stranger's leaked decoy sitting in a worktree.
    /// Cleaning up on the failure path is what keeps the test from seeding the
    /// condition it exists to detect.
    #[cfg(target_os = "linux")]
    struct PlantedProcess(u32);

    #[cfg(target_os = "linux")]
    impl Drop for PlantedProcess {
        fn drop(&mut self) {
            // The decoy is a shell with a `sleep` child; killing only the
            // parent leaves that child behind as a second orphan in a
            // worktree the test is about to delete.
            //
            // Reap the children by reading `/proc/<pid>/task/<pid>/children`,
            // NOT with `killpg`. A background job in a non-interactive `sh -c`
            // does not become a process-group leader — it inherits the
            // shell's group, which here is the *test runner's* group. A
            // `killpg` on that pgid would SIGKILL cargo itself.
            let children = std::fs::read_to_string(format!(
                "/proc/{pid}/task/{pid}/children",
                pid = self.0
            ))
            .unwrap_or_default();
            for child in children.split_whitespace().filter_map(|c| c.parse().ok()) {
                kill_if_alive(child);
            }
            kill_if_alive(self.0);
        }
    }

    // ---------------------------------------------------------------
    // Pure disposition matrix — no processes required.
    // ---------------------------------------------------------------

    #[test]
    fn registry_shared_entries_are_spared_even_when_their_session_is_gone() {
        assert_eq!(
            registry_disposition(ServerLiveness::Live, true, false),
            OrphanDisposition::SparedShared,
            "a shared server outliving its session is the documented reason the flag exists"
        );
        assert_eq!(
            registry_disposition(ServerLiveness::Live, false, false),
            OrphanDisposition::Reapable,
            "a private server whose session died is the port squatter GH #88 is about"
        );
    }

    #[test]
    fn registry_live_session_is_never_touched() {
        for shared in [true, false] {
            assert_eq!(
                registry_disposition(ServerLiveness::Live, shared, true),
                OrphanDisposition::SparedLiveOwner,
            );
        }
    }

    #[test]
    fn registry_identity_failures_refuse_before_ownership_is_considered() {
        // Ordering matters: an unprovable pid must never be signalled, even
        // when every ownership signal says "orphan".
        assert_eq!(
            registry_disposition(ServerLiveness::Replaced, false, false),
            OrphanDisposition::RefusedReplaced,
        );
        assert_eq!(
            registry_disposition(ServerLiveness::Unverifiable, false, false),
            OrphanDisposition::RefusedUnverifiable,
        );
        assert_eq!(
            registry_disposition(ServerLiveness::Gone, false, false),
            OrphanDisposition::RecordOnly,
        );
    }

    #[test]
    fn a_process_with_a_live_parent_is_not_a_candidate() {
        // The developer's editor / test runner case: cwd is inside a worktree
        // but a real parent owns it.
        assert_eq!(
            worktree_process_disposition(ParentState::Alive, true, false, false),
            None
        );
    }

    #[test]
    fn adopted_process_matrix() {
        for parent in [ParentState::Reaped, ParentState::Gone] {
            assert_eq!(
                worktree_process_disposition(parent, true, false, false),
                Some(OrphanDisposition::Reapable)
            );
            assert_eq!(
                worktree_process_disposition(parent, true, true, false),
                Some(OrphanDisposition::SparedLiveOwner),
                "a live worker's own process group still owns its descendants"
            );
            assert_eq!(
                worktree_process_disposition(parent, false, false, false),
                Some(OrphanDisposition::RefusedUnverifiable),
                "no fingerprint means the pid cannot be proven — never signal"
            );
        }
    }

    // cas-4614 (GH #107): a build tool whose parent has been reaped is waste
    // even inside a live worker's process group.
    #[test]
    fn orphaned_build_tool_is_not_spared_by_a_live_process_group() {
        for parent in [ParentState::Reaped, ParentState::Gone] {
            assert_eq!(
                worktree_process_disposition(parent, true, true, true),
                Some(OrphanDisposition::Reapable),
                "cargo is the only reader of a rustc's result; once the parent \
                 is reaped the child cannot deliver to anyone, whatever process \
                 group it sits in"
            );
        }
    }

    #[test]
    fn a_running_build_is_never_a_candidate_even_though_it_is_a_build_tool() {
        // The regression that would matter most: killing rustc out from under
        // a live cargo. `parent.is_adopted()` is checked first, so a build
        // tool with a live parent is not a candidate at all.
        assert_eq!(
            worktree_process_disposition(ParentState::Alive, true, true, true),
            None
        );
        assert_eq!(
            worktree_process_disposition(ParentState::Alive, true, false, true),
            None
        );
    }

    #[test]
    fn build_tool_flag_does_not_bypass_the_fingerprint_refusal() {
        // Identity still outranks everything: an unverifiable pid may already
        // be a different process, so being a build tool must not license a kill.
        assert_eq!(
            worktree_process_disposition(ParentState::Reaped, false, true, true),
            Some(OrphanDisposition::RefusedUnverifiable)
        );
    }

    #[test]
    fn build_tool_detection_matches_comm_exactly() {
        for comm in ["rustc", "rustdoc"] {
            assert!(looks_like_build_tool(comm), "{comm} should be a build tool");
        }
        for comm in [
            "node",
            "bash",
            "cas",
            // Substring matches must NOT count — `comm` is the executable
            // name, and a wrapper merely named after a build tool is not one.
            "cargo-nextest",
            "rustc-wrapper",
            "sccache",
            "",
        ] {
            assert!(
                !looks_like_build_tool(comm),
                "{comm:?} should not be treated as a build tool"
            );
        }
    }

    /// Regression guard for the review finding that killed the first draft:
    /// `cargo` must NOT get the live-process-group override.
    ///
    /// An adopted `cargo` is the normal shape of a backgrounded build —
    /// `cargo build &`, `nohup`, `setsid`, a detached harness step — whose
    /// launcher shell exits by design while the build runs on. A live example
    /// was found on the dev host during review: a working `cargo test`
    /// adopted by `systemd --user`, cwd inside a worktree, which the first
    /// draft classified `Reapable`.
    #[test]
    fn cargo_is_not_a_build_tool_because_adopted_cargo_is_usually_working() {
        assert!(
            !looks_like_build_tool("cargo"),
            "an adopted cargo is routinely a live backgrounded build; giving it \
             the live-owner override would kill in-flight compiles"
        );
        assert_eq!(
            worktree_process_disposition(ParentState::Reaped, true, true, false),
            Some(OrphanDisposition::SparedLiveOwner),
            "a live worker's process group must still protect an adopted cargo"
        );
    }

    #[test]
    fn dev_server_annotation_matches_the_documented_patterns() {
        for command in [
            "node server.js",
            "/usr/bin/vite --port 5173",
            "npx tsc --watch",
            "next dev",
            "playwright test --ui",
        ] {
            assert!(looks_like_dev_server(command), "{command} should match");
        }
        assert!(!looks_like_dev_server("/usr/bin/sleep 300"));
    }

    #[test]
    fn proc_stat_is_parsed_from_the_last_paren() {
        let stat = "42 (my (weird) server) S 1 42 42 0 -1 4194304 0 0";
        let parsed = parse_proc_stat(stat).expect("parse");
        assert_eq!(parsed.comm, "my (weird) server");
        assert_eq!(parsed.state, 'S');
        assert_eq!(parsed.ppid, 1);
        assert_eq!(parsed.pgid, 42);
    }

    // ---------------------------------------------------------------
    // Planted orphans — the AC's end-to-end proof.
    // ---------------------------------------------------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn planted_orphan_is_reported_and_reaped_only_when_authorized() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worktree = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&worktree).unwrap();

        let pid = plant_orphan(&worktree);
        let live_sessions = HashSet::new();
        let protected = HashSet::new();

        let report = scan(&cas_root, &live_sessions, &protected);
        let found = report.processes.iter().find(|p| p.pid == pid);
        assert!(
            found.is_some(),
            "planted orphan {pid} must appear in the report; got {:?}",
            report.processes
        );
        let found = found.unwrap();
        assert_eq!(found.disposition, OrphanDisposition::Reapable);
        assert!(found.cwd.starts_with(&worktree));
        assert!(report.render().contains(&pid.to_string()));

        // Dry run (the default posture) must not kill anything.
        let preview = cleanup(&cas_root, &report, false);
        assert_eq!(preview.killed, Vec::<u32>::new());
        assert_eq!(preview.would_kill, 1);
        assert!(
            crate::mcp::daemon::pid_alive(pid),
            "a preview must never kill: pid {pid} should still be alive"
        );

        // Authorized run reaps it.
        let done = cleanup(&cas_root, &report, true);
        assert_eq!(done.killed, vec![pid], "errors: {:?}", done.errors);

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && crate::mcp::daemon::pid_alive(pid) {
            std::thread::sleep(Duration::from_millis(25));
        }
        // A killed child of init is reaped by init, so the pid disappears.
        assert!(
            !crate::mcp::daemon::pid_alive(pid)
                || read_proc_stat(pid).map(|s| s.state) == Some('Z'),
            "orphan {pid} should be dead after an authorized cleanup"
        );
        kill_if_alive(pid);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_workers_own_process_group_is_never_reaped() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worktree = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&worktree).unwrap();

        let pid = plant_orphan(&worktree);
        // Claim the orphan's process group as a live worker's.
        let pgid = read_proc_stat(pid).expect("stat").pgid;
        let protected: HashSet<u32> = [pgid].into_iter().collect();

        let report = scan(&cas_root, &HashSet::new(), &protected);
        let found = report
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .expect("still reported, but spared");
        assert_eq!(found.disposition, OrphanDisposition::SparedLiveOwner);

        let done = cleanup(&cas_root, &report, true);
        assert!(done.killed.is_empty(), "a live-owned process must survive");
        assert!(crate::mcp::daemon::pid_alive(pid));
        kill_if_alive(pid);
    }

    /// cas-4614 (GH #107) end-to-end, and the counterpart to
    /// `a_live_workers_own_process_group_is_never_reaped`: the same setup —
    /// an adopted process claimed by a live worker's process group — flips
    /// from spared to reapable purely because the process is named `rustc`.
    ///
    /// This is the real incident: a `cargo` was killed, its `rustc` children
    /// were adopted by init but stayed in the live worker's group, the report
    /// said "spared (live owner)", and one of them went on to block a later
    /// build in a different `CARGO_TARGET_DIR` for 57 minutes.
    #[cfg(target_os = "linux")]
    #[test]
    fn orphaned_rustc_in_a_live_workers_group_is_reaped() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worktree = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&worktree).unwrap();

        // `comm` is the basename of the running executable, so a copy of the
        // shell named `rustc` is indistinguishable from the real thing to the
        // scanner — which is exactly what we want to exercise.
        //
        // It must be the shell and not `sleep`: coreutils ships as a
        // multi-call binary that dispatches on argv[0] and exits with
        // "unknown program 'rustc'" when renamed. The shell keeps running
        // under any name.
        //
        // The `; true` is load-bearing. Given a single simple command, bash
        // execs it in place rather than forking, so the process would replace
        // itself with `sleep` and `comm` would read "sleep" — the first
        // version of this test failed exactly that way. A trailing statement
        // forces bash to stay resident as the parent.
        //
        // The pid comes from `$!` — the process this test started. Never
        // select build tools by name on a shared host: `pgrep -x rustc` will
        // cheerfully match another worker's live compile.
        let fake_rustc = temp.path().join("rustc");
        std::fs::copy("/bin/bash", &fake_rustc).expect("copy bash -> rustc");
        let pid = {
            let output = Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "{} -c 'sleep 120; true' >/dev/null 2>&1 </dev/null & echo $!",
                    fake_rustc.display()
                ))
                .current_dir(&worktree)
                .output()
                .expect("spawn fake rustc orphan");
            let pid: u32 = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .expect("orphan pid");
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if matches!(
                    parent_state(read_proc_stat(pid).map(|s| s.ppid).unwrap_or(1)),
                    ParentState::Reaped | ParentState::Gone
                ) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            pid
        };

        // Guard before the first assertion: everything below can panic.
        let _guard = PlantedProcess(pid);

        let stat = read_proc_stat(pid).expect("stat");
        assert_eq!(stat.comm, "rustc", "the planted orphan must look like rustc");
        // Claim its process group as a live worker's — the exact condition
        // that used to spare it.
        let protected: HashSet<u32> = [stat.pgid].into_iter().collect();

        let report = scan(&cas_root, &HashSet::new(), &protected);
        let found = report
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .expect("planted rustc orphan must be reported");
        assert!(
            found.build_tool,
            "comm rustc must set the build_tool flag (scan saw comm={:?})",
            found.comm
        );
        assert_eq!(
            found.disposition,
            OrphanDisposition::Reapable,
            "an orphaned build tool must not be spared by a live process group"
        );
        assert!(
            report.render().contains("build tool with no parent to report to"),
            "the report must say why it is reapable despite the live owner"
        );

        let done = cleanup(&cas_root, &report, true);
        assert!(
            done.killed.contains(&pid),
            "the orphaned rustc should actually be reaped"
        );
        // `_guard` reaps it on the failure path too.
    }

    #[test]
    fn stale_registry_entry_for_a_dead_session_is_reported_and_its_record_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        // A record whose process is genuinely gone. NOT pid 0: `kill(0, 0)`
        // signals the caller's own process group and reports success, so pid 0
        // reads as *alive* — it would classify Unverifiable, not Gone. Use a
        // pid we watched exit instead.
        let dead_pid = {
            let mut child = Command::new("true").spawn().expect("spawn");
            child.wait().expect("reap");
            child.id()
        };
        let mut record = server_record(dead_pid, false, Some("dead-session"));
        record.pid_starttime = None;
        super::super::server_registry::write_record(&cas_root, &record).unwrap();

        let report = scan(&cas_root, &HashSet::new(), &HashSet::new());
        let found = report
            .servers
            .iter()
            .find(|s| s.record.id == record.id)
            .expect("stale registry entry must be reported");
        assert_eq!(found.disposition, OrphanDisposition::RecordOnly);
        assert!(report.render().contains("dead-session"));

        let preview = cleanup(&cas_root, &report, false);
        assert!(preview.records_cleared.is_empty());
        assert!(
            super::super::server_registry::find(&cas_root, &record.id)
                .unwrap()
                .is_some(),
            "a preview must leave the record in place"
        );

        let done = cleanup(&cas_root, &report, true);
        assert_eq!(done.records_cleared, vec![record.id.clone()]);
        assert!(
            super::super::server_registry::find(&cas_root, &record.id)
                .unwrap()
                .is_none(),
            "an authorized cleanup clears the stale record"
        );
    }

    #[test]
    fn shared_registry_entry_from_a_dead_session_survives_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        // Point the record at a process that really is alive (this test
        // process), so liveness resolves Live and only the shared flag can
        // spare it.
        let self_pid = std::process::id();
        let mut record = server_record(self_pid, true, Some("dead-session"));
        record.pid_starttime = crate::mcp::daemon::read_pid_starttime(self_pid);
        super::super::server_registry::write_record(&cas_root, &record).unwrap();

        let report = scan(&cas_root, &HashSet::new(), &HashSet::new());
        let found = report
            .servers
            .iter()
            .find(|s| s.record.id == record.id)
            .expect("shared entry is still reported");
        assert_eq!(found.disposition, OrphanDisposition::SparedShared);

        let done = cleanup(&cas_root, &report, true);
        assert!(
            done.killed.is_empty(),
            "a shared server must never be reaped by GC"
        );
        assert!(
            super::super::server_registry::find(&cas_root, &record.id)
                .unwrap()
                .is_some(),
            "and its record must stay"
        );
    }

    #[test]
    fn a_running_session_keeps_its_servers_out_of_the_report() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let record = server_record(std::process::id(), false, Some("live-session"));
        super::super::server_registry::write_record(&cas_root, &record).unwrap();

        let live: HashSet<String> = ["live-session".to_string()].into_iter().collect();
        let report = scan(&cas_root, &live, &HashSet::new());
        assert!(
            report.servers.is_empty(),
            "a live session's servers are not GC candidates at all"
        );
    }
}
