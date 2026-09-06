//! Factory worker liveness triage and recovery verbs (cas-4513).
//!
//! When a Claude Code worker's React-Ink UI throws an unhandled rejection it
//! dumps the Bun-bundled minified source + JS stack into stdout; the Bun event
//! loop does NOT exit on unhandled rejection, so the process stays alive with
//! its PID visible, but tool calls never complete. The supervisor sees a
//! "crashed-looking" pane, a fresh heartbeat (daemon-faked), and no way to
//! distinguish "alive but starved", "wedged in JS crash screen", or "actually
//! dead" without manual triage.
//!
//! This module adds three operator verbs to `cas factory`:
//!
//! * `is-wedged <worker>` — classify the worker as Alive / Wedged / Starved /
//!   Dead / Unverified by combining PID liveness, transcript mtime, worktree
//!   edit recency, and a content grep for the Bun/React-Ink crash-screen
//!   signature. Exits with a differentiated code so supervisor skills can
//!   script. `Dead` requires at least two independent signals to agree
//!   (cas-f781) — a pid-only "gone" reading that's contradicted by a fresh
//!   transcript or worktree edit reports `Unverified` instead, since that
//!   combination is what a stale/wrong tracked pid looks like while the
//!   real worker is still alive.
//! * `debug <worker>` — print the tail of the worker's transcript JSONL so a
//!   supervisor can see the last in-flight tool call without attaching the
//!   TUI. Essential triage input before deciding to kill.
//! * `kill <worker>` — SIGKILL the worker (SIGTERM doesn't exit cleanly on
//!   the Bun wedge) and best-effort release the Cassy lease.
//!
//! See `cas-cli/src/mcp/tools/service/factory_ops.rs::resolve_transcript`
//! (cas-900b) for the transcript path resolver used by `is-wedged` / `debug`.
//!
//! Harness-awareness (cas-058f, EPIC cas-8888 Phase 4): the module doc above
//! describes Claude's Bun/React-Ink failure mode specifically — Grok workers
//! have a structurally different transcript layout (directory-per-session
//! with a `signals.json` turn/token counter file, no known crash-screen
//! signature) and are resolved/classified differently; see
//! [`resolve_worker`]'s harness branch and [`effective_transcript_age`] /
//! [`effective_crash_signature`]. Codex rollouts live under
//! `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and are matched by
//! `session_meta.cwd` (cas-c655); classification also refuses to declare
//! Starved solely on a missing transcript when the process is busy or the
//! worktree was recently written.

use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::mcp::tools::service::factory_ops::{
    resolve_worker_transcript_path_for_account, worker_cli_from_agent, worker_scope_paths,
};
use crate::mcp::tools::service::opencode_liveness;

/// Window in which a Claude transcript mtime counts as "recent" — used to
/// distinguish a worker that is still writing tool results (alive or
/// wedged) from one whose transcript has gone cold (starved or dead).
///
/// cas-7e85: widened from the original 60s to 3 minutes. The original
/// 60s value (30s heartbeat threshold + ~45s upper end of a single `cargo
/// test` run on a saturated host, cas-0bf4) undercounted this repo's real
/// full-gate duration badly — `cargo test --workspace --no-fail-fast` here
/// routinely runs multiple minutes, and workers correctly backgrounding it
/// with a `sleep N; check` loop (the low-token pattern supervisors ask for)
/// go quiet on the transcript for the whole wait. That produced four
/// confirmed false WorkerStalled alerts in one session (BUG report
/// 2026-07-27), each recommending `cas factory kill` against a live,
/// correctly-behaving worker. `has_in_flight_tool_call` (below) is the
/// primary fix for the long-single-call case (it works regardless of
/// window size), but this widened value is a real, evidence-based
/// complementary margin for the general "checkpoint cadence exceeds a
/// tight window" case — not just a nicety. Still deliberately shorter than
/// [`CODEX_TRANSCRIPT_FRESH_WINDOW`]; see
/// `classify_codex_window_keeps_3min_transcript_fresh` for the pinned
/// ordering.
///
/// Grok is intentionally NOT touched by this widening — see
/// [`GROK_TRANSCRIPT_FRESH_WINDOW`].
pub(crate) const TRANSCRIPT_FRESH_WINDOW: Duration = Duration::from_secs(3 * 60);

/// Grok's freshness window, held at the original 60s (cas-7e85: Grok is out
/// of scope for this task — no repro evidence involved a Grok worker, and
/// its `signals.json` corroborating signal (see [`grok_activity_age`]) is
/// already a finer-grained, more-precise-than-mtime activity signal than
/// what Claude/Codex have, so it doesn't share the same risk profile that
/// motivated widening [`TRANSCRIPT_FRESH_WINDOW`]). Deliberately a
/// SEPARATE constant from `TRANSCRIPT_FRESH_WINDOW` rather than a shared
/// one, precisely so a future widening of Claude's window can't silently
/// drag Grok's along with it again.
pub(crate) const GROK_TRANSCRIPT_FRESH_WINDOW: Duration = Duration::from_secs(60);

/// Codex workers routinely sit mid-inference for several minutes without a
/// Cassy tool call (and sometimes without a rollout append). A 60 s transcript
/// window false-flags them Starved; prefer a longer harness-specific window
/// (cas-c655 / 2026-07-21 bug report). Worktree + CPU activity still override
/// Starved even inside this window when the transcript is missing entirely.
pub(crate) const CODEX_TRANSCRIPT_FRESH_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Number of trailing JSONL lines inspected for the crash-screen signature.
///
/// Bumped to 200 from the original 20 after adversarial review (cas-4513)
/// flagged the tail-window gap: the Bun event loop can continue writing
/// transcript entries after an Ink crash renders on the PTY, and a single
/// long assistant reply can evict a 20-line crash block out of the
/// detection window. 200 lines comfortably covers roughly the last
/// half-dozen tool-call cycles on a typical transcript while still
/// bounding memory.
pub(crate) const CRASH_SIGNATURE_TAIL_LINES: usize = 200;

/// How long an unread Claude Code team permission request must remain pending
/// before it is an operator-visible approval hang rather than a transient
/// routing delay. The inbox is the authoritative signal because Claude can
/// suspend before writing the corresponding `tool_use` to its transcript.
pub(crate) const LEADER_APPROVAL_PENDING_THRESHOLD_SECS: u64 = 5 * 60;

const PENDING_COMMAND_EXCERPT_CHARS: usize = 240;

/// A Claude Code team permission request that is still waiting in the lead's
/// inbox. This deliberately carries a bounded command excerpt: it is enough
/// to identify the blocked operation without copying a potentially large or
/// secret-bearing tool payload into every status poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPermission {
    pub request_id: Option<String>,
    pub tool_name: String,
    pub command_excerpt: String,
    pub age_secs: u64,
}

fn command_excerpt(command: &str) -> String {
    let collapsed = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut excerpt: String = collapsed
        .chars()
        .take(PENDING_COMMAND_EXCERPT_CHARS)
        .collect();
    if collapsed.chars().count() > PENDING_COMMAND_EXCERPT_CHARS {
        excerpt.push('…');
    }
    excerpt
}

fn claude_config_root(account_dir: Option<&str>) -> Option<PathBuf> {
    let expand_home = |value: &str| {
        value.strip_prefix("~/").map_or_else(
            || PathBuf::from(value),
            |suffix| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(suffix))
                    .unwrap_or_else(|| PathBuf::from(value))
            },
        )
    };
    let configured = account_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(expand_home)
        .or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
    configured.or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".claude"))
    })
}

fn pending_permission_from_inbox(
    inbox: &serde_json::Value,
    worker_name: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<PendingPermission> {
    let mut newest: Option<(chrono::DateTime<chrono::Utc>, PendingPermission)> = None;
    for entry in inbox.as_array()? {
        if entry.get("from").and_then(serde_json::Value::as_str) != Some(worker_name)
            || entry.get("read").and_then(serde_json::Value::as_bool) != Some(false)
        {
            continue;
        }
        let Some(text) = entry.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        if request.get("type").and_then(serde_json::Value::as_str) != Some("permission_request") {
            continue;
        }
        let timestamp = entry
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc));
        let Some(timestamp) = timestamp else {
            continue;
        };
        let tool_name = request
            .get("tool_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let command = request
            .get("input")
            .and_then(|input| input.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let pending = PendingPermission {
            request_id: request
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            tool_name,
            command_excerpt: command_excerpt(command),
            age_secs: (now - timestamp).num_seconds().max(0) as u64,
        };
        if newest
            .as_ref()
            .is_none_or(|(newest_timestamp, _)| timestamp > *newest_timestamp)
        {
            newest = Some((timestamp, pending));
        }
    }
    newest.map(|(_, pending)| pending)
}

/// Read Claude Code's durable team-lead inbox. This is intentionally
/// best-effort: missing/unreadable state means the caller must fall back to
/// transcript and process evidence, never that a permission wait is present.
pub(crate) fn pending_permission_for_worker(
    worker_name: &str,
    factory_session: Option<&str>,
    account_dir: Option<&str>,
) -> Option<PendingPermission> {
    let session = factory_session?.trim();
    if session.is_empty() {
        return None;
    }
    let path = claude_config_root(account_dir)?
        .join("teams")
        .join(session)
        .join("inboxes")
        .join("team-lead.json");
    let body = std::fs::read_to_string(path).ok()?;
    let inbox = serde_json::from_str(&body).ok()?;
    pending_permission_from_inbox(&inbox, worker_name, chrono::Utc::now())
}

/// A pending team permission request is a hang only when the worker process
/// remains alive and the process table proves there is no child doing the
/// requested work. `Unavailable` is not treated as an empty process list.
pub(crate) fn is_leader_approval_hang(
    pid_alive: bool,
    pending_permission: Option<&PendingPermission>,
    background_processes: &BackgroundProcessState,
) -> bool {
    pid_alive
        && pending_permission
            .is_some_and(|pending| pending.age_secs >= LEADER_APPROVAL_PENDING_THRESHOLD_SECS)
        && matches!(
            background_processes,
            BackgroundProcessState::Available(processes) if processes.is_empty()
        )
}

/// Evidence collected by [`classify_worker`], surfaced verbatim in
/// `cas factory is-wedged` output so a supervisor can audit the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerEvidence {
    pub pid: Option<u32>,
    pub pid_alive: bool,
    pub transcript_path: Option<PathBuf>,
    pub transcript_mtime_age_secs: Option<u64>,
    pub crash_signature_match: bool,
    /// Age since the most recently modified dirty file under the worker's
    /// worktree (per `git status --porcelain`), if resolvable. Second
    /// corroborating signal for the Dead/Unverified split (cas-f781 AC c).
    pub worktree_edit_age_secs: Option<u64>,
    /// Raw session_id the classification resolved against (reported so the
    /// supervisor can grep the projects tree manually if they distrust the
    /// resolution, per the cas-900b always-surface-session-id contract).
    pub session_id: String,
    /// True when the transcript's trailing window has an outstanding tool
    /// call (cas-7e85) — see [`has_in_flight_tool_call`]. Surfaced so a
    /// supervisor reading `is-wedged` output can see WHY a stale-looking
    /// transcript still classified Alive.
    pub in_flight_tool_call: bool,
    /// Live user-work descendants of the resolved worker pane. This is
    /// intentionally process-tree evidence rather than durable hook state:
    /// backgrounded builds continue after the harness reports no in-flight
    /// tool call (cas-058e).
    pub background_processes: BackgroundProcessState,
    /// Unread Claude team-lead permission request, if one was found for this
    /// worker. This signal is independent of transcript `tool_use` records.
    pub pending_permission: Option<PendingPermission>,
}

/// A descendant process that is doing worker-owned work in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackgroundProcess {
    pub command: String,
    pub age_secs: u64,
}

/// The process tree is either observed completely enough to report, or not
/// available. `Unavailable` is deliberately distinct from an empty list: an
/// inaccessible `/proc` must never be rendered as "no background jobs".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackgroundProcessState {
    Available(Vec<BackgroundProcess>),
    Unavailable,
}

impl BackgroundProcessState {
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Self::Available(processes) if !processes.is_empty())
    }
}

/// Shared positive-work predicate for every liveness surface. A transcript
/// can say that no tool call is in flight while a background `cargo`/`rustc`
/// descendant is still running, so neither signal alone is authoritative.
pub(crate) fn has_active_work(
    in_flight_tool_call: bool,
    background_processes: &BackgroundProcessState,
) -> bool {
    in_flight_tool_call || background_processes.is_active()
}

/// Liveness classification produced by [`classify_worker`]. The variants are
/// intentionally operator-facing — they match the names a supervisor would
/// use in a runbook, not any internal state model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerLivenessState {
    /// PID alive, transcript fresh, no crash signature. Worker is running.
    Alive,
    /// PID alive, transcript fresh, crash signature matched. Worker is in
    /// the Bun/React-Ink wedge — SIGKILL + respawn is the recovery.
    Wedged,
    /// PID alive, no child process is doing the work, and Claude's team-lead
    /// inbox has held an unread permission request past the operator grace
    /// window. The request may not have a transcript `tool_use` record at all.
    ApprovalHang,
    /// PID alive, transcript stale (no writes in
    /// [`TRANSCRIPT_FRESH_WINDOW`]). Likely scheduler-starved or hung on a
    /// tool call. Often resolves with patience; not automatically fatal.
    Starved,
    /// PID gone AND a second signal corroborates it (transcript stale AND
    /// worktree not recently edited). The cleanup path is the same as
    /// SIGKILL-after-wedge (release lease, prune worktree). Not an error —
    /// just means the worker already exited.
    Dead,
    /// PID probe says gone, but the transcript is still fresh or the
    /// worktree was recently edited — a contradiction. cas-f781: this is
    /// exactly what a stale/wrong tracked pid looks like while the real
    /// worker is still alive and working. Never auto-reset a lease off this
    /// state alone; investigate with `debug` first.
    Unverified,
}

impl WorkerLivenessState {
    /// Process exit code for the `is-wedged` subcommand. Different values so
    /// supervisor bash scripts can branch without parsing stdout.
    ///
    /// Keep in sync with the `STATE_EXIT_CODES` constant asserted in
    /// `classify_worker_state_exit_codes_are_pinned`.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            WorkerLivenessState::Alive => 0,
            WorkerLivenessState::Wedged => 1,
            WorkerLivenessState::ApprovalHang => 5,
            WorkerLivenessState::Starved => 2,
            WorkerLivenessState::Dead => 3,
            WorkerLivenessState::Unverified => 4,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            WorkerLivenessState::Alive => "alive",
            WorkerLivenessState::Wedged => "wedged",
            WorkerLivenessState::ApprovalHang => "approval-hang",
            WorkerLivenessState::Starved => "starved",
            WorkerLivenessState::Dead => "dead",
            WorkerLivenessState::Unverified => "unverified",
        }
    }
}

/// React-Ink + Bun bundle signature bytes that leak into stdout when the CLI
/// throws an unhandled rejection. A match on any of these inside the
/// transcript's last [`CRASH_SIGNATURE_TAIL_LINES`] lines is sufficient —
/// each one independently identifies the crash-screen (cas-4513 discovery
/// note 2026-04-23 15:11 UTC captured all three in one pane).
///
/// Ordered most-specific-first: the literal Ink guard-text is a near-zero
/// false-positive signal and lives at the front. The bundler-path signals
/// (`/$bunfs/root`, `createInstance (/`) could, hypothetically, appear in
/// legitimate diagnostic output — hitting them alone is still a strong
/// enough signal to classify Wedged, but if the cheaper string match up
/// front catches the common case that's a clear win.
const CRASH_SIGNATURE_NEEDLES: &[&str] = &[
    // Literal React-Ink runtime invariant — when this renders, the UI is
    // guaranteed wedged (upstream: anthropics/claude-code#52337).
    "<Box> can't be nested inside <Text>",
    // React-Ink element construction leaking through the error handler.
    "createElement(\"ink-",
    "ink-box",
    // Bun single-file-bundle prefix — only appears in stack frames dumped
    // by the error handler, never in normal transcripts.
    "/$bunfs/root",
    "createInstance (/",
];

/// Pure classifier — takes pre-measured inputs so tests drive it without
/// touching the real PID table or filesystem. The orchestrating
/// [`classify_worker`] wrapper does the measurement; keeping the decision
/// logic separate means the 4-way branch is exhaustively unit-testable
/// without ptrace or tempdir dependencies.
///
/// `fresh_window` is harness-specific (see [`activity_fresh_window`]).
/// `process_busy` is true when the resolved worker PID is consuming CPU
/// (cas-c655: codex mid-inference must not be Starved solely because the
/// rollout path was unresolved or cold).
///
/// **cas-de95:** a **missing/unresolved** transcript (`transcript_mtime_age =
/// None`) is missing evidence, not proof of inactivity. Without a positive
/// progress signal (worktree / CPU) it classifies [`WorkerLivenessState::Unverified`]
/// rather than Starved — so director/auto-nudge paths must not treat telemetry
/// absence as starvation.
///
/// **cas-7e85:** `in_flight_tool_call` (from [`has_in_flight_tool_call`]) is
/// OR'd into the freshness determination — an outstanding tool call proves
/// the worker is actively waiting on real work regardless of how stale the
/// transcript mtime looks, so it counts as "fresh" for every downstream
/// branch (Alive/Wedged vs Starved/Dead). This is the single piece of
/// evidence shared verbatim between `cas factory is-wedged` and the
/// director's `WorkerStalled` gate (`events.rs::transcript_confirms_stall`)
/// — by construction, the alert and its own recommended `is-wedged` triage
/// command can no longer disagree about this specific signal.
pub(crate) fn classify_from_evidence(
    pid_alive: bool,
    transcript_mtime_age: Option<Duration>,
    crash_signature: bool,
    worktree_recent_edit_age: Option<Duration>,
    process_busy: bool,
    fresh_window: Duration,
    in_flight_tool_call: bool,
) -> WorkerLivenessState {
    classify_from_evidence_with_background(
        pid_alive,
        transcript_mtime_age,
        crash_signature,
        worktree_recent_edit_age,
        process_busy,
        fresh_window,
        in_flight_tool_call,
        false,
    )
}

/// Extended classifier used by the live process-tree orchestrator. Kept
/// separate from [`classify_from_evidence`] so existing pure callers retain a
/// compact fixture shape while both activity signals still meet at one branch.
pub(crate) fn classify_from_evidence_with_background(
    pid_alive: bool,
    transcript_mtime_age: Option<Duration>,
    crash_signature: bool,
    worktree_recent_edit_age: Option<Duration>,
    process_busy: bool,
    fresh_window: Duration,
    in_flight_tool_call: bool,
    background_process_active: bool,
) -> WorkerLivenessState {
    // Distinguish unresolved (None) from resolved-but-stale (Some(age ≥ window)).
    let transcript_resolved = transcript_mtime_age.is_some();
    let fresh = (in_flight_tool_call || background_process_active)
        || transcript_mtime_age
            .map(|age| age < fresh_window)
            .unwrap_or(false);
    let worktree_recent = worktree_recent_edit_age
        .map(|age| age < fresh_window)
        .unwrap_or(false);
    if !pid_alive {
        // cas-f781 AC c: a pid-only "not alive" reading must never emit
        // Dead by itself — require a second independent signal to
        // corroborate. If the transcript is still fresh OR the worktree
        // was recently edited while the pid probe says gone, that's a
        // contradiction: the concrete cas-f781 repro is a stale/wrong
        // tracked pid reading dead while the real worker process keeps
        // writing to its transcript and worktree. Report Unverified so a
        // caller (e.g. a supervisor auto-reset) doesn't treat one
        // contradicted signal as ground truth for a destructive action.
        return if fresh || worktree_recent {
            WorkerLivenessState::Unverified
        } else {
            WorkerLivenessState::Dead
        };
    }
    match (fresh, crash_signature) {
        (true, true) => WorkerLivenessState::Wedged,
        (true, false) => WorkerLivenessState::Alive,
        // Positive activity overrides a cold/missing transcript (cas-c655).
        (false, _) if worktree_recent || process_busy => WorkerLivenessState::Alive,
        // cas-de95: unresolved transcript + no other progress signal →
        // Unverified (investigate), not Starved. A resolved but stale
        // transcript with no activity remains genuine Starved.
        (false, _) if !transcript_resolved => WorkerLivenessState::Unverified,
        (false, _) => WorkerLivenessState::Starved,
    }
}

/// Harness-specific transcript freshness window. Codex gets the longest
/// grace period (cas-c655); Claude was widened to 3 minutes (cas-7e85);
/// Grok stays at the original 60s (cas-7e85: deliberately out of scope,
/// see [`GROK_TRANSCRIPT_FRESH_WINDOW`]).
pub(crate) fn activity_fresh_window(cli: cas_mux::SupervisorCli) -> Duration {
    match cli {
        cas_mux::SupervisorCli::Codex => CODEX_TRANSCRIPT_FRESH_WINDOW,
        cas_mux::SupervisorCli::Claude => TRANSCRIPT_FRESH_WINDOW,
        cas_mux::SupervisorCli::Grok => GROK_TRANSCRIPT_FRESH_WINDOW,
        // cas-7296 owns OpenCode liveness; zero never invents freshness.
        cas_mux::SupervisorCli::OpenCode => Duration::ZERO,
    }
}

/// Measure transcript mtime-age. `None` means the file doesn't exist or the
/// mtime could not be read — treated as "not fresh" by the classifier.
pub(crate) fn transcript_mtime_age(path: &Path) -> Option<Duration> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    SystemTime::now().duration_since(mtime).ok()
}

/// Age since the most recently modified path in the worker's scope — the "is
/// this worktree actively being edited" signal (cas-f781 AC c, third
/// corroborating signal alongside pid liveness and transcript mtime). Normal
/// worktrees use `git status --porcelain`; while `MERGE_HEAD` exists the shared
/// probe uses the branch's merge-base..HEAD contribution paths, so staged
/// incoming merge files cannot masquerade as this worker's activity/drift.
/// `.git/objects` and `target/` churn constantly regardless of real edits and
/// are never considered.
///
/// cas-c655: also considers the current branch tip (`git log -1 --format=%ct`)
/// so a worker that just committed/pushed (clean tree, fresh HEAD) still
/// counts as active. Takes the freshest (minimum age) of dirty-file mtime
/// and HEAD commit age.
///
/// `None` when `clone_path` isn't a git worktree, git isn't on `PATH`, and
/// neither dirty files nor HEAD resolve — callers must treat `None` as
/// "no signal", never as "confirmed clean".
pub(crate) fn worktree_recent_edit_age(clone_path: &Path) -> Option<Duration> {
    let dirty = worktree_dirty_file_age(clone_path);
    let tip = worktree_branch_tip_age(clone_path);
    match (dirty, tip) {
        (Some(d), Some(t)) => Some(d.min(t)),
        (Some(d), None) => Some(d),
        (None, Some(t)) => Some(t),
        (None, None) => None,
    }
}

fn worktree_dirty_file_age(clone_path: &Path) -> Option<Duration> {
    let mut newest: Option<Duration> = None;
    for rel in worker_scope_paths(clone_path).ok()? {
        if let Some(age) = transcript_mtime_age(&clone_path.join(rel)) {
            newest = Some(newest.map_or(age, |cur: Duration| cur.min(age)));
        }
    }
    newest
}

/// Age of `HEAD` on the worktree's current branch (`git log -1 --format=%ct`).
/// Used so a just-committed clean tree still counts as active (cas-c655).
pub(crate) fn worktree_branch_tip_age(clone_path: &Path) -> Option<Duration> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("log")
        .arg("-1")
        .arg("--format=%ct")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ts: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    let commit_time = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(ts))?;
    SystemTime::now().duration_since(commit_time).ok()
}

/// Collect the last `n` lines from `reader` via a bounded ring buffer.
/// Takes `Read` so tests drive it with `Cursor<Vec<u8>>`. A 0-line request
/// is a hard short-circuit — otherwise `VecDeque::with_capacity(0)` would
/// grow unboundedly as every iteration hits `ring.len() == 0` (a no-op
/// `pop_front` on empty, then `push_back`). cas-4513 P2 correctness catch.
pub(crate) fn collect_tail_lines<R: Read>(reader: R, n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let bufread = BufReader::new(reader);
    let mut ring: std::collections::VecDeque<String> = std::collections::VecDeque::with_capacity(n);
    for line in bufread.lines().map_while(Result::ok) {
        if ring.len() == n {
            ring.pop_front();
        }
        ring.push_back(line);
    }
    ring.into_iter().collect()
}

/// Grep the last [`CRASH_SIGNATURE_TAIL_LINES`] lines of `reader` for any of
/// [`CRASH_SIGNATURE_NEEDLES`]. Takes `Read` so tests can point at a
/// `std::io::Cursor<Vec<u8>>` without touching the filesystem. Large
/// transcripts (thousands of lines) are fine — we only retain a bounded tail
/// window in memory.
pub(crate) fn has_crash_signature<R: Read>(reader: R, tail_lines: usize) -> bool {
    let tail = collect_tail_lines(reader, tail_lines);
    tail.iter().any(|l| {
        CRASH_SIGNATURE_NEEDLES
            .iter()
            .any(|needle| l.contains(*needle))
    })
}

/// Convenience wrapper that opens `path` and runs [`has_crash_signature`].
/// Missing or unreadable files read as "no signature" — the classifier then
/// treats the absence as Alive/Starved based on pid + mtime alone.
pub(crate) fn transcript_has_crash_signature(path: &Path, tail_lines: usize) -> bool {
    match std::fs::File::open(path) {
        Ok(f) => has_crash_signature(f, tail_lines),
        Err(_) => false,
    }
}

/// Number of trailing JSONL lines inspected for an in-flight (unresolved)
/// tool call (cas-7e85). Mirrors [`CRASH_SIGNATURE_TAIL_LINES`]'s reasoning:
/// a single long assistant reply, or a burst of parallel tool calls, can
/// push the still-open call out of a smaller window.
pub(crate) const IN_FLIGHT_TAIL_LINES: usize = 200;

/// True when the trailing window of `reader`'s JSONL transcript contains a
/// tool/function call that has been requested but not yet completed — a
/// `tool_use`/`function_call`-shaped entry with no matching
/// `tool_result`/`function_call_output` later in the window.
///
/// cas-7e85: this is the decisive signal the WorkerStalled false positives
/// (BUG report 2026-07-27, four confirmed specimens) were missing. A worker
/// executing a long-running tool call — e.g. `Bash: sleep 280` while a
/// backgrounded `cargo test --no-fail-fast` gate runs — produces NO
/// transcript writes for the call's whole duration. Mtime-based freshness
/// can never distinguish that from a genuinely wedged worker, no matter how
/// the freshness window is tuned (widening the window only pushes the false
/// positive further out, it never eliminates it — see the widened
/// [`TRANSCRIPT_FRESH_WINDOW`] below, which is a complementary defense, not
/// a substitute for this check). But the transcript already recorded the
/// call's *start* before it went quiet; checking for that unresolved entry
/// is a structural signal, not a timing one, so it works regardless of how
/// long the call runs.
///
/// Deliberately broader than the ticket's literal "the LAST entry is an
/// unmatched tool_use": this scans the whole tail window and tracks ALL
/// pending call ids, not just whichever block happens to be the final JSON
/// line. A turn can end with plain assistant text after issuing a tool
/// call, or issue several parallel calls where only some have resolved —
/// either way, "a call this worker is waiting on has not come back yet" is
/// the property that actually matters, and the last-line-only reading would
/// miss both cases.
///
/// Harness-aware — see the harness-specific parsers below for schema
/// detail and confidence level:
/// - **Claude**: real, well-understood schema (Anthropic Messages API
///   shape); this is the harness all four reported false positives came
///   from, so it's implemented with confidence.
/// - **Codex**: best-effort. Inferred from the Responses-API-style
///   `response_item`/`payload.type` shape already used elsewhere in this
///   codebase for `session_meta` (`factory_ops.rs::resolve_codex_transcript`)
///   — no local fixture exists for the tool-call entries specifically
///   (grepped the repo for `function_call`/`local_shell_call`/`call_id`
///   before writing this: zero hits). Fails safe: an unrecognized entry
///   shape never reports "in flight" — it just falls through to the
///   pre-existing mtime-based classification unchanged. Confirm against a
///   real Codex rollout before trusting this path the way the Claude path
///   is trusted.
/// - **Grok**: out of scope for this pass (no repro evidence involved a
///   Grok worker, and its directory-per-session + `signals.json` layout is
///   the one wedged.rs's own comments actually flag as structurally
///   different — see the module doc). Always returns `false`, deferring
///   entirely to the existing `signals.json`/mtime-based freshness check.
pub(crate) fn has_in_flight_tool_call<R: Read>(
    reader: R,
    cli: cas_mux::SupervisorCli,
    tail_lines: usize,
) -> bool {
    let lines = collect_tail_lines(reader, tail_lines);
    match cli {
        cas_mux::SupervisorCli::Grok => false,
        cas_mux::SupervisorCli::Claude => claude_has_pending_tool_call(&lines),
        cas_mux::SupervisorCli::Codex => codex_has_pending_tool_call(&lines),
        // cas-7296 owns OpenCode tool-activity evidence.
        cas_mux::SupervisorCli::OpenCode => false,
    }
}

/// Claude Code transcript entries are `{"type":"assistant"|"user"|..,
/// "message":{"role":..,"content":[...]}}`; tool blocks live inside
/// `content` as `{"type":"tool_use","id":"toolu_..","..}` (assistant turn)
/// and `{"type":"tool_result","tool_use_id":"toolu_..","..}` (the following
/// user/tool turn). Track every `tool_use` id seen, drop it when its
/// `tool_result` shows up; anything left pending after the whole window is
/// an outstanding call. Malformed/non-JSON lines (partial writes, etc.) and
/// lines with no `message.content` array are silently skipped — they carry
/// no tool-call evidence either way.
fn claude_has_pending_tool_call(lines: &[String]) -> bool {
    let mut pending: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in lines {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(content) = value.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    if let Some(id) = block.get("id").and_then(|v| v.as_str()) {
                        pending.insert(id.to_string());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                        pending.remove(id);
                    }
                }
                _ => {}
            }
        }
    }
    !pending.is_empty()
}

/// Codex rollout entries relevant here are `{"type":"response_item",
/// "payload":{"type":"function_call"|"local_shell_call"|
/// "function_call_output"|"local_shell_call_output","call_id":"..",..}}`.
/// Same pending-id tracking as the Claude parser. See
/// [`has_in_flight_tool_call`]'s doc for the confidence caveat on this
/// schema — unrecognized shapes are silently skipped, never treated as
/// evidence of an in-flight call.
fn codex_has_pending_tool_call(lines: &[String]) -> bool {
    let mut pending: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in lines {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str());
        let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
            continue;
        };
        match payload_type {
            Some("function_call" | "local_shell_call") => {
                pending.insert(call_id.to_string());
            }
            Some("function_call_output" | "local_shell_call_output") => {
                pending.remove(call_id);
            }
            _ => {}
        }
    }
    !pending.is_empty()
}

/// Convenience wrapper that opens `path` and runs [`has_in_flight_tool_call`].
/// Missing or unreadable files read as "no in-flight call" — fail safe,
/// falling through to mtime-based classification instead of masking a
/// genuine stall on a read error.
pub(crate) fn transcript_has_in_flight_tool_call(path: &Path, cli: cas_mux::SupervisorCli) -> bool {
    match std::fs::File::open(path) {
        Ok(f) => has_in_flight_tool_call(f, cli, IN_FLIGHT_TAIL_LINES),
        Err(_) => false,
    }
}

/// Age of the freshest available Grok activity signal for a resolved
/// transcript path (which points at `updates.jsonl`, the authoritative ACP
/// log — see `factory_ops::resolve_grok_transcript`). Prefers the sibling
/// `signals.json` file (rewritten on every turn/token-count change — a
/// finer-grained activity signal than JSONL mtime, cas-058f AC) when it
/// exists and is readable, falling back to `updates.jsonl`'s own mtime
/// otherwise. Both files live in the same `<session-uuid>` directory, so
/// `signals.json` is always a sibling of the resolved transcript path.
fn grok_activity_age(updates_jsonl_path: &Path) -> Option<Duration> {
    let signals_path = updates_jsonl_path.with_file_name("signals.json");
    let signals_age = transcript_mtime_age(&signals_path);
    let updates_age = transcript_mtime_age(updates_jsonl_path);
    // cas-921f P2: take the FRESHEST (minimum age) of the two, not a strict
    // "prefer signals.json, only fall back when it's absent" preference.
    // Grok appends to updates.jsonl continuously mid-turn but may only
    // rewrite signals.json at turn boundaries — an actively-working worker
    // can have a fresh updates.jsonl and a stale signals.json at the same
    // instant. Strictly preferring signals.json would return the STALE age
    // in that case and misclassify the worker Starved — exactly the
    // mid-think false-flag this whole harness-aware path exists to prevent.
    match (signals_age, updates_age) {
        (Some(s), Some(u)) => Some(s.min(u)),
        (Some(s), None) => Some(s),
        (None, Some(u)) => Some(u),
        (None, None) => None,
    }
}

/// Harness-aware transcript freshness: Grok prefers `signals.json` (see
/// [`grok_activity_age`]); Claude and Codex use the transcript/rollout's
/// own mtime (Codex rollouts resolved via `factory_ops::resolve_codex_transcript`,
/// cas-c655).
///
/// `pub(crate)` (cas-c2c2): reused by
/// `factory_ops::last_worker_activity_secs_with_transcript` so the
/// `worker_status` "last activity" display folds in the SAME per-harness
/// transcript-freshness primitive `is-wedged`/the director's stall gate
/// already trust, instead of a second ad-hoc mtime read that would drift
/// out of sync with Grok's `signals.json` preference.
pub(crate) fn effective_transcript_age(
    path: &Path,
    cli: cas_mux::SupervisorCli,
) -> Option<Duration> {
    match cli {
        cas_mux::SupervisorCli::Grok => grok_activity_age(path),
        cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Codex => {
            transcript_mtime_age(path)
        }
        // cas-7296 owns OpenCode session activity; shared DB mtime is invalid.
        cas_mux::SupervisorCli::OpenCode => None,
    }
}

/// Harness-aware crash-signature detection. [`CRASH_SIGNATURE_NEEDLES`] are
/// Claude/Bun/React-Ink-specific — cas-058f audited whether they apply to
/// Grok's UI stack and found no evidence they would (Grok isn't a Bun/Ink
/// CLI), and no Grok-specific crash signature is known yet. Codex is also
/// not a Bun/Ink CLI (cas-c655) — skip the check for both. `Wedged` simply
/// isn't a classification those harnesses support today; a stalled/crashed
/// worker still correctly falls into `Starved`/`Dead`/`Unverified` via the
/// other signals.
fn effective_crash_signature(path: &Path, cli: cas_mux::SupervisorCli) -> bool {
    match cli {
        cas_mux::SupervisorCli::Grok | cas_mux::SupervisorCli::Codex => false,
        // cas-7296 owns OpenCode crash evidence.
        cas_mux::SupervisorCli::OpenCode => false,
        cas_mux::SupervisorCli::Claude => {
            transcript_has_crash_signature(path, CRASH_SIGNATURE_TAIL_LINES)
        }
    }
}

/// Orchestrator that combines PID liveness, transcript mtime, and signature
/// grep. Called by all three verbs with the same inputs so a Wedged decision
/// in one surfaces consistently in the others.
///
/// `pid_alive_probe` is injectable so tests don't need to exercise the real
/// `kill(pid, 0)` path (cas-2749's `pid_alive` helper covers production).
/// `process_busy_probe` is injectable for the same reason (cas-c655 CPU
/// activity signal). `cli` (cas-058f) selects the harness-appropriate
/// freshness/crash-signature logic — see [`effective_transcript_age`] /
/// [`effective_crash_signature`] / [`activity_fresh_window`].
pub(crate) fn classify_worker<F, G, H>(
    pid: Option<u32>,
    transcript_path: Option<&Path>,
    clone_path: Option<&Path>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
    pid_alive_probe: F,
    worktree_age_probe: G,
    process_busy_probe: H,
) -> (WorkerLivenessState, WorkerEvidence)
where
    F: FnOnce(u32) -> bool,
    G: FnOnce(&Path) -> Option<Duration>,
    H: FnOnce(u32) -> bool,
{
    classify_worker_with_pending(
        pid,
        transcript_path,
        clone_path,
        session_id,
        cli,
        pid_alive_probe,
        worktree_age_probe,
        process_busy_probe,
        None,
    )
}

/// Classify a worker with an optional Claude team permission request. The
/// ordinary classifier remains available to pure callers that do not have a
/// worker identity from which to resolve the team inbox.
pub(crate) fn classify_worker_with_pending<F, G, H>(
    pid: Option<u32>,
    transcript_path: Option<&Path>,
    clone_path: Option<&Path>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
    pid_alive_probe: F,
    worktree_age_probe: G,
    process_busy_probe: H,
    pending_permission: Option<PendingPermission>,
) -> (WorkerLivenessState, WorkerEvidence)
where
    F: FnOnce(u32) -> bool,
    G: FnOnce(&Path) -> Option<Duration>,
    H: FnOnce(u32) -> bool,
{
    let pid_alive = pid.map(pid_alive_probe).unwrap_or(false);
    let process_busy = pid
        .filter(|_| pid_alive)
        .map(process_busy_probe)
        .unwrap_or(false);
    let (age_opt, sig, in_flight) = match transcript_path {
        Some(p) => (
            effective_transcript_age(p, cli),
            effective_crash_signature(p, cli),
            transcript_has_in_flight_tool_call(p, cli),
        ),
        None => (None, false, false),
    };
    let worktree_age = clone_path.and_then(worktree_age_probe);
    let background_processes = pid
        .filter(|_| pid_alive)
        .map(background_processes_for)
        .unwrap_or(BackgroundProcessState::Unavailable);
    let state = if is_leader_approval_hang(
        pid_alive,
        pending_permission.as_ref(),
        &background_processes,
    ) && !in_flight
    {
        WorkerLivenessState::ApprovalHang
    } else if background_processes.is_active() {
        classify_from_evidence_with_background(
            pid_alive,
            age_opt,
            sig,
            worktree_age,
            process_busy,
            activity_fresh_window(cli),
            in_flight,
            true,
        )
    } else {
        classify_from_evidence(
            pid_alive,
            age_opt,
            sig,
            worktree_age,
            process_busy,
            activity_fresh_window(cli),
            in_flight,
        )
    };
    let evidence = WorkerEvidence {
        pid,
        pid_alive,
        transcript_path: transcript_path.map(PathBuf::from),
        transcript_mtime_age_secs: age_opt.map(|d| d.as_secs()),
        crash_signature_match: sig,
        worktree_edit_age_secs: worktree_age.map(|d| d.as_secs()),
        session_id: session_id.to_string(),
        in_flight_tool_call: in_flight,
        background_processes,
        pending_permission,
    };
    (state, evidence)
}

/// Overlay the mapped OpenCode session signal on the generic process/worktree
/// classifier. The generic path is retained only as process/worktree evidence
/// when the plugin mapping is absent; it never supplies a Claude transcript.
pub(crate) fn overlay_opencode_liveness(
    fallback: WorkerLivenessState,
    verdict: cas_mux::OpenCodeLivenessVerdict,
    pid_alive: bool,
) -> WorkerLivenessState {
    match verdict {
        cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Busy) => {
            if pid_alive {
                WorkerLivenessState::Alive
            } else {
                WorkerLivenessState::Unverified
            }
        }
        cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Idle) => {
            if pid_alive {
                WorkerLivenessState::Alive
            } else {
                WorkerLivenessState::Dead
            }
        }
        cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Error) => {
            if pid_alive {
                WorkerLivenessState::Wedged
            } else {
                WorkerLivenessState::Dead
            }
        }
        cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Deleted) => {
            if pid_alive {
                WorkerLivenessState::Unverified
            } else {
                WorkerLivenessState::Dead
            }
        }
        cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Unknown)
        | cas_mux::OpenCodeLivenessVerdict::NotObserved => fallback,
        cas_mux::OpenCodeLivenessVerdict::ProcessAliveFallback => WorkerLivenessState::Alive,
    }
}

/// Resolve `(pid, clone_path, session_id, transcript_path)` for a worker by
/// name. Reads the active agent row from AgentStore. Returns an error if the
/// worker is unknown or has no registered PID — the verbs treat that as a
/// hard stop rather than making up evidence.
pub(crate) fn resolve_worker(cas_root: &Path, worker_name: &str) -> Result<ResolvedWorker> {
    use cas_store::{AgentStore, SqliteAgentStore};
    use cas_types::AgentStatus;
    let store = SqliteAgentStore::open(cas_root).with_context(|| "open agent store")?;
    let mut matches: Vec<_> = [AgentStatus::Active, AgentStatus::Stale]
        .iter()
        .flat_map(|s| store.list(Some(*s)).unwrap_or_default())
        .filter(|a| a.name == worker_name)
        .collect();
    // Same name could be registered Stale + Active — prefer Active, then the
    // most recently registered identity.
    //
    // cas-7787 (GH #160): a harness session restart re-registers the same pane
    // name under a new agent id, so "Active" alone does not pick one row. The
    // sort used to stop there, leaving the winner to `sort_by_key`'s
    // stability — i.e. to whatever `ORDER BY registered_at DESC` happened to
    // yield across two separate `list()` calls. Whoever loses that toss is a
    // superseded session whose transcript is frozen, and a frozen transcript
    // reads as "tool call still in flight" forever, which permanently vetoes
    // the supervisor wake gate. Break the tie on registration recency so the
    // live session is the one whose transcript is treated as evidence.
    matches.sort_by(|a, b| {
        let rank = |s: AgentStatus| match s {
            AgentStatus::Active => 0,
            AgentStatus::Stale => 1,
            _ => 2,
        };
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| b.registered_at.cmp(&a.registered_at))
    });
    let agent = matches
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no worker named `{worker_name}` in agent store"))?;
    let pid = agent.pid;
    let clone_path = agent.metadata.get("clone_path").cloned();
    // factory-mode agents: id IS the CC session UUID (see cas-900b caller
    // comment). cc_session_id is populated in some non-factory registration
    // flows; prefer it when available.
    let session_id = agent
        .cc_session_id
        .clone()
        .unwrap_or_else(|| agent.id.clone());
    // cas-058f / cas-c655 / cas-fa69: use the exact same harness-aware path
    // resolver as worker_status so the human and director stall surfaces
    // cannot disagree about which transcript is evidence.
    let cli = worker_cli_from_agent(&agent);
    let account_dir = agent.metadata.get("worker_account_dir").cloned();
    let transcript_path = resolve_worker_transcript_path_for_account(
        clone_path.as_deref(),
        &session_id,
        cli,
        agent.metadata.get("worker_account_dir").map(String::as_str),
    );
    Ok(ResolvedWorker {
        name: worker_name.to_string(),
        pid,
        // cas-4513 adversarial P0: thread the pid_starttime fingerprint
        // from the agent row so `execute_kill` can guard against a PID
        // that was recycled after the agent record was written. Falls
        // back to the stringly-typed metadata key for legacy rows
        // predating cas-b157's typed promotion.
        pid_starttime: agent.pid_starttime.or_else(|| {
            agent
                .metadata
                .get(crate::mcp::daemon::PID_STARTTIME_KEY)
                .and_then(|s| s.parse::<u64>().ok())
        }),
        clone_path,
        session_id,
        cli,
        transcript_path,
        account_dir,
        factory_session: agent.factory_session.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWorker {
    pub name: String,
    pub pid: Option<u32>,
    /// `/proc/<pid>/stat` starttime fingerprint, when the registration
    /// path captured one. Used by `execute_kill` to refuse SIGKILL on a
    /// PID whose fingerprint no longer matches (= PID was recycled).
    pub pid_starttime: Option<u64>,
    pub clone_path: Option<String>,
    pub session_id: String,
    /// The harness this worker runs (cas-058f) — determines transcript
    /// layout and freshness/crash-signature semantics in `classify_worker`.
    pub cli: cas_mux::SupervisorCli,
    pub transcript_path: Option<PathBuf>,
    pub account_dir: Option<String>,
    /// Native Claude Agent Teams session containing the lead inbox. Kept
    /// alongside the worker identity so `is-wedged` can inspect pending
    /// permission state without guessing from the current process env.
    pub factory_session: Option<String>,
}

// ---------------------------------------------------------------------------
// Subcommand execution — thin glue between clap args and the helpers above.
// ---------------------------------------------------------------------------

/// `cas factory is-wedged <worker>`: classify + print evidence + exit.
pub(crate) fn execute_is_wedged(cas_root: Option<&Path>, worker: &str, json: bool) -> Result<()> {
    let cas_root =
        cas_root.ok_or_else(|| anyhow!("--cas-root required or run from a Cassy project"))?;
    // Scope the store opens so their SqliteConnection drops (running any
    // pending WAL checkpoint) before we call `std::process::exit` — that
    // function skips Rust destructors entirely. cas-4513 adversarial P2.
    let exit_code = {
        let w = resolve_worker(cas_root, worker)?;
        // cas-c655 / cas-f781: the agent-store PID is frequently the MCP
        // `cas serve` child (self-registration), not the real worker. Prefer
        // a live process-table match (argv --agent-name / CAS_AGENT_NAME,
        // with codex binary preferred over cas-serve descendants).
        let resolved_pid = find_worker_pid(&RealProcessTable, &w.name);
        let pid = pick_kill_pid(w.pid, resolved_pid);
        let env_factory_session = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .filter(|session| !session.trim().is_empty());
        let factory_session = w
            .factory_session
            .as_deref()
            .or(env_factory_session.as_deref());
        let pending_permission =
            pending_permission_for_worker(&w.name, factory_session, w.account_dir.as_deref());
        let (fallback, mut evidence) = classify_worker_with_pending(
            pid,
            if w.cli == cas_mux::SupervisorCli::OpenCode {
                None
            } else {
                w.transcript_path.as_deref()
            },
            w.clone_path.as_deref().map(Path::new),
            &w.session_id,
            w.cli,
            crate::mcp::daemon::pid_alive,
            worktree_recent_edit_age,
            process_cpu_busy,
            pending_permission,
        );
        let opencode_observation = if w.cli == cas_mux::SupervisorCli::OpenCode {
            opencode_liveness::observe(
                cas_root,
                &w.session_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or_default(),
                evidence.pid_alive,
            )
        } else {
            None
        };
        let state = if let Some(observation) = opencode_observation.as_ref() {
            evidence.in_flight_tool_call = opencode_liveness::active_tool(observation);
            overlay_opencode_liveness(fallback, observation.verdict, evidence.pid_alive)
        } else {
            fallback
        };
        if json {
            if let Some(observation) = opencode_observation.as_ref() {
                println!(
                    "{}",
                    format_state_json_with_opencode(&state, &evidence, observation)
                );
            } else {
                println!("{}", format_state_json(&state, &evidence));
            }
        } else {
            println!("{}", format_state_human(&state, &evidence));
            if let Some(observation) = opencode_observation.as_ref() {
                println!(
                    "OpenCode: {}",
                    opencode_liveness::verdict_label(observation.verdict)
                );
                println!(
                    "OpenCode session: {}",
                    opencode_liveness::mapped_session_id(observation)
                        .unwrap_or("<pending ses_* mapping>")
                );
            } else if w.cli == cas_mux::SupervisorCli::OpenCode {
                println!("OpenCode session: <mapping unavailable/delayed>");
            }
        }
        state.exit_code()
    };
    std::process::exit(exit_code);
}

/// `cas factory debug <worker>`: print tail of worker transcript.
pub(crate) fn execute_debug(cas_root: Option<&Path>, worker: &str, tail: usize) -> Result<()> {
    let cas_root =
        cas_root.ok_or_else(|| anyhow!("--cas-root required or run from a Cassy project"))?;
    let w = resolve_worker(cas_root, worker)?;
    if w.cli == cas_mux::SupervisorCli::OpenCode {
        let process_alive = w.pid.is_some_and(crate::mcp::daemon::pid_alive);
        let observation = opencode_liveness::observe(
            cas_root,
            &w.session_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or_default(),
            process_alive,
        )
        .ok_or_else(|| {
            anyhow!(
                "OpenCode session mapping unavailable/delayed for CAS session {}; no Claude transcript fallback",
                w.session_id
            )
        })?;
        let session_id = opencode_liveness::mapped_session_id(&observation).ok_or_else(|| {
            anyhow!(
                "OpenCode session mapping unavailable/delayed for CAS session {}; no Claude transcript fallback",
                w.session_id
            )
        })?;
        let export = opencode_liveness::export_session(&observation, w.account_dir.as_deref())
            .map_err(|error| anyhow!("bounded OpenCode export for {session_id} failed: {error}"))?;
        println!("# OpenCode session export: {session_id}");
        println!(
            "# liveness: {}",
            opencode_liveness::verdict_label(observation.verdict)
        );
        println!("# tail: bounded export ({} bytes)\n", export.len());
        print!("{export}");
        return Ok(());
    }
    let Some(path) = w.transcript_path.as_deref() else {
        bail!(
            "no transcript found for worker `{worker}` (session {}). Try `cas factory status` \
             to see what the agent store knows.",
            w.session_id
        );
    };
    let lines = read_last_lines(path, tail)
        .with_context(|| format!("read transcript at {}", path.display()))?;
    println!("# transcript: {}", path.display());
    println!("# session:    {}", w.session_id);
    println!("# tail:       {} lines\n", lines.len());
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// Minimal abstraction over the OS process table, injected so tests can
/// simulate `/proc` contents without spawning real processes. Real usage is
/// [`RealProcessTable`]; tests provide an in-memory fake. cas-f781.
pub(crate) trait ProcessTable {
    /// All PIDs currently visible in the table.
    fn pids(&self) -> Vec<u32>;
    /// Raw `/proc/<pid>/cmdline` bytes (NUL-separated argv), if readable.
    fn cmdline(&self, pid: u32) -> Option<Vec<u8>>;
    /// Raw `/proc/<pid>/environ` bytes (NUL-separated `KEY=VALUE`), if
    /// readable. Codex workers carry their identity here rather than in
    /// argv (cas-f781 investigation: the `codex` CLI has no `--agent-name`
    /// equivalent, only the `CAS_AGENT_NAME` env var).
    fn environ(&self, pid: u32) -> Option<Vec<u8>>;
}

/// Live `/proc` implementation. Linux-only, matching the existing
/// `read_pid_starttime` / fingerprint-guard gating in `daemon.rs` — other
/// platforms get an empty table and [`find_worker_pid`] always falls back
/// to the tracked pid.
pub(crate) struct RealProcessTable;

impl ProcessTable for RealProcessTable {
    #[cfg(target_os = "linux")]
    fn pids(&self) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    fn pids(&self) -> Vec<u32> {
        Vec::new()
    }

    #[cfg(target_os = "linux")]
    fn cmdline(&self, pid: u32) -> Option<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/cmdline")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    fn cmdline(&self, _pid: u32) -> Option<Vec<u8>> {
        None
    }

    #[cfg(target_os = "linux")]
    fn environ(&self, pid: u32) -> Option<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/environ")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    fn environ(&self, _pid: u32) -> Option<Vec<u8>> {
        None
    }
}

/// Extract the value of a `--agent-name <value>` argument from raw
/// NUL-separated `/proc/<pid>/cmdline` bytes. Scans tokens rather than
/// assuming a fixed position so a `nice -n <N> claude ...` wrapper
/// (`maybe_wrap_with_nice`, cas-pty) doesn't shift the match.
pub(crate) fn agent_name_from_cmdline(cmdline: &[u8]) -> Option<String> {
    let tokens: Vec<&str> = cmdline
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
        .collect();
    tokens
        .iter()
        .position(|t| *t == "--agent-name")
        .and_then(|i| tokens.get(i + 1))
        .map(|s| s.to_string())
}

/// Extract `CAS_AGENT_NAME=<value>` from raw NUL-separated
/// `/proc/<pid>/environ` bytes — the Codex worker identity signal, since
/// Codex's argv carries no `--agent-name` flag (cas-f781 investigation).
pub(crate) fn agent_name_from_environ(environ: &[u8]) -> Option<String> {
    environ
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .find_map(|entry| {
            std::str::from_utf8(entry)
                .ok()
                .and_then(|s| s.strip_prefix("CAS_AGENT_NAME="))
                .map(|v| v.to_string())
        })
}

/// Scan the live process table for the pid whose cmdline or environ
/// identifies it as `worker_name`. This is the authoritative resolution
/// [`execute_kill`] / [`execute_is_wedged`] trust over the agent store's
/// `pid` column — that column can be overwritten by an unrelated process's
/// self-registration (cas-f781 discovery: an MCP-server child process
/// re-registers over the real `claude --agent-name <worker>` pid using its
/// own `std::process::id()`). Matching against a live process's own
/// argv/environ is a direct identity proof, unlike a stored pid that might
/// describe the wrong process entirely.
pub(crate) fn find_worker_pid<T: ProcessTable + ?Sized>(
    table: &T,
    worker_name: &str,
) -> Option<u32> {
    let pids = table.pids();
    // cas-a91b: argv (`--agent-name`) is only ever present on the actual
    // `claude`/leader process's OWN command line — unlike an env var, argv
    // is never copied to child processes. `CAS_AGENT_NAME`, by contrast, is
    // *inherited* by every descendant the worker spawns (its `cas serve`
    // child, git, cargo, ...), so an environ-only match is ambiguous — it
    // could be the leader or any of its children, and `ProcessTable::pids()`
    // order is unspecified. Search cmdline across ALL pids first (a global
    // pass, not interleaved per-pid); only fall back to the environ signal
    // (needed for Codex, whose argv carries no identifying flag at all) when
    // no process's own argv identifies it as this worker.
    if let Some(pid) = pids.iter().copied().find(|&pid| {
        table
            .cmdline(pid)
            .and_then(|c| agent_name_from_cmdline(&c))
            .as_deref()
            == Some(worker_name)
    }) {
        return Some(pid);
    }
    // cas-c655: among environ matches, prefer the real harness binary
    // (codex/claude/grok) over inherited descendants like `cas serve`.
    // Score all candidates and pick the highest; ties break on lowest pid
    // for determinism under unspecified `pids()` order.
    let mut best: Option<(u32, i32)> = None;
    for pid in pids {
        let Some(environ) = table.environ(pid) else {
            continue;
        };
        if agent_name_from_environ(&environ).as_deref() != Some(worker_name) {
            continue;
        }
        let score = environ_candidate_score(table.cmdline(pid).as_deref());
        match best {
            Some((_, best_score)) if score < best_score => {}
            Some((best_pid, best_score)) if score == best_score && pid >= best_pid => {}
            _ => best = Some((pid, score)),
        }
    }
    best.map(|(pid, _)| pid)
}

/// Rank an environ-matched process by how likely it is to be the actual
/// worker harness rather than a descendant that inherited `CAS_AGENT_NAME`.
/// Higher is better. Pure function of cmdline bytes so tests drive it
/// without `/proc`.
pub(crate) fn environ_candidate_score(cmdline: Option<&[u8]>) -> i32 {
    let Some(raw) = cmdline else {
        return 40;
    };
    let tokens: Vec<&str> = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
        .collect();
    if tokens.is_empty() {
        return 40;
    }
    let joined = tokens.join(" ").to_ascii_lowercase();
    // MCP self-registration child — the cas-f781 / cas-c655 false-pid
    // source. Hard deprioritize so is-wedged doesn't report cas-serve as
    // the worker.
    if tokens.iter().any(|t| {
        let base = std::path::Path::new(t)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(t)
            .to_ascii_lowercase();
        base == "cas" || base.starts_with("cas-")
    }) && joined.contains("serve")
    {
        return 0;
    }
    // Prefer harness binaries (argv0 basename or path component).
    for t in &tokens {
        let base = std::path::Path::new(t)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(t)
            .to_ascii_lowercase();
        if base == "codex" || base.starts_with("codex-") {
            return 100;
        }
        if base == "claude" || base.starts_with("claude") {
            return 100;
        }
        if base == "grok" || base.starts_with("grok") {
            return 100;
        }
        if base == "opencode" || base.starts_with("opencode-") {
            return 100;
        }
    }
    // Common tool descendants of a worker — keep above cas-serve but well
    // below the harness so a busy cargo child doesn't win when codex is
    // also visible.
    if joined.contains("cargo")
        || joined.contains("rustc")
        || joined.contains("git")
        || joined.contains("node")
        || joined.contains("python")
    {
        return 10;
    }
    50
}

/// Enumerate live descendants of a worker's resolved pane PID on Linux.
///
/// The kernel's `children` files give a bounded, ownership-preserving walk:
/// a cargo build started by the worker is a descendant, while an unrelated
/// host build never is. Process start ages use the same `/proc/<pid>/stat`
/// starttime fingerprint source used elsewhere in Cassy. If either the root
/// traversal or uptime clock cannot be read, return `Unavailable` rather than
/// pretending there are no jobs (cas-058e's fail-honest contract).
pub(crate) fn background_processes_for(root_pid: u32) -> BackgroundProcessState {
    #[cfg(target_os = "linux")]
    {
        let Some(uptime_secs) = proc_uptime_secs() else {
            return BackgroundProcessState::Unavailable;
        };
        let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks_per_second <= 0 {
            return BackgroundProcessState::Unavailable;
        }
        let mut queue = VecDeque::from([root_pid]);
        let mut visited = HashSet::from([root_pid]);
        let mut processes = Vec::new();
        while let Some(parent) = queue.pop_front() {
            let Some(children) = descendant_pids(parent) else {
                // A vanished descendant is normal; a root we cannot inspect
                // is not. The latter would turn an unknown tree into a false
                // "none" claim, so surface unavailable.
                if parent == root_pid {
                    return BackgroundProcessState::Unavailable;
                }
                continue;
            };
            for child in children {
                if !visited.insert(child) {
                    continue;
                }
                queue.push_back(child);
                let Some(start_ticks) = crate::mcp::daemon::read_pid_starttime(child) else {
                    // The process exited during the walk. It is no longer a
                    // running job; continue rather than reporting stale data.
                    continue;
                };
                let age_secs =
                    (uptime_secs - start_ticks as f64 / ticks_per_second as f64).max(0.0) as u64;
                let command = process_command_name(child);
                if !is_cas_sidecar(&command, child) {
                    processes.push(BackgroundProcess { command, age_secs });
                }
            }
        }
        processes.sort_by(|a, b| a.command.cmp(&b.command).then(a.age_secs.cmp(&b.age_secs)));
        BackgroundProcessState::Available(processes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root_pid;
        BackgroundProcessState::Unavailable
    }
}

/// Discover immediate children spawned from *any* thread of `parent`.
///
/// `/proc/<pid>/task/<tid>/children` is per-thread, so checking only the
/// process leader misses a child spawned from an executor/test thread. The
/// PPID scan is intentionally retained as a fallback: some procfs mounts do
/// not expose `children`, while field 4 of `/proc/<pid>/stat` remains the
/// kernel's authoritative parent relation.
#[cfg(target_os = "linux")]
fn descendant_pids(parent: u32) -> Option<Vec<u32>> {
    let task_dir = std::fs::read_dir(format!("/proc/{parent}/task")).ok()?;
    let mut children = HashSet::new();
    for task in task_dir.flatten() {
        let task_id = task.file_name();
        let Some(task_id) = task_id.to_str() else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(format!("/proc/{parent}/task/{task_id}/children"))
        else {
            continue;
        };
        children.extend(
            raw.split_whitespace()
                .filter_map(|pid| pid.parse::<u32>().ok()),
        );
    }

    // Do not depend on CONFIG_PROC_CHILDREN: scan the visible process table as
    // well, which also catches a child before its spawning thread's children
    // file is published.
    let proc_entries = std::fs::read_dir("/proc").ok()?;
    for entry in proc_entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if parent_pid_from_stat(&stat) == Some(parent) {
            children.insert(pid);
        }
    }
    Some(children.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn parent_pid_from_stat(raw: &str) -> Option<u32> {
    let tail = raw.get(raw.rfind(')')? + 1..)?.trim_start();
    // Fields after `comm`: state (3), ppid (4), ...
    tail.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn proc_uptime_secs() -> Option<f64> {
    std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn process_command_name(pid: u32) -> String {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok();
    cmdline
        .as_deref()
        .and_then(|raw| raw.split(|b| *b == 0).find(|part| !part.is_empty()))
        .and_then(|arg0| std::str::from_utf8(arg0).ok())
        .and_then(|arg0| Path::new(arg0).file_name().and_then(|name| name.to_str()))
        .map(str::to_owned)
        .or_else(|| {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .unwrap_or_else(|| "<command unavailable>".to_string())
}

/// Persistent MCP/harness sidecars are inherited by every worker; treating
/// them as jobs would suppress stall detection forever. They are not user
/// background commands, unlike the cargo/rustc descendants this feature
/// reports. Keep this allowlist tied to their command-line identity rather
/// than filtering every `node`/`npm` process a worker might intentionally run.
#[cfg(target_os = "linux")]
fn is_cas_sidecar(command: &str, pid: u32) -> bool {
    let Some(raw) = std::fs::read(format!("/proc/{pid}/cmdline")).ok() else {
        return false;
    };
    is_known_sidecar_commandline(command, &raw)
}

#[cfg(target_os = "linux")]
fn is_known_sidecar_commandline(command: &str, raw: &[u8]) -> bool {
    if (command == "cas" || command.starts_with("cas-"))
        && raw.split(|byte| *byte == 0).any(|part| part == b"serve")
    {
        return true;
    }
    let commandline = String::from_utf8_lossy(raw).to_ascii_lowercase();
    [
        "@playwright/mcp",
        "playwright-mcp",
        ".playwright-mcp-profile",
        "@neondatabase/mcp-server-neon",
        "mcp-server-neon",
    ]
    .iter()
    .any(|marker| commandline.contains(marker))
}

/// Sum of user+system jiffies from `/proc/<pid>/stat` (fields 14 and 15
/// after the parenthesized `comm`). `None` when unreadable or non-Linux.
pub(crate) fn process_cpu_ticks(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let last_paren = raw.rfind(')')?;
        let tail = raw.get(last_paren + 1..)?;
        // fields after comm: state(3) ppid(4) ... utime(14) stime(15)
        // index 0 of tail split is state → utime is index 11, stime index 12
        let mut parts = tail.split_whitespace();
        let utime: u64 = parts.nth(11)?.parse().ok()?;
        let stime: u64 = parts.next()?.parse().ok()?;
        Some(utime.saturating_add(stime))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// True when `pid` consumed CPU between two short samples — the mid-
/// inference signal for codex workers whose rollout path is unresolved
/// (cas-c655). 50 ms ceiling keeps `is-wedged` snappy.
pub(crate) fn process_cpu_busy(pid: u32) -> bool {
    let Some(t1) = process_cpu_ticks(pid) else {
        return false;
    };
    std::thread::sleep(Duration::from_millis(50));
    let Some(t2) = process_cpu_ticks(pid) else {
        return false;
    };
    t2 > t1
}

/// Convert `pid` to its process GROUP LEADER's pid via `getpgid()` (cas-a91b).
/// `find_worker_pid`'s environ-based fallback (Codex workers) can still
/// resolve a descendant rather than the actual session leader, since
/// `CAS_AGENT_NAME` is inherited by every child process. Converting through
/// the kernel's own process-group bookkeeping — rather than assuming the
/// resolved pid IS the pgid — is what makes `killpg` safe to call on it:
/// descendants stay in their parent's process group unless they explicitly
/// detach (`setsid`/`setpgid`), so `getpgid(descendant_pid)` correctly
/// returns the leader's pid. Returns `None` if the process is already gone
/// (`getpgid` fails, e.g. ESRCH) — callers fall back to the original pid,
/// which the subsequent liveness/kill checks handle as "already dead".
fn resolve_group_leader_pid(pid: u32) -> Option<u32> {
    let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
    if pgid < 0 { None } else { Some(pgid as u32) }
}

/// Pick which pid [`execute_kill`] targets: a live process-table match (by
/// agent-name/environ) always wins over the tracked agent-store pid, since
/// it's a direct identity proof rather than a value that might have been
/// clobbered (cas-f781). Falls back to the tracked pid when no live match
/// is found (offline host, non-Linux, or an unrecognized worker CLI).
pub(crate) fn pick_kill_pid(tracked_pid: Option<u32>, resolved_pid: Option<u32>) -> Option<u32> {
    resolved_pid.or(tracked_pid)
}

/// Decide whether `execute_kill` should proceed to reset the worker's
/// task leases, given the kill verdict and (for the `Go` case) whether
/// death was actually confirmed after the SIGKILL was delivered. cas-f781
/// AC b: a still-alive process — whether because the kill was refused or
/// because it demonstrably survived the signal — must never have its lease
/// reset out from under it.
pub(crate) fn decide_post_kill_action(
    verdict: &KillVerdict,
    death_confirmed_after_kill: bool,
) -> bool {
    match verdict {
        KillVerdict::AlreadyDead => true,
        KillVerdict::Go => death_confirmed_after_kill,
        KillVerdict::RefuseFingerprintMismatch | KillVerdict::RefuseNoFingerprint => false,
    }
}

/// Parse the process `state` (field 3) out of a raw `/proc/<pid>/stat` line,
/// `true` iff it's `Z` (zombie). Same comm-parsing caveat as
/// `daemon::parse_starttime_from_stat` (`comm` is parenthesized and may
/// itself contain spaces/parens) — split on the LAST `)` before reading
/// fields from the tail. Field 3 is the first field after the parens.
#[cfg(target_os = "linux")]
fn is_zombie_state(raw: &str) -> bool {
    let Some(last_paren) = raw.rfind(')') else {
        return false;
    };
    let Some(tail) = raw.get(last_paren + 1..) else {
        return false;
    };
    tail.trim_start().split_whitespace().next() == Some("Z")
}

/// Whether `pid` is currently a zombie (exited but not yet reaped by its
/// parent). Linux-only (`/proc`); non-Linux always reports `false` — see
/// `verify_death` doc for why that's the safe default there.
#[cfg(target_os = "linux")]
fn pid_is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .is_some_and(|raw| is_zombie_state(&raw))
}

#[cfg(not(target_os = "linux"))]
fn pid_is_zombie(_pid: u32) -> bool {
    false
}

/// Poll pid liveness briefly after SIGKILL. The signal-delivery syscall
/// returning success is not proof of death — the kernel needs a scheduling
/// tick to actually reap the process — and cas-f781 AC b requires the lease
/// reset to wait for genuinely-confirmed death rather than assume the kill
/// worked. 10 x 20ms = 200ms ceiling: generous for an already-signalled
/// process while keeping `cas factory kill` responsive.
///
/// cas-a91b: a killed process GROUP LEADER becomes a zombie under its
/// original parent (typically the daemon that spawned it, not this `cas
/// factory kill` invocation) — `pid_alive` (`kill(pid, 0)`) reports a zombie
/// as alive, since its `/proc` entry still exists until reaped. Without the
/// zombie check, `verify_death` would time out and return `false` for a
/// worker that is, for all practical purposes, dead — leaving its task
/// stuck InProgress with no way to reclaim it short of a manual reset.
fn verify_death(pid: u32) -> bool {
    for _ in 0..10 {
        if !crate::mcp::daemon::pid_alive(pid) || pid_is_zombie(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !crate::mcp::daemon::pid_alive(pid) || pid_is_zombie(pid)
}

/// `cas factory kill <worker>`: SIGKILL the worker process and release any
/// active Cassy lease. Idempotent — already-dead worker still runs the cleanup.
///
/// PID-recycling guard (cas-4513 adversarial P0): before delivering SIGKILL,
/// we verify the agent's stored `pid_starttime` fingerprint matches the
/// process currently at that PID. On a busy host the kernel can recycle a
/// PID between the agent row being written and `kill` being called;
/// without the fingerprint guard we could SIGKILL an unrelated process.
/// When the fingerprint check fails, we refuse unless `--force` is set.
/// Legacy agents without a stored fingerprint also require `--force`.
///
/// Process resolution (cas-f781 P0): the agent store's `pid` column is not
/// trusted blindly — it can be overwritten by an unrelated process's
/// self-registration (an MCP-server child stomping the real `claude
/// --agent-name <worker>` pid with its own). Before falling back to that
/// tracked pid, we scan the live process table for a process whose own
/// argv/environ identifies it as `worker` and prefer that instead
/// ([`pick_kill_pid`]). The resolved target is killed via the process
/// GROUP (`killpg`, see `send_sigkill`), not a single pid, since workers are
/// spawned as session leaders and may have forked children of their own.
///
/// Lease reset only fires after death is independently confirmed
/// ([`decide_post_kill_action`] + [`verify_death`]) — a kill that was
/// refused (fingerprint mismatch / no fingerprint) or that demonstrably
/// didn't take never resets the task lease out from under a still-running
/// worker.
pub(crate) fn execute_kill(cas_root: Option<&Path>, worker: &str, force: bool) -> Result<()> {
    let cas_root =
        cas_root.ok_or_else(|| anyhow!("--cas-root required or run from a Cassy project"))?;
    let w = resolve_worker(cas_root, worker)?;
    let mut summary = Vec::<String>::new();

    let resolved_pid = find_worker_pid(&RealProcessTable, &w.name);
    if let (Some(tracked), Some(resolved)) = (w.pid, resolved_pid) {
        if tracked != resolved {
            summary.push(format!(
                "process-table scan resolved a live process for `{}` at pid {resolved} \
                 (agent-name match) — overriding stale tracked pid {tracked}",
                w.name
            ));
        }
    }
    let kill_pid = pick_kill_pid(w.pid, resolved_pid);
    let scan_confirmed = resolved_pid.is_some() && kill_pid == resolved_pid;

    // Inner scope so the SqliteAgentStore / SqliteTaskStore connections
    // opened by `reset_worker_tasks` drop (and any WAL checkpoints fire)
    // BEFORE we print the summary. cas-4513 adversarial P2.
    let death_confirmed = {
        match kill_pid {
            Some(pid) => {
                // A scan-resolved pid is already authoritatively identified
                // by its own live argv/environ — the starttime fingerprint
                // gate exists to guard a *tracked* pid that might describe
                // the wrong (recycled) process, which doesn't apply here.
                let verdict = if scan_confirmed {
                    if crate::mcp::daemon::pid_alive(pid) {
                        KillVerdict::Go
                    } else {
                        KillVerdict::AlreadyDead
                    }
                } else {
                    kill_verdict(pid, w.pid_starttime, force)
                };
                let death_after_attempt = match &verdict {
                    KillVerdict::Go => {
                        // cas-a91b: convert to the actual process GROUP
                        // LEADER before signaling — `pid` may be a descendant
                        // that inherited CAS_AGENT_NAME in its environ
                        // (find_worker_pid's Codex fallback), not the leader
                        // itself. `killpg`/`verify_death` only make sense
                        // against the real pgid; falling back to the raw
                        // `pid` when the process just vanished mid-resolve is
                        // fine — the ESRCH/pid_alive checks downstream still
                        // handle that safely.
                        let group_pid = resolve_group_leader_pid(pid).unwrap_or(pid);
                        match send_sigkill(group_pid) {
                            Ok(()) => {
                                summary.push(format!(
                                    "SIGKILL delivered to process group {group_pid}"
                                ));
                                verify_death(group_pid)
                            }
                            Err(e) => {
                                summary.push(format!("SIGKILL failed for pid {group_pid}: {e}"));
                                // cas-a91b: do NOT fall through to verify_death
                                // on a failed/refused kill — a failure here
                                // must never be treated as "confirmed dead".
                                false
                            }
                        }
                    }
                    KillVerdict::AlreadyDead => {
                        summary.push(format!("pid {pid} already dead — skipping SIGKILL"));
                        true
                    }
                    KillVerdict::RefuseFingerprintMismatch => {
                        summary.push(format!(
                            "pid {pid} SKIPPED: starttime fingerprint mismatch (PID recycled). Pass --force to override."
                        ));
                        false
                    }
                    KillVerdict::RefuseNoFingerprint => {
                        summary.push(format!(
                            "pid {pid} SKIPPED: no starttime fingerprint recorded (legacy agent). Pass --force to override."
                        ));
                        false
                    }
                };
                let reset_ok = decide_post_kill_action(&verdict, death_after_attempt);
                if !reset_ok {
                    summary.push(format!(
                        "death not verified for pid {pid} — lease NOT reset (worker may still be running)"
                    ));
                }
                reset_ok
            }
            None => {
                summary.push(
                    "worker has no PID recorded and no live process resolved by agent-name — treating as dead"
                        .into(),
                );
                true
            }
        }
    };

    if death_confirmed {
        // Release leases + reset task status to Open. cas-4513 correctness P2
        // flagged that just releasing the lease (like the pre-fix code did)
        // leaves tasks stuck at InProgress with no assignee, so a fresh worker
        // can never claim them. Match the MCP `cas_task_reset` semantics:
        // release lease + status=Open + clear assignee, covers both
        // InProgress and Blocked task states (adversarial P2).
        match reset_worker_tasks(cas_root, &w.name) {
            Ok(n) if n > 0 => summary.push(format!(
                "reset {n} task(s) held by {}: released lease + status→Open + cleared assignee",
                w.name
            )),
            Ok(_) => summary.push("no active leases to release".into()),
            Err(e) => summary.push(format!("task reset failed: {e}")),
        }
    } else {
        summary.push(format!(
            "skipping lease reset for `{}` — worker death not confirmed",
            w.name
        ));
    }

    println!("kill-worker `{}` completed:", w.name);
    for line in summary {
        println!("  - {line}");
    }
    Ok(())
}

/// Decision for the SIGKILL stage of `execute_kill`, separated so the
/// PID-recycling guard logic is testable without real processes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KillVerdict {
    /// PID is alive AND fingerprint matches (or force override).
    Go,
    /// PID is already gone — cleanup still runs, kill is a no-op.
    AlreadyDead,
    /// PID is alive but fingerprint mismatch — refuse unless forced.
    RefuseFingerprintMismatch,
    /// PID is alive, no fingerprint stored (legacy agent) — refuse
    /// unless forced. Preserves PID-recycling safety for registrations
    /// predating cas-ea46.
    RefuseNoFingerprint,
}

fn kill_verdict(pid: u32, expected_starttime: Option<u64>, force: bool) -> KillVerdict {
    if !crate::mcp::daemon::pid_alive(pid) {
        return KillVerdict::AlreadyDead;
    }
    if force {
        return KillVerdict::Go;
    }
    match expected_starttime {
        None => KillVerdict::RefuseNoFingerprint,
        Some(expected) => {
            if crate::mcp::daemon::pid_matches_fingerprint(pid, expected) {
                KillVerdict::Go
            } else {
                KillVerdict::RefuseFingerprintMismatch
            }
        }
    }
}

fn send_sigkill(pgid: u32) -> Result<()> {
    // cas-f781: kill the process GROUP, not just the single recorded pid.
    // Workers are spawned as session leaders (portable_pty calls setsid()
    // before exec), so pid == pgid for the actual leader — killpg here also
    // reaps any children the worker forked (e.g. an in-flight tool
    // subprocess), where a bare `kill(pid)` would leave those running.
    // `pgid` must already be a real process-group id by the time this is
    // called — callers convert via `resolve_group_leader_pid` first
    // (cas-a91b), since a raw resolved pid can be a descendant rather than
    // the leader (see `find_worker_pid`'s environ-fallback ambiguity).
    // SAFETY: libc::killpg with SIGKILL has no side effects on this process.
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    let errno = std::io::Error::last_os_error();
    if errno.raw_os_error() == Some(libc::ESRCH) {
        // cas-a91b adversarial P1: ESRCH means "no process group with this
        // id exists" — but that's only trustworthy as "the worker's group is
        // dead" if `pgid` doesn't independently resolve to a still-alive
        // process. If it does, we passed the wrong number (e.g. a
        // descendant's raw pid that was never actually a valid pgid) and
        // silently treating this as success would let a live worker's task
        // lease get reset out from under it — the exact destructive bug this
        // task exists to close. Refuse instead of guessing.
        if crate::mcp::daemon::pid_alive(pgid) {
            bail!(
                "killpg({pgid}) returned ESRCH but pid {pgid} is still alive — refusing to \
                 treat this as a successful kill (the resolved target was not a valid process \
                 group; the worker may still be running)"
            );
        }
        return Ok(());
    }
    Err(errno.into())
}

/// Fully reset every active task held by `worker_name`: release lease,
/// force status to Open, clear assignee. Matches the MCP `cas_task_reset`
/// semantics (see `task_claiming.rs` cas_task_reset) so a supervisor
/// running `cas factory kill` doesn't have to chase up with a second
/// `action=reset` per task to make them claimable again. Covers both
/// `InProgress` and `Blocked` assignment states (cas-4513 adversarial P2).
fn reset_worker_tasks(cas_root: &Path, worker_name: &str) -> Result<usize> {
    use cas_store::{AgentStore, SqliteAgentStore, SqliteTaskStore, TaskStore};
    use cas_types::TaskStatus;
    let task_store = SqliteTaskStore::open(cas_root).with_context(|| "open task store")?;
    let agent_store = SqliteAgentStore::open(cas_root).with_context(|| "open agent store")?;
    let assigned: Vec<_> = task_store
        .list(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            matches!(t.status, TaskStatus::InProgress | TaskStatus::Blocked)
                && t.assignee.as_deref() == Some(worker_name)
        })
        .collect();
    let mut reset_count = 0usize;
    for mut t in assigned {
        // Same three steps as `cas_task_reset` (task_claiming.rs):
        //   1. Force-release any active lease (idempotent — Ok(false)
        //      when no lease exists).
        //   2. Set task.status = Open.
        //   3. Clear task.assignee.
        let _ = agent_store.release_lease_for_task(&t.id, "Wedged worker recovery");
        t.status = TaskStatus::Open;
        t.assignee = None;
        t.updated_at = chrono::Utc::now();
        if task_store.update(&t).is_ok() {
            reset_count += 1;
        }
    }
    Ok(reset_count)
}

fn read_last_lines(path: &Path, tail: usize) -> Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    // cas-4513 correctness P2: delegates to the shared helper which
    // guards `tail == 0` against the unbounded-growth bug (empty ring
    // buffer + push_back would retain the entire file).
    Ok(collect_tail_lines(f, tail))
}

fn format_state_human(state: &WorkerLivenessState, ev: &WorkerEvidence) -> String {
    let mut s = format!("state: {}\n", state.label());
    // cas-4513 maintainability P3: render pid as a bare integer so
    // `cas factory is-wedged | grep pid | awk '{print $2}' | xargs kill`
    // actually works — Rust's `{:?}` would print `Some(4242)`.
    let pid_str = ev
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "<none>".into());
    s.push_str(&format!("  pid: {pid_str} (alive: {})\n", ev.pid_alive));
    if let Some(ref p) = ev.transcript_path {
        s.push_str(&format!("  transcript: {}\n", p.display()));
    } else {
        s.push_str("  transcript: <unresolved>\n");
    }
    match ev.transcript_mtime_age_secs {
        Some(age) => s.push_str(&format!("  transcript mtime age: {age}s\n")),
        None => s.push_str("  transcript mtime age: <unknown>\n"),
    }
    s.push_str(&format!(
        "  crash signature match: {}\n",
        ev.crash_signature_match
    ));
    match ev.worktree_edit_age_secs {
        Some(age) => s.push_str(&format!("  worktree recent-edit age: {age}s\n")),
        None => s.push_str("  worktree recent-edit age: <unknown>\n"),
    }
    s.push_str(&format!("  session: {}\n", ev.session_id));
    s.push_str(&format!(
        "  in-flight tool call: {}\n",
        ev.in_flight_tool_call
    ));
    match &ev.background_processes {
        BackgroundProcessState::Available(processes) if processes.is_empty() => {
            s.push_str("  background processes: none\n");
        }
        BackgroundProcessState::Available(processes) => {
            let detail = processes
                .iter()
                .map(|process| format!("{} ({}s)", process.command, process.age_secs))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!("  background processes: {detail}\n"));
        }
        BackgroundProcessState::Unavailable => {
            s.push_str("  background-process state unavailable\n");
        }
    }
    if let Some(pending) = &ev.pending_permission {
        let request = pending
            .request_id
            .as_deref()
            .map(|id| format!(" request {id}"))
            .unwrap_or_default();
        s.push_str(&format!(
            "  pending permission: {}{} ({}s; command: {})\n",
            pending.tool_name, request, pending.age_secs, pending.command_excerpt
        ));
        if matches!(state, WorkerLivenessState::ApprovalHang) {
            s.push_str(
                "  awaiting leader approval: no active child process; inspect or resolve the team inbox request\n",
            );
        } else {
            s.push_str("  awaiting leader approval: request is still pending\n");
        }
    }
    s
}

fn format_state_json(state: &WorkerLivenessState, ev: &WorkerEvidence) -> String {
    // cas-4513 adversarial P2: use serde_json so backslashes, control
    // characters, and any non-ASCII session_id / path bytes are escaped
    // correctly. The prior hand-rolled escape only handled `"` and
    // produced malformed JSON for paths or session ids containing
    // backslashes or control chars.
    let transcript = ev
        .transcript_path
        .as_ref()
        .map(|p| serde_json::Value::String(p.display().to_string()))
        .unwrap_or(serde_json::Value::Null);
    let body = serde_json::json!({
        "state": state.label(),
        "pid": ev.pid,
        "pid_alive": ev.pid_alive,
        "transcript_path": transcript,
        "transcript_mtime_age_secs": ev.transcript_mtime_age_secs,
        "crash_signature_match": ev.crash_signature_match,
        "worktree_edit_age_secs": ev.worktree_edit_age_secs,
        "session_id": ev.session_id,
        "in_flight_tool_call": ev.in_flight_tool_call,
        "approval_status": ev.pending_permission.as_ref().map(|_| {
            if matches!(state, WorkerLivenessState::ApprovalHang) {
                "awaiting leader approval"
            } else {
                "leader approval pending"
            }
        }),
        "pending_permission": ev.pending_permission.as_ref().map(|pending| serde_json::json!({
            "request_id": pending.request_id,
            "tool_name": pending.tool_name,
            "command_excerpt": pending.command_excerpt,
            "age_secs": pending.age_secs,
        })),
        "background_processes": match &ev.background_processes {
            BackgroundProcessState::Available(processes) => serde_json::Value::Array(processes.iter().map(|process| serde_json::json!({
                "command": process.command,
                "age_secs": process.age_secs,
            })).collect()),
            BackgroundProcessState::Unavailable => serde_json::Value::Null,
        },
    });
    body.to_string()
}

fn format_state_json_with_opencode(
    state: &WorkerLivenessState,
    ev: &WorkerEvidence,
    observation: &opencode_liveness::OpenCodeObservation,
) -> String {
    let mut body: serde_json::Value =
        serde_json::from_str(&format_state_json(state, ev)).expect("state JSON is valid");
    if let serde_json::Value::Object(fields) = &mut body {
        fields.insert(
            "opencode_session_id".to_string(),
            serde_json::Value::String(
                opencode_liveness::mapped_session_id(observation)
                    .unwrap_or_default()
                    .to_string(),
            ),
        );
        fields.insert(
            "opencode_liveness".to_string(),
            serde_json::Value::String(
                opencode_liveness::verdict_label(observation.verdict).to_string(),
            ),
        );
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn opencode_liveness_overlay_covers_busy_idle_crash_and_fallback() {
        assert_eq!(
            overlay_opencode_liveness(
                WorkerLivenessState::Unverified,
                cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Busy),
                true,
            ),
            WorkerLivenessState::Alive
        );
        assert_eq!(
            overlay_opencode_liveness(
                WorkerLivenessState::Unverified,
                cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Idle),
                true,
            ),
            WorkerLivenessState::Alive
        );
        assert_eq!(
            overlay_opencode_liveness(
                WorkerLivenessState::Alive,
                cas_mux::OpenCodeLivenessVerdict::Signal(cas_mux::OpenCodeLiveness::Error),
                true,
            ),
            WorkerLivenessState::Wedged
        );
        assert_eq!(
            overlay_opencode_liveness(
                WorkerLivenessState::Unverified,
                cas_mux::OpenCodeLivenessVerdict::ProcessAliveFallback,
                true,
            ),
            WorkerLivenessState::Alive
        );
        assert_eq!(
            overlay_opencode_liveness(
                WorkerLivenessState::Unverified,
                cas_mux::OpenCodeLivenessVerdict::NotObserved,
                false,
            ),
            WorkerLivenessState::Unverified
        );
    }

    #[test]
    fn classify_dead_when_pid_gone_and_second_signal_corroborates() {
        // cas-f781 AC c: Dead requires TWO independent signals to agree —
        // pid gone AND (transcript stale AND worktree not recently edited).
        for sig in [true, false] {
            let got = classify_from_evidence(
                false,
                Some(Duration::from_secs(5 * 60)),
                sig,
                None,
                false,
                TRANSCRIPT_FRESH_WINDOW,
                false,
            );
            assert_eq!(got, WorkerLivenessState::Dead, "sig={sig}");
        }
    }

    #[test]
    fn classify_unverified_when_pid_gone_but_transcript_still_fresh() {
        // cas-f781 core fix: a pid-only "gone" reading contradicted by a
        // transcript still being written in the last minute must NOT be
        // reported as Dead — that combination is exactly the stale/wrong
        // tracked-pid bug (the real worker is still alive and writing).
        // Report Unverified so an operator investigates before a caller
        // (e.g. a supervisor auto-reset) treats it as ground truth.
        for sig in [true, false] {
            let got = classify_from_evidence(
                false,
                Some(Duration::from_secs(5)),
                sig,
                None,
                false,
                TRANSCRIPT_FRESH_WINDOW,
                false,
            );
            assert_eq!(got, WorkerLivenessState::Unverified, "sig={sig}");
        }
    }

    #[test]
    fn classify_unverified_when_pid_gone_but_worktree_recently_edited() {
        // Same contradiction, corroborated by worktree activity instead of
        // transcript mtime — matches the bug report's concrete repro
        // ("fresh worktree edits, 20s-old transcript").
        let got = classify_from_evidence(
            false,
            Some(Duration::from_secs(5 * 60)),
            false,
            Some(Duration::from_secs(20)),
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Unverified);
    }

    #[test]
    fn classify_dead_when_no_corroborating_signals_available_at_all() {
        // No transcript resolved, no worktree resolved, pid gone: nothing
        // contradicts "dead", so Dead still fires — matches the
        // no-pid-registered case (classify_worker_no_pid_short_circuits_to_dead).
        let got = classify_from_evidence(
            false,
            None,
            true,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Dead);
    }

    #[test]
    fn classify_wedged_when_alive_fresh_and_signature_matches() {
        let got = classify_from_evidence(
            true,
            Some(Duration::from_secs(5)),
            true,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Wedged);
    }

    #[test]
    fn classify_alive_when_fresh_and_no_signature() {
        let got = classify_from_evidence(
            true,
            Some(Duration::from_secs(5)),
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Alive);
    }

    #[test]
    fn classify_starved_when_alive_but_stale() {
        // Stale wins over signature: a crashed-but-not-touched-in-5min
        // worker is functionally hung, not wedged — the recovery playbook
        // is the same (SIGKILL + respawn) but the label matters for
        // operator triage.
        // cas-7e85: TRANSCRIPT_FRESH_WINDOW widened 60s -> 3min, so the
        // fixture moved from 120s to 200s to stay past the window.
        for sig in [true, false] {
            let got = classify_from_evidence(
                true,
                Some(Duration::from_secs(200)),
                sig,
                None,
                false,
                TRANSCRIPT_FRESH_WINDOW,
                false,
            );
            assert_eq!(got, WorkerLivenessState::Starved, "sig={sig}");
        }
    }

    #[test]
    fn classify_unverified_when_no_mtime_available() {
        // cas-de95: missing/unresolved transcript is missing evidence, not
        // Starved — even if a crash needle were claimed without a file.
        let got = classify_from_evidence(
            true,
            None,
            true,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Unverified);
        let got_clean = classify_from_evidence(
            true,
            None,
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got_clean, WorkerLivenessState::Unverified);
    }

    /// cas-de95: Claude + Codex with unresolved transcript + recent worktree → Alive.
    #[test]
    fn classify_claude_and_codex_unresolved_with_worktree_are_alive() {
        for window in [TRANSCRIPT_FRESH_WINDOW, CODEX_TRANSCRIPT_FRESH_WINDOW] {
            let got = classify_from_evidence(
                true,
                None,
                false,
                Some(Duration::from_secs(10)),
                false,
                window,
                false,
            );
            assert_eq!(got, WorkerLivenessState::Alive, "window={window:?}");
        }
    }

    /// cas-de95: genuine starvation requires a resolved cold transcript.
    #[test]
    fn classify_genuine_starvation_when_transcript_stale_and_no_activity() {
        let got = classify_from_evidence(
            true,
            Some(Duration::from_secs(10 * 60)),
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Starved);
    }

    /// cas-c655 AC: missing transcript alone must not force Starved when the
    /// resolved process is busy (codex mid-inference).
    #[test]
    fn classify_alive_when_no_transcript_but_process_busy() {
        let got = classify_from_evidence(
            true,
            None,
            false,
            None,
            true,
            CODEX_TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Alive);
    }

    /// cas-c655 AC: fixture codex worker with recent worktree writes → not
    /// starved even with unresolved transcript.
    #[test]
    fn classify_alive_when_no_transcript_but_worktree_recent() {
        let got = classify_from_evidence(
            true,
            None,
            false,
            Some(Duration::from_secs(15)),
            false,
            CODEX_TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Alive);
    }

    /// cas-c655 edge: truly dead process with no corroborating activity
    /// still reports Dead.
    #[test]
    fn classify_dead_when_pid_gone_and_no_activity_codex_window() {
        let got = classify_from_evidence(
            false,
            None,
            false,
            None,
            false,
            CODEX_TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Dead);
    }

    /// cas-c655: a stale-but-within-codex-window transcript is still Alive.
    #[test]
    fn classify_codex_window_keeps_3min_transcript_fresh() {
        // cas-7e85: TRANSCRIPT_FRESH_WINDOW widened from 60s to 3min, so the
        // fixture moved to 4min to keep sitting strictly between the two
        // windows (Claude's widened 3min and Codex's 5min).
        let age = Duration::from_secs(4 * 60);
        assert!(
            age < CODEX_TRANSCRIPT_FRESH_WINDOW && age >= TRANSCRIPT_FRESH_WINDOW,
            "fixture must sit between the claude and codex windows"
        );
        let got = classify_from_evidence(
            true,
            Some(age),
            false,
            None,
            false,
            CODEX_TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(got, WorkerLivenessState::Alive);
        let claude = classify_from_evidence(
            true,
            Some(age),
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
        );
        assert_eq!(claude, WorkerLivenessState::Starved);
    }

    #[test]
    fn activity_fresh_window_is_longer_for_codex() {
        assert_eq!(
            activity_fresh_window(cas_mux::SupervisorCli::Codex),
            CODEX_TRANSCRIPT_FRESH_WINDOW
        );
        assert_eq!(
            activity_fresh_window(cas_mux::SupervisorCli::Claude),
            TRANSCRIPT_FRESH_WINDOW
        );
        assert_eq!(
            activity_fresh_window(cas_mux::SupervisorCli::Grok),
            GROK_TRANSCRIPT_FRESH_WINDOW
        );
    }

    /// cas-7e85: Grok's window must stay at the original 60s, independent
    /// of any future widening of Claude's `TRANSCRIPT_FRESH_WINDOW` — the
    /// two are deliberately separate constants specifically to prevent this
    /// class of accidental coupling.
    #[test]
    fn grok_window_unaffected_by_claude_widening() {
        assert_eq!(GROK_TRANSCRIPT_FRESH_WINDOW, Duration::from_secs(60));
        assert_ne!(GROK_TRANSCRIPT_FRESH_WINDOW, TRANSCRIPT_FRESH_WINDOW);
    }

    #[test]
    fn environ_candidate_score_prefers_codex_over_cas_serve() {
        assert!(
            environ_candidate_score(Some(b"codex\0--yolo\0"))
                > environ_candidate_score(Some(b"cas\0serve\0--foreground\0"))
        );
        assert_eq!(
            environ_candidate_score(Some(b"cas\0serve\0--foreground\0")),
            0
        );
        assert_eq!(environ_candidate_score(Some(b"codex\0--yolo\0")), 100);
    }

    #[test]
    fn classify_state_exit_codes_are_pinned() {
        // cas-4513 AC: supervisor bash scripts branch on exit code.
        assert_eq!(WorkerLivenessState::Alive.exit_code(), 0);
        assert_eq!(WorkerLivenessState::Wedged.exit_code(), 1);
        assert_eq!(WorkerLivenessState::ApprovalHang.exit_code(), 5);
        assert_eq!(WorkerLivenessState::Starved.exit_code(), 2);
        assert_eq!(WorkerLivenessState::Dead.exit_code(), 3);
        assert_eq!(WorkerLivenessState::Unverified.exit_code(), 4);
    }

    #[test]
    fn pending_permission_parser_uses_newest_unread_request_and_excerpt() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-06T18:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let inbox = serde_json::json!([
            {
                "from": "worker",
                "read": false,
                "timestamp": "2026-09-06T17:50:00Z",
                "text": serde_json::json!({
                    "type": "permission_request",
                    "request_id": "old",
                    "tool_name": "Bash",
                    "input": {"command": "echo old"}
                }).to_string()
            },
            {
                "from": "worker",
                "read": false,
                "timestamp": "2026-09-06T17:54:00Z",
                "text": serde_json::json!({
                    "type": "permission_request",
                    "request_id": "new",
                    "tool_name": "Bash",
                    "input": {"command": "python3 - <<'PY'\nrewrite target\nPY"}
                }).to_string()
            },
            {
                "from": "other-worker",
                "read": false,
                "timestamp": "2026-09-06T17:59:00Z",
                "text": serde_json::json!({
                    "type": "permission_request",
                    "request_id": "other",
                    "tool_name": "Bash",
                    "input": {"command": "echo other"}
                }).to_string()
            },
            {
                "from": "worker",
                "read": true,
                "timestamp": "2026-09-06T17:59:30Z",
                "text": serde_json::json!({"type": "permission_request"}).to_string()
            }
        ]);
        let pending = pending_permission_from_inbox(&inbox, "worker", now)
            .expect("unread worker permission request");
        assert_eq!(pending.request_id.as_deref(), Some("new"));
        assert_eq!(pending.tool_name, "Bash");
        assert_eq!(pending.age_secs, 360);
        assert_eq!(
            pending.command_excerpt,
            "python3 - <<'PY' rewrite target PY"
        );
    }

    #[test]
    fn pending_permission_requires_live_worker_and_empty_process_tree() {
        let pending = PendingPermission {
            request_id: Some("perm".to_string()),
            tool_name: "Bash".to_string(),
            command_excerpt: "rm -rf $B/$h/$sk".to_string(),
            age_secs: LEADER_APPROVAL_PENDING_THRESHOLD_SECS,
        };
        assert!(is_leader_approval_hang(
            true,
            Some(&pending),
            &BackgroundProcessState::Available(vec![])
        ));
        assert!(!is_leader_approval_hang(
            false,
            Some(&pending),
            &BackgroundProcessState::Available(vec![])
        ));
        assert!(!is_leader_approval_hang(
            true,
            Some(&pending),
            &BackgroundProcessState::Available(vec![BackgroundProcess {
                command: "python3".to_string(),
                age_secs: 1,
            }])
        ));
        assert!(!is_leader_approval_hang(
            true,
            Some(&pending),
            &BackgroundProcessState::Unavailable
        ));
    }

    #[test]
    fn classification_surfaces_pending_permission_without_transcript_tool_use() {
        let pending = PendingPermission {
            request_id: Some("perm".to_string()),
            tool_name: "Bash".to_string(),
            command_excerpt: "python3 -<<'PY' rewrite.html PY".to_string(),
            age_secs: LEADER_APPROVAL_PENDING_THRESHOLD_SECS,
        };
        let (state, evidence) = classify_worker_with_pending(
            Some(std::process::id()),
            None,
            None,
            "session",
            cas_mux::SupervisorCli::Claude,
            |_| true,
            |_| None,
            |_| false,
            Some(pending.clone()),
        );
        assert_eq!(state, WorkerLivenessState::ApprovalHang);
        assert_eq!(evidence.pending_permission, Some(pending));
        assert!(!evidence.in_flight_tool_call);
        let human = format_state_human(&state, &evidence);
        assert!(human.contains("awaiting leader approval"));
        assert!(human.contains("python3 -<<'PY' rewrite.html PY"));
        let json: serde_json::Value =
            serde_json::from_str(&format_state_json(&state, &evidence)).expect("valid JSON");
        assert_eq!(json["approval_status"], "awaiting leader approval");
        assert_eq!(json["pending_permission"]["tool_name"], "Bash");
        assert!(
            json["pending_permission"]["command_excerpt"]
                .as_str()
                .is_some_and(|command| command.contains("rewrite.html"))
        );
    }

    #[test]
    fn worktree_recent_edit_age_detects_dirty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(repo)
            .status()
            .expect("git init should run");
        assert!(status.success());
        std::fs::write(repo.join("touched.txt"), "hello").unwrap();
        let age = worktree_recent_edit_age(repo).expect("dirty file should be detected");
        assert!(
            age < Duration::from_secs(5),
            "expected fresh edit age, got {age:?}"
        );
    }

    #[test]
    fn worktree_recent_edit_age_none_when_not_a_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(worktree_recent_edit_age(tmp.path()).is_none());
    }

    // -------------------------------------------------------------------
    // cas-058f (EPIC cas-8888 Phase 4): Grok-aware liveness signals.
    // -------------------------------------------------------------------

    #[test]
    fn grok_activity_age_prefers_fresher_signals_json_when_updates_is_staler() {
        let tmp = tempfile::tempdir().unwrap();
        let updates = tmp.path().join("updates.jsonl");
        let signals = tmp.path().join("signals.json");
        // updates.jsonl written first (stale-ish), signals.json touched
        // just now — the fresher signals.json age must win.
        std::fs::write(&updates, b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&signals, b"{\"turn\":3}").unwrap();

        let age = grok_activity_age(&updates).expect("signals.json should be found");
        assert!(
            age < Duration::from_millis(40),
            "expected the fresher signals.json mtime to win, got {age:?}"
        );
    }

    /// cas-921f P2: the inverse ordering. Grok appends to `updates.jsonl`
    /// continuously mid-turn but may only rewrite `signals.json` at turn
    /// boundaries — an actively-working worker can have a FRESH
    /// updates.jsonl and a STALE signals.json at the same instant. A strict
    /// "prefer signals.json, fall back only if absent" rule would return the
    /// stale age here and misclassify the worker Starved; taking the min of
    /// the two must return the fresher updates.jsonl age instead.
    #[test]
    fn grok_activity_age_prefers_fresher_updates_jsonl_when_signals_is_staler() {
        let tmp = tempfile::tempdir().unwrap();
        let updates = tmp.path().join("updates.jsonl");
        let signals = tmp.path().join("signals.json");
        // signals.json written first (stale-ish), updates.jsonl touched
        // just now — the fresher updates.jsonl age must win.
        std::fs::write(&signals, b"{\"turn\":3}").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&updates, b"{}").unwrap();

        let age = grok_activity_age(&updates).expect("updates.jsonl should be found");
        assert!(
            age < Duration::from_millis(40),
            "expected the fresher updates.jsonl mtime to win over a staler \
             signals.json, got {age:?}"
        );
    }

    #[test]
    fn grok_activity_age_falls_back_to_updates_jsonl_when_signals_json_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let updates = tmp.path().join("updates.jsonl");
        std::fs::write(&updates, b"{}").unwrap();
        // No signals.json written alongside it.
        let age = grok_activity_age(&updates).expect("updates.jsonl mtime should be used");
        assert!(age < Duration::from_secs(5));
    }

    #[test]
    fn effective_transcript_age_grok_uses_grok_activity_age() {
        let tmp = tempfile::tempdir().unwrap();
        let updates = tmp.path().join("updates.jsonl");
        std::fs::write(&updates, b"{}").unwrap();
        assert!(effective_transcript_age(&updates, cas_mux::SupervisorCli::Grok).is_some());
    }

    #[test]
    fn effective_transcript_age_claude_and_codex_use_plain_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        std::fs::write(&path, b"{}").unwrap();
        assert!(effective_transcript_age(&path, cas_mux::SupervisorCli::Claude).is_some());
        assert!(effective_transcript_age(&path, cas_mux::SupervisorCli::Codex).is_some());
    }

    #[test]
    fn effective_crash_signature_skips_for_grok_even_when_claude_needle_present() {
        // cas-058f audit: Claude/Bun/React-Ink crash strings don't apply to
        // Grok's UI stack — must never fire for a Grok worker, even if the
        // literal needle text happens to appear in its transcript content.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("updates.jsonl");
        std::fs::write(&path, b"<Box> can't be nested inside <Text>").unwrap();
        assert!(
            !effective_crash_signature(&path, cas_mux::SupervisorCli::Grok),
            "crash-signature detection must be skipped entirely for Grok"
        );
        assert!(
            effective_crash_signature(&path, cas_mux::SupervisorCli::Claude),
            "the same content must still be detected for Claude (sanity check on the fixture)"
        );
    }

    /// End-to-end through the full `classify_worker` orchestrator (not just
    /// the isolated helpers above): a Grok worker whose `updates.jsonl` looks
    /// stale (past `TRANSCRIPT_FRESH_WINDOW`) but whose sibling
    /// `signals.json` was just touched — a mid-think turn updating token
    /// counters without a new JSONL line yet — must classify `Alive`, not
    /// `Starved`. This is the AC's concrete "a grok worker mid-think is not
    /// false-flagged Dead"-adjacent scenario (Starved is the more precise
    /// wrong verdict a stale-mtime-only read would produce here; pid_alive
    /// is true throughout, so `Dead` was never in play — the risk this test
    /// pins is the *next* rung down).
    #[test]
    fn classify_worker_grok_prefers_fresh_signals_json_over_stale_updates_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let updates = tmp.path().join("updates.jsonl");
        let signals = tmp.path().join("signals.json");
        std::fs::write(&updates, b"{}").unwrap();
        // Backdate updates.jsonl past the freshness window.
        let stale_time = std::time::SystemTime::now() - Duration::from_secs(120);
        let file = std::fs::File::open(&updates).unwrap();
        file.set_modified(stale_time).unwrap();
        // signals.json touched just now.
        std::fs::write(&signals, b"{\"turn\":7}").unwrap();

        let pid_probe = |_: u32| true;
        let worktree_probe = |_: &Path| None::<Duration>;
        let (state, ev) = classify_worker(
            Some(4242),
            Some(&updates),
            None,
            "grok-ses",
            cas_mux::SupervisorCli::Grok,
            pid_probe,
            worktree_probe,
            |_: u32| false,
        );
        assert_eq!(
            state,
            WorkerLivenessState::Alive,
            "signals.json freshness must win over stale updates.jsonl mtime: {ev:?}"
        );
    }

    #[test]
    fn crash_signature_matches_bun_root_path() {
        // Evidence from cas-4513 discovery note: `/$bunfs/root` prefix
        // inside a JS stack frame is the strongest single signal.
        let transcript = r#"{"type":"assistant","text":"..."}
{"type":"tool_use","name":"Bash"}
{"error":"at createInstance (/$bunfs/root/src/entrypoints/cli.js:496:249)"}"#;
        assert!(has_crash_signature(
            Cursor::new(transcript),
            CRASH_SIGNATURE_TAIL_LINES
        ));
    }

    #[test]
    fn crash_signature_matches_literal_ink_guard_text() {
        // Supervisor's cas-4513 nit: the literal Ink invariant text is a
        // stronger signal than bundler paths. If this regresses, the whole
        // crash-screen detection weakens to a path-heuristic only.
        let transcript = "normal\n{\"error\":\"<Box> can't be nested inside <Text>\"}\nmore";
        assert!(has_crash_signature(
            Cursor::new(transcript),
            CRASH_SIGNATURE_TAIL_LINES
        ));
    }

    #[test]
    fn crash_signature_matches_ink_createelement() {
        let transcript = "normal line\nanother line\ncreateElement(\"ink-box\", {ref:V})";
        assert!(has_crash_signature(
            Cursor::new(transcript),
            CRASH_SIGNATURE_TAIL_LINES
        ));
    }

    #[test]
    fn crash_signature_no_match_on_clean_transcript() {
        let transcript = r#"{"type":"user","text":"hi"}
{"type":"assistant","text":"hello"}
{"type":"tool_use","name":"Read"}"#;
        assert!(!has_crash_signature(
            Cursor::new(transcript),
            CRASH_SIGNATURE_TAIL_LINES
        ));
    }

    #[test]
    fn crash_signature_ignores_old_lines_outside_tail_window() {
        // cas-4513 scope note: we only look at the LAST N lines. A crash
        // signature buried earlier in a long transcript should NOT fire
        // — the worker recovered from it.
        let mut lines: Vec<String> = vec!["createElement(\"ink-\")".to_string()];
        for i in 0..50 {
            lines.push(format!("{{\"msg\":\"line {i}\"}}"));
        }
        let body = lines.join("\n");
        assert!(!has_crash_signature(Cursor::new(body), 20));
    }

    #[test]
    fn classify_worker_orchestrator_threads_probe_fn() {
        // The orchestrating wrapper must actually call the injectable pid
        // probe (not hardcode a kill(0) call). Use a Cell to observe it.
        let called = std::cell::Cell::new(false);
        let probe = |_: u32| {
            called.set(true);
            true
        };
        let worktree_probe = |_: &Path| None::<Duration>;
        let (state, ev) = classify_worker(
            Some(1234),
            None,
            None,
            "ses",
            cas_mux::SupervisorCli::Claude,
            probe,
            worktree_probe,
            |_: u32| false,
        );
        assert!(called.get(), "probe must be called when pid is Some");
        // cas-de95: no transcript → Unverified (missing evidence), not Starved.
        assert_eq!(state, WorkerLivenessState::Unverified);
        assert_eq!(ev.pid, Some(1234));
        assert!(ev.pid_alive);
        assert!(!ev.crash_signature_match);
        assert_eq!(ev.session_id, "ses");
    }

    #[test]
    fn classify_worker_no_pid_short_circuits_to_dead() {
        let probe = |_: u32| panic!("probe must not be called when pid is None");
        let worktree_probe = |_: &Path| None::<Duration>;
        let (state, ev) = classify_worker(
            None,
            None,
            None,
            "ses",
            cas_mux::SupervisorCli::Claude,
            probe,
            worktree_probe,
            |_: u32| false,
        );
        assert_eq!(state, WorkerLivenessState::Dead);
        assert!(!ev.pid_alive);
    }

    #[test]
    fn classify_worker_threads_worktree_probe_only_when_clone_path_present() {
        // clone_path=None must short-circuit without invoking the probe —
        // mirrors the existing no-pid short-circuit contract for the pid
        // probe. cas-f781.
        let pid_probe = |_: u32| false;
        let worktree_probe = |_: &Path| panic!("worktree probe must not run without a clone_path");
        let (state, ev) = classify_worker(
            None,
            None,
            None,
            "ses",
            cas_mux::SupervisorCli::Claude,
            pid_probe,
            worktree_probe,
            |_: u32| false,
        );
        assert_eq!(state, WorkerLivenessState::Dead);
        assert_eq!(ev.worktree_edit_age_secs, None);
    }

    #[test]
    fn classify_worker_surfaces_worktree_evidence_when_clone_path_present() {
        let pid_probe = |_: u32| false;
        let worktree_probe = |_: &Path| Some(Duration::from_secs(20));
        let (state, ev) = classify_worker(
            None,
            None,
            Some(Path::new("/some/clone/path")),
            "ses",
            cas_mux::SupervisorCli::Claude,
            pid_probe,
            worktree_probe,
            |_: u32| false,
        );
        // pid gone, transcript unresolved (not fresh), but worktree edited
        // 20s ago (fresh) — contradiction → Unverified, not Dead.
        assert_eq!(state, WorkerLivenessState::Unverified);
        assert_eq!(ev.worktree_edit_age_secs, Some(20));
    }

    #[test]
    fn transcript_mtime_age_reads_recent_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fresh.jsonl");
        std::fs::write(&path, b"{}").unwrap();
        let age = transcript_mtime_age(&path).expect("fresh file must have mtime");
        assert!(
            age < Duration::from_secs(5),
            "just-written file should be < 5s old, got {age:?}"
        );
    }

    #[test]
    fn transcript_mtime_age_none_for_missing_file() {
        let missing = Path::new("/tmp/does-not-exist-cas-4513");
        assert!(transcript_mtime_age(missing).is_none());
    }

    #[test]
    fn transcript_has_crash_signature_missing_file_is_false() {
        let missing = Path::new("/tmp/does-not-exist-cas-4513");
        assert!(!transcript_has_crash_signature(missing, 20));
    }

    #[test]
    fn format_state_human_surfaces_session_and_state() {
        let ev = WorkerEvidence {
            pid: Some(4242),
            pid_alive: true,
            transcript_path: Some(PathBuf::from("/p/a.jsonl")),
            transcript_mtime_age_secs: Some(7),
            crash_signature_match: true,
            worktree_edit_age_secs: Some(3),
            session_id: "ses-xyz".to_string(),
            in_flight_tool_call: false,
            background_processes: BackgroundProcessState::Available(vec![]),
            pending_permission: None,
        };
        let out = format_state_human(&WorkerLivenessState::Wedged, &ev);
        assert!(out.contains("state: wedged"));
        assert!(out.contains("session: ses-xyz"));
        // cas-4513 maintainability P3: bare integer, not Debug `Some(4242)`.
        assert!(
            out.contains("pid: 4242"),
            "expected bare integer, got:\n{out}"
        );
        assert!(!out.contains("Some(4242)"));
        assert!(out.contains("transcript: /p/a.jsonl"));
        assert!(out.contains("crash signature match: true"));
        assert!(out.contains("worktree recent-edit age: 3s"));
    }

    #[test]
    fn format_state_human_none_fields_render_placeholders() {
        // cas-4513 testing P3: the None branches for pid, transcript_path,
        // and transcript_mtime_age_secs must produce a legible placeholder
        // rather than nothing / a crash.
        let ev = WorkerEvidence {
            pid: None,
            pid_alive: false,
            transcript_path: None,
            transcript_mtime_age_secs: None,
            crash_signature_match: false,
            worktree_edit_age_secs: None,
            session_id: "ses-abc".to_string(),
            in_flight_tool_call: false,
            background_processes: BackgroundProcessState::Unavailable,
            pending_permission: None,
        };
        let out = format_state_human(&WorkerLivenessState::Dead, &ev);
        assert!(out.contains("pid: <none>"));
        assert!(out.contains("transcript: <unresolved>"));
        assert!(out.contains("transcript mtime age: <unknown>"));
        assert!(out.contains("worktree recent-edit age: <unknown>"));
        assert!(out.contains("session: ses-abc"));
    }

    #[test]
    fn format_state_json_escapes_quotes_and_is_valid() {
        let ev = WorkerEvidence {
            pid: Some(4242),
            pid_alive: true,
            transcript_path: Some(PathBuf::from("/p/with\"quote.jsonl")),
            transcript_mtime_age_secs: None,
            crash_signature_match: false,
            worktree_edit_age_secs: None,
            session_id: "ses\"id".to_string(),
            in_flight_tool_call: false,
            background_processes: BackgroundProcessState::Available(vec![]),
            pending_permission: None,
        };
        let out = format_state_json(&WorkerLivenessState::Alive, &ev);
        // Should be parseable as JSON.
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["state"], "alive");
        assert_eq!(parsed["pid"], 4242);
        assert_eq!(parsed["session_id"], "ses\"id");
        assert_eq!(parsed["transcript_mtime_age_secs"], serde_json::Value::Null);
    }

    #[test]
    fn read_last_lines_returns_at_most_tail_count() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("long.jsonl");
        let body: String = (0..100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, body).unwrap();
        let got = read_last_lines(&path, 5).unwrap();
        assert_eq!(got.len(), 5);
        assert_eq!(got[0], "line 95");
        assert_eq!(got[4], "line 99");
    }

    #[test]
    fn read_last_lines_short_file_returns_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("short.jsonl");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let got = read_last_lines(&path, 100).unwrap();
        assert_eq!(got, vec!["a", "b", "c"]);
    }

    #[test]
    fn read_last_lines_tail_zero_returns_empty_not_unbounded() {
        // cas-4513 correctness P2: `tail = 0` used to grow the ring
        // buffer unboundedly (VecDeque::with_capacity(0) + len==0 guard
        // fires on every push). The shared helper now short-circuits.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("long.jsonl");
        let body: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, body).unwrap();
        let got = read_last_lines(&path, 0).unwrap();
        assert!(got.is_empty(), "tail=0 must return empty, not retain file");
    }

    #[test]
    fn has_crash_signature_tail_zero_is_false() {
        // cas-4513 testing P3: explicit coverage for the 0-line guard.
        let transcript = "<Box> can't be nested inside <Text>";
        assert!(!has_crash_signature(Cursor::new(transcript), 0));
    }

    #[test]
    fn collect_tail_lines_returns_bounded_window() {
        let body: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let got = collect_tail_lines(Cursor::new(body), 3);
        assert_eq!(got, vec!["line 47", "line 48", "line 49"]);
    }

    // --- cas-7e85: has_in_flight_tool_call -----------------------------------

    /// Real Claude Code transcript shape: a `sleep 280` Bash call requested
    /// (matches the actual `happy-sparrow-33`/`patient-cobra-45` repro
    /// specimens) with no `tool_result` line following it yet.
    #[test]
    fn claude_in_flight_tool_call_detected_when_unresolved() {
        let transcript = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Running the gate in the background."},{"type":"tool_use","id":"toolu_01abc","name":"Bash","input":{"command":"sleep 280"}}]}}"#,
            "\n",
        );
        assert!(has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Claude,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Same shape, but the `tool_result` has already come back — no call is
    /// outstanding.
    #[test]
    fn claude_no_in_flight_call_when_tool_result_present() {
        let transcript = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_01abc","name":"Bash","input":{"command":"sleep 280"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01abc","content":"done"}]}}"#,
            "\n",
        );
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Claude,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// A transcript with no tool calls at all (plain assistant text) must
    /// not be misread as having an outstanding call.
    #[test]
    fn claude_no_in_flight_call_on_plain_text_transcript() {
        let transcript = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"All done."}]}}"#,
            "\n",
        );
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Claude,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Malformed / non-JSON lines (partial writes) must not crash the parser
    /// or be misread as evidence either way.
    #[test]
    fn claude_malformed_lines_are_skipped_not_treated_as_in_flight() {
        let transcript = "not json at all\n{\"partial\": tr\n";
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Claude,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Codex rollout shape (best-effort schema, see `has_in_flight_tool_call`
    /// doc): a `function_call` with no matching `function_call_output`.
    #[test]
    fn codex_in_flight_tool_call_detected_when_unresolved() {
        let transcript = concat!(
            r#"{"type":"session_meta","payload":{"cwd":"/tmp"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","call_id":"call_01","name":"shell","arguments":"{\"command\":\"sleep 280\"}"}}"#,
            "\n",
        );
        assert!(has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Codex,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Codex: the matching `function_call_output` resolves the call.
    #[test]
    fn codex_no_in_flight_call_when_output_present() {
        let transcript = concat!(
            r#"{"type":"response_item","payload":{"type":"function_call","call_id":"call_01","name":"shell","arguments":"{}"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_01","output":"ok"}}"#,
            "\n",
        );
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Codex,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Codex: the `local_shell_call` / `local_shell_call_output` variant
    /// (sandboxed shell tool) resolves the same way.
    #[test]
    fn codex_local_shell_call_pairing_resolves() {
        let transcript = concat!(
            r#"{"type":"response_item","payload":{"type":"local_shell_call","call_id":"call_02"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"local_shell_call_output","call_id":"call_02","output":"ok"}}"#,
            "\n",
        );
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Codex,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Grok is explicitly out of scope for cas-7e85 — always `false`,
    /// regardless of transcript content (falls back to the existing
    /// signals.json/mtime path).
    #[test]
    fn grok_in_flight_tool_call_always_false() {
        let transcript = concat!(
            r#"{"type":"response_item","payload":{"type":"function_call","call_id":"call_01"}}"#,
            "\n",
        );
        assert!(!has_in_flight_tool_call(
            Cursor::new(transcript),
            cas_mux::SupervisorCli::Grok,
            IN_FLIGHT_TAIL_LINES,
        ));
    }

    /// Convenience wrapper: missing file fails safe to `false`.
    #[test]
    fn transcript_has_in_flight_tool_call_missing_file_is_false() {
        let missing = Path::new("/tmp/does-not-exist-cas-7e85.jsonl");
        assert!(!transcript_has_in_flight_tool_call(
            missing,
            cas_mux::SupervisorCli::Claude
        ));
    }

    /// End-to-end through the file-reading wrapper, not just the `Read`
    /// core — proves `transcript_has_in_flight_tool_call` actually opens
    /// and parses the file it's given.
    #[test]
    fn transcript_has_in_flight_tool_call_reads_real_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_x","name":"Bash","input":{}}]}}"#,
                "\n",
            ),
        )
        .unwrap();
        assert!(transcript_has_in_flight_tool_call(
            &path,
            cas_mux::SupervisorCli::Claude
        ));
    }

    /// cas-7e85 AC4: this is the SAME signal `classify_from_evidence` (the
    /// core of `cas factory is-wedged`) consults — a stale-by-mtime but
    /// in-flight transcript must classify Alive, not Starved, matching the
    /// director's suppression above one-for-one.
    #[test]
    fn classify_from_evidence_in_flight_call_overrides_stale_mtime() {
        let got = classify_from_evidence(
            true,
            Some(Duration::from_secs(10 * 60)), // long past every window
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            true, // in-flight tool call
        );
        assert_eq!(
            got,
            WorkerLivenessState::Alive,
            "an in-flight tool call must classify Alive regardless of mtime age"
        );
    }

    /// cas-058e: a backgrounded command is a live activity signal even after
    /// the transcript records that no tool call is currently in flight.
    #[test]
    fn classify_background_process_overrides_stale_mtime() {
        let got = classify_from_evidence_with_background(
            true,
            Some(Duration::from_secs(10 * 60)),
            false,
            None,
            false,
            TRANSCRIPT_FRESH_WINDOW,
            false,
            true,
        );
        assert_eq!(got, WorkerLivenessState::Alive);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn background_processes_reports_a_real_long_running_child_and_no_child_case() {
        // This process is the fake pane PID. Its direct `sleep` child is the
        // same shape as `codex -> shell -> cargo` after a backgrounded build.
        assert_eq!(
            background_processes_for(std::process::id()),
            BackgroundProcessState::Available(vec![]),
            "the fake pane must begin with no descendants"
        );
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn long-running fake background job");
        // Linux publishes the child relationship asynchronously relative to
        // `spawn` returning; settle only the test fixture, never production.
        std::thread::sleep(Duration::from_millis(20));
        let observed = background_processes_for(std::process::id());
        child.kill().expect("stop fake background job");
        child.wait().expect("reap fake background job");

        let BackgroundProcessState::Available(processes) = observed else {
            panic!("live /proc child walk unexpectedly unavailable: {observed:?}");
        };
        assert!(
            processes
                .iter()
                .any(|process| process.command == "sleep" && process.age_secs < 5),
            "expected named, age-bearing child evidence: {processes:?}"
        );
        assert!(BackgroundProcessState::Available(processes).is_active());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn known_mcp_sidecars_do_not_count_as_worker_background_jobs() {
        assert!(is_known_sidecar_commandline(
            "npm",
            b"npm\0exec\0@playwright/mcp@0.0.70\0"
        ));
        assert!(is_known_sidecar_commandline(
            "node",
            b"node\0.../@playwright/mcp/cli.js\0"
        ));
        assert!(is_known_sidecar_commandline("cas", b"cas\0serve\0"));
        assert!(!is_known_sidecar_commandline("node", b"node\0server.js\0"));
    }

    #[test]
    fn format_state_human_names_background_processes_and_unavailable_state() {
        let active = WorkerEvidence {
            pid: Some(42),
            pid_alive: true,
            transcript_path: None,
            transcript_mtime_age_secs: Some(600),
            crash_signature_match: false,
            worktree_edit_age_secs: None,
            session_id: "s".to_string(),
            in_flight_tool_call: false,
            background_processes: BackgroundProcessState::Available(vec![BackgroundProcess {
                command: "cargo".to_string(),
                age_secs: 1_832,
            }]),
            pending_permission: None,
        };
        assert!(
            format_state_human(&WorkerLivenessState::Alive, &active)
                .contains("background processes: cargo (1832s)")
        );
        let unavailable = WorkerEvidence {
            background_processes: BackgroundProcessState::Unavailable,
            ..active
        };
        assert!(
            format_state_human(&WorkerLivenessState::Unverified, &unavailable)
                .contains("background-process state unavailable")
        );
    }

    #[test]
    fn kill_verdict_refuses_legacy_agent_without_force() {
        // cas-4513 adversarial P0: legacy agent (no pid_starttime) must
        // NOT auto-kill without --force. Use a PID guaranteed alive on
        // every Linux host: PID 1 (init).
        let verdict = kill_verdict(1, None, false);
        assert_eq!(verdict, KillVerdict::RefuseNoFingerprint);
    }

    #[test]
    fn kill_verdict_force_overrides_missing_fingerprint() {
        // Force path documented in the skill: legacy agent with operator-
        // confirmed PID can be killed via --force.
        let verdict = kill_verdict(1, None, true);
        assert_eq!(verdict, KillVerdict::Go);
    }

    #[test]
    fn kill_verdict_dead_pid_is_already_dead_regardless_of_force() {
        // Use u32::MAX-1 which is out-of-range → kill(pid,0) returns ESRCH.
        let verdict = kill_verdict(u32::MAX - 1, None, false);
        assert_eq!(verdict, KillVerdict::AlreadyDead);
        let verdict_force = kill_verdict(u32::MAX - 1, None, true);
        assert_eq!(verdict_force, KillVerdict::AlreadyDead);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_verdict_refuses_fingerprint_mismatch() {
        // cas-4513 adversarial P0: a live PID with the wrong starttime
        // is treated as a recycled PID and refused (the core protection).
        // PID 1 on Linux has some real starttime; passing 0 guarantees mismatch.
        let verdict = kill_verdict(1, Some(0), false);
        assert_eq!(verdict, KillVerdict::RefuseFingerprintMismatch);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_verdict_go_on_fingerprint_match_self() {
        // Use our own pid + our own starttime — must classify Go.
        let my_pid = std::process::id();
        let my_starttime =
            crate::mcp::daemon::read_pid_starttime(my_pid).expect("self should have starttime");
        let verdict = kill_verdict(my_pid, Some(my_starttime), false);
        assert_eq!(verdict, KillVerdict::Go);
    }

    #[test]
    fn format_state_json_handles_backslash_and_control_chars() {
        // cas-4513 adversarial P2: the old hand-rolled escaper only
        // handled `"`; a path with `\` or a session_id with `\n` produced
        // malformed JSON. serde_json handles all of these.
        let ev = WorkerEvidence {
            pid: Some(4242),
            pid_alive: true,
            transcript_path: Some(PathBuf::from("/p/back\\slash\"quote.jsonl")),
            transcript_mtime_age_secs: None,
            crash_signature_match: false,
            worktree_edit_age_secs: None,
            // Newline + backslash inside session_id — worst-case.
            session_id: "ses\nfoo\\bar".to_string(),
            in_flight_tool_call: false,
            background_processes: BackgroundProcessState::Available(vec![]),
            pending_permission: None,
        };
        let out = format_state_json(&WorkerLivenessState::Alive, &ev);
        // Parse round-trip. If escaping is wrong, this panics with a clear
        // error — catching any regression back to the hand-rolled path.
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["session_id"], "ses\nfoo\\bar");
        assert_eq!(parsed["transcript_path"], "/p/back\\slash\"quote.jsonl");
    }

    // -------------------------------------------------------------------
    // cas-f781: process-table resolution (AC a) + post-kill lease gating
    // (AC b).
    // -------------------------------------------------------------------

    struct FakeProcessTable {
        entries: std::collections::HashMap<u32, (Option<Vec<u8>>, Option<Vec<u8>>)>,
    }

    impl ProcessTable for FakeProcessTable {
        fn pids(&self) -> Vec<u32> {
            self.entries.keys().copied().collect()
        }
        fn cmdline(&self, pid: u32) -> Option<Vec<u8>> {
            self.entries.get(&pid).and_then(|e| e.0.clone())
        }
        fn environ(&self, pid: u32) -> Option<Vec<u8>> {
            self.entries.get(&pid).and_then(|e| e.1.clone())
        }
    }

    #[test]
    fn agent_name_from_cmdline_extracts_flag_value() {
        let cmdline = b"claude\0--dangerously-skip-permissions\0--agent-name\0hv-live\0";
        assert_eq!(
            agent_name_from_cmdline(cmdline),
            Some("hv-live".to_string())
        );
    }

    #[test]
    fn agent_name_from_cmdline_tolerates_nice_wrapper_prefix() {
        // maybe_wrap_with_nice (cas-pty) may prepend `nice -n <N>` — the
        // flag search must not assume a fixed argv position.
        let cmdline = b"nice\0-n\010\0claude\0--agent-name\0hv-live\0";
        assert_eq!(
            agent_name_from_cmdline(cmdline),
            Some("hv-live".to_string())
        );
    }

    #[test]
    fn agent_name_from_cmdline_none_without_flag() {
        let cmdline = b"cas\0serve\0--foreground\0";
        assert_eq!(agent_name_from_cmdline(cmdline), None);
    }

    #[test]
    fn agent_name_from_environ_extracts_codex_env_var() {
        // Codex workers carry identity only in env (no --agent-name in
        // argv) — cas-f781 investigation.
        let environ = b"PATH=/usr/bin\0CAS_AGENT_NAME=hv-live\0CAS_AGENT_ROLE=worker\0";
        assert_eq!(
            agent_name_from_environ(environ),
            Some("hv-live".to_string())
        );
    }

    #[test]
    fn agent_name_from_environ_none_without_var() {
        let environ = b"PATH=/usr/bin\0HOME=/root\0";
        assert_eq!(agent_name_from_environ(environ), None);
    }

    #[test]
    fn find_worker_pid_prefers_live_agent_name_match_over_unrelated_process() {
        let mut entries = std::collections::HashMap::new();
        // An unrelated process (e.g. the tracked stale child pid from the
        // agent store) with no agent-name of its own.
        entries.insert(9999, (Some(b"cas\0serve\0".to_vec()), None));
        // The real worker: claude spawned with --agent-name hv-live.
        entries.insert(
            4242,
            (
                Some(b"claude\0--dangerously-skip-permissions\0--agent-name\0hv-live\0".to_vec()),
                None,
            ),
        );
        let table = FakeProcessTable { entries };
        assert_eq!(find_worker_pid(&table, "hv-live"), Some(4242));
    }

    #[test]
    fn find_worker_pid_matches_codex_via_environ() {
        let mut entries = std::collections::HashMap::new();
        entries.insert(
            5555,
            (
                Some(b"codex\0exec\0".to_vec()),
                Some(b"PATH=/usr/bin\0CAS_AGENT_NAME=hv-live\0".to_vec()),
            ),
        );
        let table = FakeProcessTable { entries };
        assert_eq!(find_worker_pid(&table, "hv-live"), Some(5555));
    }

    /// cas-c655: when both `cas serve` (MCP child) and the real `codex`
    /// binary inherit `CAS_AGENT_NAME`, prefer the codex PID — is-wedged
    /// was previously reporting the cas-serve PID as the worker.
    #[test]
    fn find_worker_pid_prefers_codex_binary_over_cas_serve_environ_twin() {
        let mut entries = std::collections::HashMap::new();
        entries.insert(
            111084, // cas serve — the false PID from the bug report
            (
                Some(b"cas\0serve\0--foreground\0".to_vec()),
                Some(b"PATH=/usr/bin\0CAS_AGENT_NAME=worker-android\0".to_vec()),
            ),
        );
        entries.insert(
            222000,
            (
                Some(b"/home/x/.local/bin/codex\0--yolo\0--no-alt-screen\0".to_vec()),
                Some(b"PATH=/usr/bin\0CAS_AGENT_NAME=worker-android\0".to_vec()),
            ),
        );
        let table = OrderedFakeProcessTable {
            // Enumerate cas-serve FIRST so a naive .find() would pick it.
            order: vec![111084, 222000],
            entries,
        };
        assert_eq!(
            find_worker_pid(&table, "worker-android"),
            Some(222000),
            "codex harness binary must win over cas-serve descendant"
        );
    }

    /// End-to-end cas-c655 happy path through classify_worker: codex cli,
    /// unresolved transcript, process busy → Alive (not Starved).
    #[test]
    fn classify_worker_codex_busy_without_transcript_is_alive() {
        let pid_probe = |_: u32| true;
        let worktree_probe = |_: &Path| None::<Duration>;
        let (state, ev) = classify_worker(
            Some(222000),
            None,
            None,
            "codex-worker-android-2f828ac6-deadbeef",
            cas_mux::SupervisorCli::Codex,
            pid_probe,
            worktree_probe,
            |_: u32| true, // process busy
        );
        assert_eq!(
            state,
            WorkerLivenessState::Alive,
            "busy codex with unresolved transcript must not be Starved: {ev:?}"
        );
        assert_eq!(ev.pid, Some(222000));
        assert!(ev.transcript_path.is_none());
    }

    /// End-to-end cas-c655: recent worktree activity rescues missing transcript.
    #[test]
    fn classify_worker_codex_recent_worktree_without_transcript_is_alive() {
        let pid_probe = |_: u32| true;
        let worktree_probe = |_: &Path| Some(Duration::from_secs(10));
        let (state, _) = classify_worker(
            Some(222000),
            None,
            Some(Path::new("/tmp/clone")),
            "codex-worker-android-2f828ac6-deadbeef",
            cas_mux::SupervisorCli::Codex,
            pid_probe,
            worktree_probe,
            |_: u32| false,
        );
        assert_eq!(state, WorkerLivenessState::Alive);
    }

    #[test]
    fn find_worker_pid_none_when_no_process_matches() {
        let mut entries = std::collections::HashMap::new();
        entries.insert(1, (Some(b"init\0".to_vec()), None));
        let table = FakeProcessTable { entries };
        assert_eq!(find_worker_pid(&table, "hv-live"), None);
    }

    /// A `ProcessTable` whose `pids()` returns entries in an EXPLICIT,
    /// caller-controlled order — `FakeProcessTable`'s `HashMap`-backed
    /// `pids()` has unspecified iteration order, which can't reliably
    /// reproduce "the wrong candidate is scanned first" (cas-a91b).
    struct OrderedFakeProcessTable {
        order: Vec<u32>,
        entries: std::collections::HashMap<u32, (Option<Vec<u8>>, Option<Vec<u8>>)>,
    }

    impl ProcessTable for OrderedFakeProcessTable {
        fn pids(&self) -> Vec<u32> {
            self.order.clone()
        }
        fn cmdline(&self, pid: u32) -> Option<Vec<u8>> {
            self.entries.get(&pid).and_then(|e| e.0.clone())
        }
        fn environ(&self, pid: u32) -> Option<Vec<u8>> {
            self.entries.get(&pid).and_then(|e| e.1.clone())
        }
    }

    #[test]
    fn find_worker_pid_prefers_cmdline_match_over_environ_match_regardless_of_scan_order() {
        // cas-a91b P1: CAS_AGENT_NAME is inherited by EVERY descendant of the
        // worker (its `cas serve` child, git, cargo, ...) — an environ-only
        // match is not proof of being the actual leader, unlike argv, which
        // is never copied to children. Simulate the exact failure mode: a
        // descendant (environ match only) is enumerated BEFORE the real
        // leader (cmdline match) — proving the two-pass priority fix picks
        // the leader regardless of scan order, where the pre-fix single-pass
        // `.find()` would have nondeterministically returned the descendant.
        let mut entries = std::collections::HashMap::new();
        entries.insert(
            9999,
            (
                Some(b"cas\0serve\0".to_vec()),
                Some(b"PATH=/usr/bin\0CAS_AGENT_NAME=hv-live\0".to_vec()),
            ),
        );
        entries.insert(
            4242,
            (
                Some(b"claude\0--dangerously-skip-permissions\0--agent-name\0hv-live\0".to_vec()),
                None,
            ),
        );
        let table = OrderedFakeProcessTable {
            order: vec![9999, 4242], // descendant (environ match) scanned FIRST
            entries,
        };
        assert_eq!(
            find_worker_pid(&table, "hv-live"),
            Some(4242),
            "the cmdline (leader) match must win even though the environ-matching \
             descendant was scanned first"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_group_leader_pid_converts_descendant_to_actual_leader() {
        // cas-a91b: prove getpgid() correctly walks a descendant pid back to
        // its real process-group leader — the fix that makes killpg safe to
        // call on whatever find_worker_pid resolved, even when that's a
        // descendant rather than the leader itself. Spawn a detached leader
        // (own session/group, distinct from the `cargo test` process group)
        // whose script backgrounds a child in the SAME group, then confirm
        // resolve_group_leader_pid(child_pid) == leader_pid.
        use std::os::unix::process::CommandExt;

        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("child.pid");
        let script = format!("sleep 5 & echo $! > {} ; wait", pidfile.display());

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(&script);
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut leader = cmd.spawn().expect("spawn detached leader");
        let leader_pid = leader.id();

        let mut child_pid: Option<u32> = None;
        for _ in 0..50 {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(p) = s.trim().parse::<u32>() {
                    child_pid = Some(p);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let child_pid = child_pid.expect("background child pid should appear");

        assert_eq!(
            resolve_group_leader_pid(child_pid),
            Some(leader_pid),
            "a descendant's group leader must resolve to the actual session leader"
        );
        assert_eq!(
            resolve_group_leader_pid(leader_pid),
            Some(leader_pid),
            "the leader's own group leader is itself (pid == pgid via setsid())"
        );

        // Clean up: killpg the real group so nothing outlives the test.
        let _ = send_sigkill(leader_pid);
        let _ = leader.wait();
    }

    #[cfg(unix)]
    #[test]
    fn send_sigkill_refuses_esrch_when_target_pid_still_alive() {
        // cas-a91b P1: killpg() on an ordinary (non-leader) pid returns
        // ESRCH because no process group has that id — but if the
        // underlying process is still alive, blindly treating ESRCH as
        // "already dead" silently no-ops the kill while the caller believes
        // it succeeded. This is the exact destructive path: a live worker's
        // task lease would then get reset out from under it. Use a plain
        // child process (NOT a session/group leader — it stays in this
        // test's own process group) as the "wrong pid resolved" stand-in.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn plain child");
        let pid = child.id();
        assert!(crate::mcp::daemon::pid_alive(pid));

        let result = send_sigkill(pid);
        assert!(
            result.is_err(),
            "send_sigkill must refuse (return Err), not silently succeed, when killpg(pid) \
             returns ESRCH but pid is still alive: {result:?}"
        );

        // This test process is the child's real parent — kill + reap directly
        // rather than relying on the (deliberately refused) send_sigkill.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn verify_death_treats_zombie_as_confirmed_dead() {
        // cas-a91b P3: a killed process GROUP LEADER becomes a zombie under
        // its original parent (not `cas factory kill`) — pid_alive
        // (kill(pid,0)) reports a zombie as alive, since its /proc entry
        // persists until reaped. Without zombie detection, verify_death
        // would time out (200ms) and return false for an effectively-dead
        // worker, leaving its task stuck InProgress.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn short-lived child");
        let pid = child.id();
        // A zombie exists once the child exits but before THIS process (its
        // parent) reaps it via wait(). Do not assume a fixed delay is enough:
        // factory churn can defer scheduling this short-lived child beyond an
        // arbitrary 100ms sleep. Poll only this test-owned PID for a bounded
        // interval, so unrelated host zombies cannot affect the assertion.
        let mut became_zombie = false;
        for _ in 0..100 {
            if pid_is_zombie(pid) {
                became_zombie = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let confirmed_dead = became_zombie && verify_death(pid);
        let _ = child.wait(); // reap even when an assertion below fails
        assert!(
            became_zombie,
            "owned child should become a zombie within one second (exited, not yet reaped)"
        );
        assert!(
            confirmed_dead,
            "a zombie must be treated as confirmed-dead, not timed-out-alive"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn is_zombie_state_parses_z_state_from_stat_line() {
        // Pure-function coverage independent of a real /proc round-trip.
        // Synthetic stat line: "pid (comm) state ppid ...".
        assert!(is_zombie_state("1234 (sh) Z 1 1234 1234 0 -1 ..."));
        assert!(!is_zombie_state("1234 (sh) R 1 1234 1234 0 -1 ..."));
        assert!(!is_zombie_state("1234 (sh) S 1 1234 1234 0 -1 ..."));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn is_zombie_state_handles_comm_with_parens_and_spaces() {
        // comm can itself contain spaces/parens — must split on the LAST
        // `)`, same caveat as daemon::parse_starttime_from_stat.
        assert!(is_zombie_state(
            "1234 (my (weird) proc) Z 1 1234 1234 0 -1 ..."
        ));
    }

    #[test]
    fn pick_kill_pid_prefers_resolved_over_stale_tracked_pid() {
        // cas-f781 AC a: a live process-table match must win over a stale
        // tracked child pid, even though the tracked pid also exists.
        assert_eq!(pick_kill_pid(Some(9999), Some(4242)), Some(4242));
    }

    #[test]
    fn pick_kill_pid_falls_back_to_tracked_when_no_scan_match() {
        assert_eq!(pick_kill_pid(Some(9999), None), Some(9999));
    }

    #[test]
    fn pick_kill_pid_none_when_nothing_available() {
        assert_eq!(pick_kill_pid(None, None), None);
    }

    #[test]
    fn decide_post_kill_action_resets_when_already_dead() {
        assert!(decide_post_kill_action(&KillVerdict::AlreadyDead, false));
    }

    #[test]
    fn decide_post_kill_action_resets_only_if_death_confirmed_after_go() {
        assert!(decide_post_kill_action(&KillVerdict::Go, true));
        assert!(!decide_post_kill_action(&KillVerdict::Go, false));
    }

    #[test]
    fn decide_post_kill_action_never_resets_on_refused_kill() {
        // cas-f781 AC b: a still-alive process — kill refused, never
        // attempted — must never have its lease reset out from under it.
        assert!(!decide_post_kill_action(
            &KillVerdict::RefuseFingerprintMismatch,
            false
        ));
        assert!(!decide_post_kill_action(
            &KillVerdict::RefuseNoFingerprint,
            false
        ));
        // Even if the process happened to die of unrelated causes right
        // after the refusal, the gate keys only off the verdict for the
        // refuse cases — a refused kill is never a green light to reset.
        assert!(!decide_post_kill_action(
            &KillVerdict::RefuseFingerprintMismatch,
            true
        ));
    }

    #[cfg(unix)]
    #[test]
    fn send_sigkill_terminates_the_whole_process_group_not_just_the_leader() {
        // cas-f781 AC a: prove `send_sigkill` uses killpg semantics — a
        // bare `kill(leader_pid)` would leave a backgrounded sibling in the
        // same process group alive. Spawn a detached session leader (own
        // pgid, distinct from the `cargo test` process group) whose script
        // backgrounds a second long-lived process in the same group, then
        // confirm BOTH die from a single send_sigkill(leader_pid) call.
        use std::os::unix::process::CommandExt;

        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("child.pid");
        let script = format!("sleep 30 & echo $! > {} ; wait", pidfile.display());

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(&script);
        // SAFETY: setsid() is async-signal-safe; standard pattern for
        // detaching a test child into its own session/process group so
        // this test can't touch the surrounding `cargo test` process group.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut leader = cmd.spawn().expect("spawn detached leader");
        let leader_pid = leader.id();

        let mut child_pid: Option<u32> = None;
        for _ in 0..50 {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(p) = s.trim().parse::<u32>() {
                    child_pid = Some(p);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let child_pid = child_pid.expect("background child pid should appear");

        assert!(crate::mcp::daemon::pid_alive(leader_pid));
        assert!(crate::mcp::daemon::pid_alive(child_pid));

        send_sigkill(leader_pid).expect("killpg should succeed");

        // `leader` is a direct child of THIS test process, so once killed
        // it's a zombie (kill(pid,0)/pid_alive would report it "alive"
        // forever) until its parent reaps it — use `wait()` rather than
        // `verify_death` to confirm the leader specifically.
        let status = leader.wait().expect("wait on killed leader");
        assert!(
            !status.success(),
            "leader should have been SIGKILLed, not exited cleanly"
        );

        // `child_pid` (the backgrounded grandchild) is NOT a child of this
        // test process — it's reparented away once the shell dies, so
        // `pid_alive` polling is the right liveness check here, exactly as
        // `execute_kill` uses it in production.
        assert!(
            verify_death(child_pid),
            "background sibling in the same process group should ALSO die via killpg \
             — a bare kill(leader_pid) would leave it running"
        );
    }
}
