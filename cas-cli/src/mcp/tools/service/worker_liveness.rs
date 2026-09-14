//! Bounded, local execution evidence. Registry heartbeat is deliberately absent.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use cas_mux::SupervisorCli;
use chrono::{DateTime, Utc};
use serde_json::Value;

const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    Executing,
    WaitingForInput,
    Stalled,
    Dead,
}
impl Liveness {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Executing => "executing",
            Self::WaitingForInput => "waiting_for_input",
            Self::Stalled => "stalled",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub state: Liveness,
    pub evidence: String,
}
impl Observation {
    pub(crate) fn summary(&self, name: &str) -> String {
        format!("liveness: {} | {name}", self.state.as_str())
    }
    pub(crate) fn detail(&self) -> String {
        format!("liveness: {} ({})", self.state.as_str(), self.evidence)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessEvidence {
    // None means unavailable, not exited.
    pub alive: Option<bool>,
    pub cpu_busy: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone)]
struct TurnEvent {
    at: DateTime<Utc>,
    state: Liveness,
    kind: String,
}

/// Reads a fixed-size snapshot; ignores a partial first/last record, invalid
/// UTF-8 records and malformed JSON. A concurrent append is seen next poll.
pub(crate) fn tail_records(path: &Path, mut visit: impl FnMut(&Value)) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let Ok(meta) = file.metadata() else { return };
    let start = meta.len().saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    if file
        .take(meta.len() - start)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return;
    }
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    if start > 0 {
        lines.next();
    }
    for line in lines {
        if line.last() != Some(&b'\n') {
            continue;
        }
        if let Ok(value) = serde_json::from_slice(line) {
            visit(&value);
        }
    }
}

fn event(value: &Value, cli: SupervisorCli) -> Option<TurnEvent> {
    let at = value
        .get("timestamp")
        .or_else(|| value.get("ts"))?
        .as_str()?;
    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
    let kind = value.get("type")?.as_str()?;
    let (state, kind) = match cli {
        SupervisorCli::Codex => {
            if let Some(end) = super::harness_observation::codex_turn_end_kind(value) {
                (
                    if end == "error"
                        || value
                            .pointer("/payload/error")
                            .is_some_and(|e| !e.is_null())
                    {
                        Liveness::Stalled
                    } else {
                        Liveness::WaitingForInput
                    },
                    end,
                )
            } else if kind == "event_msg"
                && matches!(
                    value.pointer("/payload/type")?.as_str()?,
                    "turn_started" | "task_started"
                )
            {
                (
                    Liveness::Executing,
                    value.pointer("/payload/type")?.as_str()?,
                )
            } else if kind == "response_item" {
                let payload = value.get("payload")?;
                match payload.get("type")?.as_str()? {
                    "function_call"
                    | "function_call_output"
                    | "custom_tool_call"
                    | "custom_tool_call_output" => (Liveness::Executing, "tool activity"),
                    "message" if payload.get("role")?.as_str()? == "assistant" => {
                        (Liveness::Executing, "assistant")
                    }
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        SupervisorCli::Claude => {
            if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            if kind == "system"
                && value.get("subtype").and_then(Value::as_str) == Some("turn_duration")
            {
                (Liveness::WaitingForInput, "turn_duration")
            } else if kind == "assistant" {
                match value
                    .pointer("/message/stop_reason")
                    .and_then(Value::as_str)
                {
                    Some("end_turn" | "stop_sequence") => (Liveness::WaitingForInput, "end_turn"),
                    Some("tool_use") => (Liveness::Executing, "tool_use"),
                    _ => (Liveness::Executing, "assistant"),
                }
            } else if kind == "user"
                && value.pointer("/message/role").and_then(Value::as_str) == Some("user")
            {
                let textual = value.pointer("/message/content").is_some_and(|content| {
                    content.as_str().is_some_and(|s| !s.trim().is_empty())
                        || content.as_array().is_some_and(|parts| {
                            parts
                                .iter()
                                .any(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                        })
                });
                (
                    Liveness::Executing,
                    if textual { "user" } else { "tool_result" },
                )
            } else {
                return None;
            }
        }
        SupervisorCli::Grok => match kind {
            "turn_started" | "task_started" | "assistant_message" | "agent_message" => {
                (Liveness::Executing, kind)
            }
            "turn_ended" => (Liveness::WaitingForInput, kind),
            _ => return None,
        },
        SupervisorCli::OpenCode => return None,
    };
    Some(TurnEvent {
        at,
        state,
        kind: kind.into(),
    })
}

pub(crate) fn observe(
    cli: SupervisorCli,
    path: Option<&Path>,
    process: ProcessEvidence,
    now: DateTime<Utc>,
    stall_secs: i64,
) -> Observation {
    let mut last: Option<TurnEvent> = None;
    if let Some(path) = path {
        tail_records(path, |value| {
            if let Some(event) = event(value, cli) {
                // Late tool/assistant flushes after a terminal event do not
                // start another turn. Only an actual start can reopen it.
                if last
                    .as_ref()
                    .is_some_and(|old| old.state != Liveness::Executing)
                    && event.state == Liveness::Executing
                    && !matches!(
                        event.kind.as_str(),
                        "turn_started" | "task_started" | "user"
                    )
                {
                    return;
                }
                last = Some(event);
            }
        });
        if cli == SupervisorCli::Grok {
            tail_records(&path.with_file_name("events.jsonl"), |value| {
                if let Some(event) = event(value, cli) {
                    if last.as_ref().is_none_or(|old| event.at >= old.at) {
                        last = Some(event);
                    }
                }
            });
        }
    }
    let write_age = path
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok())
        .map(|at| (now - DateTime::<Utc>::from(at)).num_seconds().max(0));
    let state = if process.alive == Some(false) {
        Liveness::Dead
    } else if let Some(event) = &last {
        if event.state == Liveness::Executing
            && (now - event.at).num_seconds() > stall_secs
            && process.cpu_busy != Some(true)
        {
            Liveness::Stalled
        } else {
            event.state
        }
    } else if process.cpu_busy == Some(true) {
        Liveness::Executing
    } else {
        Liveness::Stalled
    };
    let event_detail = last
        .map(|e| format!("{} {}s ago", e.kind, (now - e.at).num_seconds().max(0)))
        .unwrap_or_else(|| "turn evidence unavailable in bounded tail".into());
    Observation {
        state,
        evidence: format!(
            "{event_detail}; {}; last write {}",
            process.detail,
            write_age
                .map(|s| format!("{s}s ago"))
                .unwrap_or_else(|| "unavailable".into())
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessCandidate {
    pid: u32,
    alive: bool,
    harness: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessSelection {
    Found(u32),
    Exited(&'static str),
    Unavailable(&'static str),
}

struct ProcessProbes<'a> {
    pid_alive: &'a dyn Fn(u32) -> bool,
    pid_matches_fingerprint: &'a dyn Fn(u32, u64) -> bool,
    is_harness: &'a dyn Fn(u32) -> bool,
    parent_pid: &'a dyn Fn(u32) -> Option<u32>,
}

/// Select a harness process without treating a registered MCP-server PID as
/// the harness. An MCP registration records its own PID in `Agent.pid` and
/// the interactive parent's PID in `Agent.ppid`; only the latter can prove
/// that the Codex/Claude process exited. A live-but-unidentified PID is
/// deliberately unavailable rather than dead because it may be a recycled
/// PID or an MCP child.
fn select_harness_process(
    registered: Option<ProcessCandidate>,
    parent: Option<ProcessCandidate>,
    scanned: Option<ProcessCandidate>,
) -> ProcessSelection {
    if let Some(candidate) = parent.filter(|candidate| candidate.alive && candidate.harness) {
        return ProcessSelection::Found(candidate.pid);
    }
    if let Some(candidate) = registered.filter(|candidate| candidate.alive && candidate.harness) {
        return ProcessSelection::Found(candidate.pid);
    }
    if let Some(candidate) = scanned.filter(|candidate| candidate.alive && candidate.harness) {
        return ProcessSelection::Found(candidate.pid);
    }

    if let Some(parent) = parent {
        if !parent.alive {
            return ProcessSelection::Exited("worker harness parent exited");
        }
        return ProcessSelection::Unavailable("worker harness parent is not an identified harness");
    }
    if let Some(registered) = registered {
        if !registered.alive {
            return ProcessSelection::Exited("registered worker harness exited");
        }
        return ProcessSelection::Unavailable(
            "worker harness unavailable; registered process is not an identified harness",
        );
    }
    ProcessSelection::Unavailable("worker harness pid unavailable")
}

fn process_selection_for_agent(
    agent: &cas_types::Agent,
    scanned: Option<ProcessCandidate>,
    probes: &ProcessProbes<'_>,
) -> ProcessSelection {
    let registered = if agent.ppid.is_none() {
        agent.pid.map(|pid| {
            let fingerprint_matches = super::agent_liveness::agent_process_is_alive_with(
                agent,
                probes.pid_alive,
                probes.pid_matches_fingerprint,
            );
            ProcessCandidate {
                // Keep raw existence separate from the PID-start fingerprint:
                // a recycled, still-live PID is unavailable identity evidence,
                // not proof that the registered harness exited.
                alive: (probes.pid_alive)(pid),
                harness: fingerprint_matches && (probes.is_harness)(pid),
                pid,
            }
        })
    } else {
        None
    };
    let parent = agent.ppid.map(|pid| {
        // `pid` is the MCP child for server registrations. Accept its parent
        // as the worker harness only when the child still has the recorded
        // parent and its own start-time fingerprint is valid. This blocks a
        // recycled parent PID that happens to run another Codex/Claude.
        let child_owned = agent.pid.is_some_and(|child| {
            (probes.pid_alive)(child)
                && super::agent_liveness::agent_process_is_alive_with(
                    agent,
                    probes.pid_alive,
                    probes.pid_matches_fingerprint,
                )
                && (probes.parent_pid)(child) == Some(pid)
        });
        ProcessCandidate {
            alive: (probes.pid_alive)(pid),
            harness: child_owned && (probes.is_harness)(pid),
            pid,
        }
    });
    select_harness_process(registered, parent, scanned)
}

fn process_parent_pid(pid: u32) -> Option<u32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, tail) = raw.rsplit_once(')')?;
    tail.split_whitespace().nth(1)?.parse().ok()
}

/// Nonblocking process sample. CPU is a delta across polls, never a sleep.
/// A stdin reader exists even mid-turn in threaded harnesses: display it as
/// corroboration only; it must never override a turn-start or terminal event.
pub(crate) fn process_evidence(agent: &cas_types::Agent) -> ProcessEvidence {
    use crate::cli::factory::wedged;
    let is_harness = |pid| {
        if !cfg!(target_os = "linux") {
            return true;
        }
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .is_some_and(|raw| is_harness_command(&raw))
    };

    let scanned = wedged::find_worker_pid(&wedged::RealProcessTable, &agent.name).map(|pid| {
        ProcessCandidate {
            // `find_worker_pid` only returns a currently visible process.
            alive: true,
            harness: is_harness(pid),
            pid,
        }
    });
    let pid_alive = crate::mcp::daemon::pid_alive;
    let pid_matches_fingerprint = crate::mcp::daemon::pid_matches_fingerprint;
    let parent_pid = process_parent_pid;
    let probes = ProcessProbes {
        pid_alive: &pid_alive,
        pid_matches_fingerprint: &pid_matches_fingerprint,
        is_harness: &is_harness,
        parent_pid: &parent_pid,
    };
    let selection = process_selection_for_agent(agent, scanned, &probes);
    let pid = match selection {
        ProcessSelection::Found(pid) => pid,
        ProcessSelection::Exited(detail) => {
            return ProcessEvidence {
                alive: Some(false),
                cpu_busy: None,
                detail: detail.into(),
            };
        }
        ProcessSelection::Unavailable(detail) => {
            return ProcessEvidence {
                // Missing identity evidence is not evidence of exit. Turn
                // records may still establish execution below in `observe`.
                alive: None,
                cpu_busy: None,
                detail: detail.into(),
            };
        }
    };
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let fields: Vec<_> = raw
        .rsplit_once(')')
        .map(|(_, tail)| tail.split_whitespace().collect())
        .unwrap_or_default();
    let state = fields.first().copied().unwrap_or("unknown");
    let fingerprint = fields.get(19).copied().unwrap_or("").to_string();
    let ticks = wedged::process_cpu_ticks(pid);
    static SAMPLES: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<u32, (String, u64)>>,
    > = std::sync::LazyLock::new(Default::default);
    let busy = ticks.and_then(|ticks| {
        let mut samples = SAMPLES.lock().ok()?;
        if samples.len() > 1024 {
            samples.clear();
        }
        let prior = samples.insert(pid, (fingerprint.clone(), ticks));
        prior
            .filter(|(old, _)| old == &fingerprint)
            .map(|(_, old)| ticks > old)
    });
    let wchan = std::fs::read_to_string(format!("/proc/{pid}/wchan"))
        .unwrap_or_else(|_| "unavailable".into());
    let syscall = std::fs::read_to_string(format!("/proc/{pid}/syscall")).unwrap_or_default();
    let syscall: Vec<_> = syscall.split_whitespace().collect();
    // Linux x86_64 read(0, ...); other architectures leave this unobserved.
    let stdin = cfg!(all(target_os = "linux", target_arch = "x86_64"))
        && syscall.first() == Some(&"0")
        && syscall.get(1) == Some(&"0x0");
    ProcessEvidence {
        alive: Some(!matches!(state, "Z" | "X")),
        cpu_busy: busy,
        detail: format!(
            "pid {pid} {state}, cpu {}, wait {}, stdin_read {}",
            busy.map(|b| if b { "active" } else { "idle" })
                .unwrap_or("unsampled"),
            wchan.trim(),
            if stdin { "yes" } else { "unobserved" }
        ),
    }
}

fn is_harness_command(raw: &[u8]) -> bool {
    let command = String::from_utf8_lossy(raw);
    command.split('\0').take(2).any(|arg| {
        let name = Path::new(arg)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        // codex-code-mode-host inherits the worker identity but is a tool
        // sidecar, so accepting arbitrary codex-* names recreates false life.
        matches!(name, "codex" | "claude" | "grok" | "opencode")
    })
}

#[cfg(test)]
mod tests;
