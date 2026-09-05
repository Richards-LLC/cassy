//! Factory session hygiene — surface and record the main worktree's state
//! around session boundaries so supervisors can attribute leftover
//! uncommitted work from crashed/interrupted prior factory sessions.
//!
//! Two features live here:
//!
//! 1. A **structured factory event log** appended to
//!    `.cas/logs/factory-session-{YYYY-MM-DD}.log`. Mid-session task,
//!    coordination, and worktree events share the same JSON-lines append
//!    path as the session-end worktree summary. This gives supervisors a
//!    greppable history without repeating an unbounded porcelain dump.
//!
//! 2. A **WIP candidates** helper used by `coordination action=gc_report`
//!    (and consumable by `SessionStart` triage for task cas-aeec) that
//!    lists uncommitted entries in the main worktree so they can be
//!    surfaced — never auto-deleted.
//!
//! The module is best-effort: I/O and git failures are swallowed because
//! hygiene instrumentation must never break a session-end hook.

use std::path::{Path, PathBuf};
use std::process::Command;

use cas_store::{IngestBatch, KnowledgePage, KnowledgeStore, PageWrite, SqliteKnowledgeStore};
use cas_types::{AgentStatus, TaskStatus, TaskType};

use crate::store::{AgentStore, TaskStore, open_agent_store, open_task_store};

const CURRENT_STATE_REL_PATH: &str = "current-state.md";
const CURRENT_STATE_SOURCE: &str = "cas://session-end-current-state";

/// Append one structured JSON-lines event to today's factory session log.
///
/// Mid-session callers use the active `CAS_FACTORY_SESSION`; outside factory
/// mode this is a no-op. Logging is deliberately best-effort and must never
/// make the operation being observed fail.
pub fn append_factory_session_event(
    cas_root: &Path,
    event: &str,
    fields: &[(&str, &str)],
) -> Option<PathBuf> {
    let factory_session = std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let agent_name = std::env::var("CAS_AGENT_NAME").ok();
    let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
    append_factory_session_event_with_context(
        cas_root,
        event,
        &factory_session,
        agent_name.as_deref(),
        agent_role.as_deref(),
        fields,
    )
}

fn append_factory_session_event_with_context(
    cas_root: &Path,
    event: &str,
    factory_session: &str,
    agent_name: Option<&str>,
    agent_role: Option<&str>,
    fields: &[(&str, &str)],
) -> Option<PathBuf> {
    let now = chrono::Utc::now();
    let log_dir = cas_root.join("logs");
    std::fs::create_dir_all(&log_dir).ok()?;
    let log_path = log_dir.join(format!("factory-session-{}.log", now.format("%Y-%m-%d")));

    let mut record = serde_json::Map::new();
    record.insert("timestamp".into(), serde_json::Value::String(now.to_rfc3339()));
    record.insert("event".into(), serde_json::Value::String(event.to_string()));
    record.insert(
        "factory_session".into(),
        serde_json::Value::String(factory_session.to_string()),
    );
    record.insert(
        "agent".into(),
        serde_json::Value::String(agent_name.unwrap_or("unknown").to_string()),
    );
    record.insert(
        "role".into(),
        serde_json::Value::String(agent_role.unwrap_or("unknown").to_string()),
    );
    for (key, value) in fields {
        record.insert(
            (*key).to_string(),
            serde_json::Value::String((*value).to_string()),
        );
    }

    let mut line = serde_json::to_string(&serde_json::Value::Object(record)).ok()?;
    line.push('\n');

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok()?;
    file.write_all(line.as_bytes()).ok()?;
    Some(log_path)
}

/// Single `git status --porcelain` entry.
///
/// `status` is the raw two-char porcelain code (e.g. `"??"`, `" M"`, `"M "`,
/// `"A "`). `path` is the file path relative to the worktree root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainEntry {
    pub status: String,
    pub path: String,
}

impl PorcelainEntry {
    /// True if this is an untracked file (`??` status).
    pub fn is_untracked(&self) -> bool {
        self.status.starts_with("??")
    }

    /// Short human label for the entry's state.
    pub fn label(&self) -> &'static str {
        match self.status.as_str() {
            "??" => "untracked",
            " M" => "modified",
            "M " | "MM" | "AM" => "modified-staged",
            "A " => "added",
            "D " | " D" => "deleted",
            _ => "changed",
        }
    }
}

/// Resolve the main repo root for this Cassy installation.
///
/// By convention, the Cassy root sits at `<repo>/.cas`, so the main
/// worktree is its parent directory. Returns `None` if the layout is
/// unexpected.
pub fn main_worktree_path(cas_root: &Path) -> Option<PathBuf> {
    let repo_adjacent = cas_root.parent()?;

    // Ask git for the *common* git dir; in a linked worktree this points at
    // the main repo's `.git`, whereas `--git-dir` would point at
    // `.git/worktrees/<name>`. The main worktree then lives one dir above
    // the common dir (assuming the normal `<repo>/.git` layout). Falls
    // back to `cas_root.parent()` when git is unavailable or the layout is
    // unexpected, preserving the prior best-effort behaviour.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_adjacent)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(repo_adjacent.to_path_buf());
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return Some(repo_adjacent.to_path_buf());
    }
    let common_path = PathBuf::from(common);
    // `.git` common dir → main worktree is its parent.
    if common_path.file_name().and_then(|s| s.to_str()) == Some(".git") {
        if let Some(parent) = common_path.parent() {
            return Some(parent.to_path_buf());
        }
    }
    // Bare repo or unusual layout — give up safely.
    Some(repo_adjacent.to_path_buf())
}

/// Run `git status --porcelain=v1` in `repo` and parse the output.
///
/// Returns `None` if git is unavailable, the directory is not a repo,
/// or the command fails. On success, returns an empty vec for a clean
/// tree.
pub fn porcelain_status(repo: &Path) -> Option<Vec<PorcelainEntry>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain=v1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        // Porcelain v1: "XY path", where XY are exactly 2 chars and then a space.
        let (status, rest) = line.split_at(2);
        // `rest` starts with a space; strip it.
        let path = rest.trim_start().to_string();
        entries.push(PorcelainEntry {
            status: status.to_string(),
            path,
        });
    }
    Some(entries)
}

/// Append a summarized session-end event to
/// `<cas_root>/logs/factory-session-{YYYY-MM-DD}.log`.
///
/// The event records dirty counts rather than an unbounded list of paths. It
/// uses the same JSON-lines append path as mid-session factory events.
///
/// Returns the log path on success, or `None` if the worktree could not be
/// resolved or the git probe failed. I/O errors are swallowed by design.
pub fn write_session_end_manifest(
    cas_root: &Path,
    session_id: &str,
    agent_name: Option<&str>,
    agent_role: Option<&str>,
) -> Option<PathBuf> {
    let repo = main_worktree_path(cas_root)?;
    let entries = porcelain_status(&repo)?;
    let total = entries.len().to_string();
    let untracked = entries
        .iter()
        .filter(|entry| entry.is_untracked())
        .count()
        .to_string();
    let modified = entries
        .iter()
        .filter(|entry| !entry.is_untracked())
        .count()
        .to_string();
    let git_status = if entries.is_empty() { "clean" } else { "dirty" };
    let worktree = repo.display().to_string();

    append_factory_session_event_with_context(
        cas_root,
        "session_end",
        session_id,
        agent_name,
        agent_role,
        &[
            ("worktree", &worktree),
            ("git_status", git_status),
            ("git_status_total", &total),
            ("git_status_modified", &modified),
            ("git_status_untracked", &untracked),
        ],
    )
}

/// Upsert the one mechanical handoff snapshot used to seed the next session.
///
/// Unlike a handoff memory, this is deliberately assembled only from local
/// stores, Git, and the saved release-receipt artifacts. It makes no model
/// call, and failures are returned to the SessionEnd caller so that caller can
/// keep the hook best-effort.
pub fn write_current_state_snapshot(cas_root: &Path) -> Result<(), String> {
    let repo = main_worktree_path(cas_root)
        .ok_or_else(|| "could not resolve the main worktree".to_string())?;
    let task_store =
        open_task_store(cas_root).map_err(|error| format!("could not open task store: {error}"))?;
    let tasks = task_store
        .list(None)
        .map_err(|error| format!("could not list tasks: {error}"))?;
    let agents = open_agent_store(cas_root)
        .map_err(|error| format!("could not open agent store: {error}"))?
        .list(Some(AgentStatus::Active))
        .map_err(|error| format!("could not list active agents: {error}"))?;

    let mut open_epics: Vec<_> = tasks
        .iter()
        .filter(|task| task.task_type == TaskType::Epic && !task.is_terminal())
        .collect();
    open_epics.sort_by(|left, right| left.id.cmp(&right.id));

    let mut epic_lines = Vec::new();
    for epic in open_epics {
        let subtasks = task_store
            .get_subtasks(&epic.id)
            .map_err(|error| format!("could not list subtasks for {}: {error}", epic.id))?;
        let remaining = subtasks
            .iter()
            .filter(|task| !task.is_terminal())
            .count();
        epic_lines.push(format!(
            "- {} — {} ({} subtasks; {} open)",
            epic.id,
            epic.title,
            subtasks.len(),
            remaining
        ));
    }

    let mut awaiting_merge: Vec<_> = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::AwaitingMerge)
        .collect();
    awaiting_merge.sort_by(|left, right| left.id.cmp(&right.id));

    let mut live_agents = agents;
    live_agents.sort_by(|left, right| left.id.cmp(&right.id));

    let head = git_value(&repo, &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|| "unavailable".to_string());
    let tag = git_value(&repo, &["describe", "--tags", "--exact-match"])
        .unwrap_or_else(|| "none".to_string());
    let last_release = latest_posted_release_receipt(&repo)
        .unwrap_or_else(|| "No posted release receipt found.".to_string());

    let mut body = format!(
        "# Current State\n\n\
This page is regenerated automatically at SessionEnd from local Cassy data; it is not a manual handoff.\n\n\
## Runtime and HEAD\n\n\
- Runtime version: v{}\n\
- HEAD: {head}\n\
- HEAD tag: {tag}\n\n\
## Open Epics\n\n",
        env!("CARGO_PKG_VERSION")
    );
    if epic_lines.is_empty() {
        body.push_str("- None.\n");
    } else {
        body.push_str(&epic_lines.join("\n"));
        body.push('\n');
    }

    body.push_str("\n## Pending Merges\n\n");
    if awaiting_merge.is_empty() {
        body.push_str("- None.\n");
    } else {
        for task in awaiting_merge {
            body.push_str(&format!("- {} — {}\n", task.id, task.title));
        }
    }

    body.push_str("\n## Live Fleet\n\n");
    if live_agents.is_empty() {
        body.push_str("- No active agents.\n");
    } else {
        for agent in live_agents {
            body.push_str(&format!(
                "- {} ({}, {}, {} active tasks; heartbeat {})\n",
                agent.name,
                agent.id,
                agent.role,
                agent.active_tasks,
                agent.last_heartbeat.to_rfc3339()
            ));
        }
    }

    body.push_str("\n## Last Release Announced\n\n");
    body.push_str(&format!("- {last_release}\n"));

    let store = SqliteKnowledgeStore::open(cas_root)
        .map_err(|error| format!("could not open knowledge store: {error}"))?;
    let existing = store
        .get_page_by_rel_path(CURRENT_STATE_REL_PATH)
        .map_err(|error| format!("could not read current-state page: {error}"))?;
    if existing.as_ref().is_some_and(|page| page.locked) {
        return Err("current-state page is locked; preserving the user-owned page".to_string());
    }

    let id = match &existing {
        Some(page) => page.id.clone(),
        None => store
            .generate_id()
            .map_err(|error| format!("could not allocate current-state page ID: {error}"))?,
    };
    let mut page = KnowledgePage::new(id, "workflow", "Current State");
    page.rel_path = CURRENT_STATE_REL_PATH.to_string();
    page.sources = vec![CURRENT_STATE_SOURCE.to_string()];
    page.snippet = format!("Runtime v{} at HEAD {head}", env!("CARGO_PKG_VERSION"));
    if let Some(existing) = existing {
        page.created_at = existing.created_at;
    }

    store
        .commit_ingest(&IngestBatch {
            pages: vec![PageWrite { page, body }],
            ..IngestBatch::default()
        })
        .map_err(|error| format!("could not write current-state page: {error}"))?;
    Ok(())
}

fn git_value(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn latest_posted_release_receipt(repo: &Path) -> Option<String> {
    let release_dir = repo.join("docs/release-notes");
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(release_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).ok()?;
        if !body.to_ascii_uppercase().contains("POSTED") {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        if latest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            latest = Some((modified, name));
        }
    }
    latest.map(|(_, name)| name)
}

/// Summary of WIP candidates in the main worktree.
///
/// Returned by [`wip_candidates`] so callers can render a concise report
/// without re-running git. `entries` preserves the porcelain output order.
#[derive(Debug, Clone, Default)]
pub struct WipSummary {
    pub worktree: PathBuf,
    pub entries: Vec<PorcelainEntry>,
}

impl WipSummary {
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn untracked_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_untracked()).count()
    }

    pub fn modified_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_untracked()).count()
    }
}

/// Inspect the main worktree and return a [`WipSummary`].
///
/// Returns `None` if the worktree path can't be resolved or git is
/// unavailable. Clean trees return `Some(WipSummary { entries: [] })`
/// so callers can still report "clean".
pub fn wip_candidates(cas_root: &Path) -> Option<WipSummary> {
    let repo = main_worktree_path(cas_root)?;
    let entries = porcelain_status(&repo)?;
    Some(WipSummary {
        worktree: repo,
        entries,
    })
}

/// Extract the first `cas-xxxx` task id from `text`, if any.
///
/// Task ids in commit messages follow the canonical 4-char hex form used
/// throughout the codebase (e.g. `cas-4181`, `cas-a9ab`). Anything past 4
/// lowercase hex chars is rejected so arbitrary strings like `cas-foo`
/// are not falsely matched.
pub(crate) fn extract_task_id(text: &str) -> Option<String> {
    // Tiny hand-rolled scanner to avoid pulling in a regex dep just for one
    // match. Finds `cas-` then up to 4 hex chars followed by a non-hex
    // boundary (whitespace, punctuation, end-of-string).
    let bytes = text.as_bytes();
    let mut i = 0;
    let eq_ci = |a: u8, b: u8| a.eq_ignore_ascii_case(&b);
    while i + 8 <= bytes.len() {
        if eq_ci(bytes[i], b'c')
            && eq_ci(bytes[i + 1], b'a')
            && eq_ci(bytes[i + 2], b's')
            && bytes[i + 3] == b'-'
        {
            let start = i + 4;
            let mut end = start;
            while end < bytes.len() && end - start < 4 {
                let c = bytes[end];
                let is_hex =
                    c.is_ascii_digit() || (b'a'..=b'f').contains(&c) || (b'A'..=b'F').contains(&c);
                if !is_hex {
                    break;
                }
                end += 1;
            }
            if end - start == 4 {
                // Boundary check: next byte must be a non-hex, non-alnum
                // delimiter (or end of string).
                let terminates = end == bytes.len() || {
                    let c = bytes[end];
                    !(c.is_ascii_digit()
                        || (b'a'..=b'z').contains(&c)
                        || (b'A'..=b'Z').contains(&c))
                };
                if terminates {
                    let id = std::str::from_utf8(&bytes[i..end]).ok()?.to_ascii_lowercase();
                    return Some(id);
                }
            }
        }
        i += 1;
    }
    None
}

/// Ask git for the most recent commit that touched `file` and return the
/// first `cas-xxxx` task id from its subject+body, if any. Used to
/// attribute a modified WIP file to the task that most likely left it
/// behind. Returns `None` for untracked files, missing history, or when
/// the last commit message carries no task id.
pub fn attribute_file_to_task(repo: &Path, file: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%s%n%b", "--", file])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return None;
    }
    extract_task_id(&text)
}

/// Maximum WIP entries the SessionStart banner itself renders inline.
///
/// Above this cap we print a "... and N more" suffix and direct the
/// supervisor to `gc_report` for the full list. The cap exists for two
/// reasons surfaced in review:
///
/// 1. Each tracked entry spawns `git log -1` for attribution. On a
///    pathological dirty tree (hundreds of files after a prior-session
///    crash) an uncapped banner turns the SessionStart hook into a
///    multi-second stall, which the user experiences as Claude Code
///    hanging on startup. 20 entries ≤ ~1s at 20–50ms per subprocess.
/// 2. The full SessionStart preview window is limited. Flooding it with
///    attribution lines buries the codemap/overview signals.
const WIP_BANNER_MAX_ENTRIES: usize = 20;

/// Render the SessionStart triage banner for a supervisor session, or
/// `None` when the worktree is clean / git is unavailable / cas_root
/// cannot be resolved. The banner is best-effort and must never fail a
/// session start.
///
/// The banner caps itself at [`WIP_BANNER_MAX_ENTRIES`] inline rows and
/// forwards the overflow count to `gc_report` so the supervisor can
/// paginate on demand — this bounds both `git log` subprocess fan-out
/// and the token budget the banner eats from the context window.
///
/// Output shape (example):
/// ```text
/// ⚠ Prior-factory WIP detected in main worktree (3 files, 2 modified, 1 untracked):
///   [modified]  src/foo.rs                (last touched by cas-a9ab)
///   [modified]  src/bar.rs                (last touched by cas-4181)
///   [untracked] src/baz.rs                (unattributed — no git history)
///
/// Triage BEFORE spawning workers: decide salvage / commit / discard.
/// Full history: .cas/logs/factory-session-{date}.log
/// ```
#[cfg(test)]
pub fn build_session_start_wip_banner(cas_root: &Path) -> Option<String> {
    build_session_start_wip_banner_sized(cas_root).map(|b| b.full)
}

/// A SessionStart banner in both its full and compact renderings (cas-b114).
///
/// The assembled SessionStart payload has an aggregate byte budget (see
/// [`crate::hooks::handlers::session_budget`]). Variable-length banners hand
/// the assembler both forms so an over-budget payload degrades to counts plus
/// the remediation command instead of being truncated mid-line by the harness.
#[derive(Debug, Clone)]
pub struct SessionStartBanner {
    pub full: String,
    pub compact: String,
}

/// Full + compact renderings of the prior-factory WIP triage banner.
pub fn build_session_start_wip_banner_sized(cas_root: &Path) -> Option<SessionStartBanner> {
    let summary = wip_candidates(cas_root)?;
    if summary.is_clean() {
        return None;
    }
    Some(render_wip_banner(&summary))
}

/// Pure renderer for the WIP banner — separated from the git scan so the
/// SessionStart budget test (cas-b114) can drive a worst-case summary.
pub(crate) fn render_wip_banner(summary: &WipSummary) -> SessionStartBanner {
    let prefix_for_compact = crate::harness_policy::own_tool_prefix();
    let compact = format!(
        "⚠ Prior-factory WIP in main worktree: {} file(s) ({} modified, {} untracked). \
         Triage BEFORE spawning workers — run `{prefix_for_compact}coordination action=gc_report` \
         for the per-file list and task attribution; full history in \
         .cas/logs/factory-session-{{date}}.log.\n",
        summary.entries.len(),
        summary.modified_count(),
        summary.untracked_count(),
    );
    let mut out = String::new();
    out.push_str(&format!(
        "⚠ Prior-factory WIP detected in main worktree ({} files, {} modified, {} untracked):\n",
        summary.entries.len(),
        summary.modified_count(),
        summary.untracked_count(),
    ));
    let total = summary.entries.len();
    let shown = total.min(WIP_BANNER_MAX_ENTRIES);
    for entry in summary.entries.iter().take(shown) {
        let attribution = if entry.is_untracked() {
            "(unattributed — no git history)".to_string()
        } else {
            match attribute_file_to_task(&summary.worktree, &entry.path) {
                Some(task_id) => format!("(last touched by {task_id})"),
                None => "(no task id in last commit)".to_string(),
            }
        };
        out.push_str(&format!(
            "  [{:15}] {}  {}\n",
            entry.label(),
            entry.path,
            attribution,
        ));
    }
    if total > shown {
        let extra = total - shown;
        // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — this banner is read
        // by the supervisor telling itself what to run next.
        let prefix = crate::harness_policy::own_tool_prefix();
        out.push_str(&format!(
            "  ... and {extra} more — run `{prefix}coordination action=gc_report` for the full list.\n",
        ));
    }
    out.push_str(
        "\nTriage BEFORE spawning workers: decide salvage / commit / discard.\n\
         Full history: .cas/logs/factory-session-{date}.log (see cas-supervisor-checklist)\n",
    );
    SessionStartBanner { full: out, compact }
}

/// Maximum orphan rows the SessionStart banner renders inline (cas-b7dd).
const ORPHAN_BANNER_MAX_ENTRIES: usize = 10;

/// SessionStart banner for leftovers from dead sessions (cas-b7dd, GH #88).
///
/// A new session used to inherit the previous one's orphans silently and
/// discover them as an `EADDRINUSE` failure several minutes later, with no
/// hint that the squatter was Cassy's own leftover. Stating them up front turns
/// that into an explicit "adopt or kill" decision at the one moment the
/// supervisor is deciding what this session will do.
///
/// Visibility only — this NEVER kills anything. Killing stays behind
/// `gc_cleanup force=true dry_run=false`, because a session start is not
/// consent to signal processes.
///
/// Returns `None` when there is nothing to report, which is the common case.
#[cfg(test)]
pub fn build_session_start_orphan_banner(cas_root: &Path) -> Option<String> {
    build_session_start_orphan_banner_sized(cas_root).map(|b| b.full)
}

/// Full + compact renderings of the stale-builtin-reference banner (cas-0c0a).
///
/// `cas update --sync` refuses to overwrite a skill reference whose content
/// matches neither its recorded baseline nor any version Cassy shipped, and says
/// so only in CLI output that scripted/unattended syncs discard. Eight
/// supervisor/worker reference files sat six weeks stale that way, including
/// worker-recovery guidance whose absence can strand live workers. This
/// surfaces the same skip where every session sees it.
///
/// Returns `None` when the last sync skipped nothing — the common case.
pub fn build_session_start_stale_reference_banner_sized(
    cas_root: &Path,
) -> Option<SessionStartBanner> {
    let skipped = crate::builtins::skipped_owned_references(cas_root);
    if skipped.is_empty() {
        return None;
    }
    Some(render_stale_reference_banner(&skipped))
}

/// Pure renderer for the stale-reference banner — separated from the ledger
/// read so tests (and the SessionStart budget test) can drive it directly.
pub(crate) fn render_stale_reference_banner(
    skipped: &std::collections::BTreeMap<String, Vec<String>>,
) -> SessionStartBanner {
    let total: usize = skipped.values().map(Vec::len).sum();
    let compact = format!(
        "⚠ {total} builtin skill reference file(s) are NOT being updated by `cas update --sync` \
         — they differ from every version Cassy shipped and are preserved as local edits. \
         Run `cas update --sync` for the path list; to accept the Cassy version, delete the file \
         and rerun the sync.\n"
    );

    let mut full = format!(
        "⚠ {total} builtin skill reference file(s) are NOT being updated by `cas update --sync`: \
         their content matches neither the last synced baseline nor any version Cassy shipped, so \
         sync preserves them as local customizations. Until resolved these files stay stale \
         forever.\n"
    );
    for (harness, paths) in skipped {
        for path in paths {
            full.push_str(&format!("  ! .{harness}/{path}\n"));
        }
    }
    full.push_str(
        "  Review each file; to accept the Cassy version, delete it and rerun `cas update --sync`.\n",
    );

    SessionStartBanner { full, compact }
}

/// Maximum staged-report file names the unfiled-reports banner lists inline.
const UNFILED_REPORTS_BANNER_MAX_ENTRIES: usize = 10;

/// Staged bug/feature reports sitting unfiled at `docs/requests/` (cas-20f27).
///
/// The write-first filing flow stages a report as `docs/requests/BUG-<slug>.md`
/// before pushing it to GitHub, and the durable fallback deliberately keeps that
/// file when `gh` or `issues.repo` is unavailable. Nothing then notices the file
/// again: 13 reports pooled there for weeks. Filing is recall-based today; this
/// makes the leftover state deterministic, per the codemap/project-overview
/// detector pattern.
///
/// Scans the `docs/requests/` root only — `completed/` is the archive and never
/// counts. Returns `None` when nothing is staged, which is the common case.
pub fn build_session_start_unfiled_reports_banner_sized(
    cas_root: &Path,
) -> Option<SessionStartBanner> {
    let repo_root = cas_root.parent()?;
    let staged = staged_request_reports(repo_root);
    if staged.is_empty() {
        return None;
    }
    Some(render_unfiled_reports_banner(&staged))
}

/// File names of staged `BUG-*` / `FEATURE-*` reports directly under
/// `<repo_root>/docs/requests/`, sorted. Sub-directories (notably `completed/`)
/// are ignored — the archive is not a backlog.
pub(crate) fn staged_request_reports(repo_root: &Path) -> Vec<String> {
    let dir = repo_root.join("docs").join("requests");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("BUG-") || name.starts_with("FEATURE-"))
        .collect();
    names.sort();
    names
}

/// Pure renderer for the unfiled-staged-reports banner — separated from the
/// directory scan so tests and the SessionStart budget test drive it directly.
pub(crate) fn render_unfiled_reports_banner(staged: &[String]) -> SessionStartBanner {
    let total = staged.len();
    let remediation = if total == 1 {
        "File it now: `gh issue create --repo \"$(cas config get issues.repo)\" \
         --title \"<title>\" --body-file docs/requests/<file>`, then remove the staged file \
         once the issue URL is known (see the cas-supervisor filing-cas-bugs reference)."
    } else {
        "Run the cas-github-issues sweep skill to file and reconcile them; each file is \
         removed only after `gh issue create` succeeds and the issue URL is known."
    };
    let compact = format!(
        "⚠ {total} staged bug/feature report(s) in docs/requests/ were never filed to GitHub. \
         {remediation}\n"
    );

    let mut full = format!(
        "⚠ {total} staged bug/feature report(s) sit unfiled in docs/requests/ — written by the \
         write-first flow but never pushed to the issue tracker, so no one outside this \
         checkout can see them:\n"
    );
    for name in staged.iter().take(UNFILED_REPORTS_BANNER_MAX_ENTRIES) {
        full.push_str(&format!("  ! docs/requests/{name}\n"));
    }
    if total > UNFILED_REPORTS_BANNER_MAX_ENTRIES {
        full.push_str(&format!(
            "  ... and {} more\n",
            total - UNFILED_REPORTS_BANNER_MAX_ENTRIES
        ));
    }
    full.push_str(&format!("  {remediation}\n"));

    SessionStartBanner { full, compact }
}

/// Unset `[issues] repo` in a project that stages requests (cas-20f27).
///
/// Without a target the filing flow has nowhere to push, so every report takes
/// the durable-fallback path and silently accumulates. Fires only when the
/// project shows it wants the flow (a `docs/requests/` directory exists), and
/// deliberately never proposes a value: `origin` in a downstream project is the
/// consumer's own repo, and guessing routes Cassy bugs into the wrong tracker
/// (filing-cas-bugs.md rule).
pub fn build_session_start_issues_target_banner_sized(
    cas_root: &Path,
    config: &crate::config::Config,
) -> Option<SessionStartBanner> {
    let configured = config
        .issues
        .as_ref()
        .and_then(|issues| issues.repo.as_deref())
        .map(str::trim)
        .filter(|repo| !repo.is_empty());
    if configured.is_some() {
        return None;
    }
    let repo_root = cas_root.parent()?;
    if !repo_root.join("docs").join("requests").is_dir() {
        return None;
    }
    Some(render_issues_target_banner())
}

/// Pure renderer for the unset-issues-target banner. Takes no arguments on
/// purpose: there is nothing project-specific to interpolate, and in particular
/// no origin-derived suggestion.
pub(crate) fn render_issues_target_banner() -> SessionStartBanner {
    let text = "⚠ This project stages requests in docs/requests/ but `[issues] repo` is unset, \
                so bug/feature reports have nowhere to be filed. Set the tracker the receiving \
                team gave you — `cas config set issues.repo owner/repo`. Do not derive it from \
                the `origin` remote.\n"
        .to_string();
    SessionStartBanner {
        full: text.clone(),
        compact: text,
    }
}

/// Full + compact renderings of the orphan/stale-server banner (cas-b114).
pub fn build_session_start_orphan_banner_sized(cas_root: &Path) -> Option<SessionStartBanner> {
    let report = crate::ui::factory::orphan_gc::scan(
        cas_root,
        &live_factory_session_names(),
        &live_worker_pgids(cas_root),
        &std::collections::HashSet::new(),
    );
    if report.is_empty() {
        return None;
    }
    Some(render_orphan_banner(&report))
}

/// Pure renderer for the orphan banner — separated from the process scan so the
/// SessionStart budget test (cas-b114) can drive a worst-case report.
pub(crate) fn render_orphan_banner(
    report: &crate::ui::factory::orphan_gc::OrphanReport,
) -> SessionStartBanner {
    let prefix = crate::harness_policy::own_tool_prefix();
    let ports = report.squatted_ports();
    let mut out = format!(
        "⚠ Leftovers from earlier sessions: {} orphan process(es) in worktrees, \
         {} stale server registration(s){}.\n",
        report.processes.len(),
        report.servers.len(),
        if ports.is_empty() {
            String::new()
        } else {
            format!(
                " — holding port(s) {}",
                ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );

    // Compact form: the header (counts + squatted ports) plus the remediation
    // commands, with the per-orphan rows dropped. `gc_report` reproduces them.
    let compact = format!(
        "{out}Adopt or kill BEFORE binding these ports: `{prefix}coordination action=gc_report` \
         lists them; reclaim with `{prefix}coordination action=gc_cleanup force=true \
         dry_run=false`.\n"
    );

    let shown: Vec<String> = report
        .processes
        .iter()
        .map(|p| {
            format!(
                "  [process] pid {} ({}) — {}\n",
                p.pid,
                p.comm,
                p.disposition.label()
            )
        })
        .chain(report.servers.iter().map(|s| {
            format!(
                "  [server ] {} pid {} (session {}) — {}\n",
                s.record.name,
                s.record.pid,
                s.record.factory_session.as_deref().unwrap_or("none"),
                s.disposition.label()
            )
        }))
        .collect();
    let total = shown.len();
    for line in shown.iter().take(ORPHAN_BANNER_MAX_ENTRIES) {
        out.push_str(line);
    }
    if total > ORPHAN_BANNER_MAX_ENTRIES {
        out.push_str(&format!(
            "  ... and {} more\n",
            total - ORPHAN_BANNER_MAX_ENTRIES
        ));
    }
    out.push_str(&format!(
        "\nAdopt or kill BEFORE starting work that binds these ports: review with \
         `{prefix}coordination action=gc_report`, then reclaim with \
         `{prefix}coordination action=gc_cleanup force=true dry_run=false`. \
         Servers registered `shared` are left alone by design.\n"
    ));
    SessionStartBanner { full: out, compact }
}

fn live_factory_session_names() -> std::collections::HashSet<String> {
    crate::ui::factory::SessionManager::new()
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.is_running)
        .map(|session| session.name)
        .collect()
}

fn live_worker_pgids(cas_root: &Path) -> std::collections::HashSet<u32> {
    crate::ui::factory::process_groups::list(cas_root)
        .unwrap_or_default()
        .into_iter()
        .filter(crate::ui::factory::process_groups::is_live)
        .map(|record| record.pgid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init_repo(dir: &Path) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["init", "-q", "-b", "main"])
            .status();
        // Minimal identity so commits don't fail.
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "test@example.com"])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "test"])
            .status();
    }

    #[test]
    fn current_state_snapshot_upserts_head_epics_merges_fleet_and_release_receipt() {
        let project = tempfile::tempdir().unwrap();
        let repo = project.path();
        init_repo(repo);
        fs::write(repo.join("README.md"), "snapshot fixture").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", "README.md"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-qm", "snapshot fixture"])
            .status()
            .unwrap();

        let cas_root = crate::store::init_cas_dir(repo).unwrap();
        let tasks = crate::store::open_task_store(&cas_root).unwrap();
        let mut epic = cas_types::Task::new("cas-epic".to_string(), "Snapshot epic".to_string());
        epic.task_type = TaskType::Epic;
        tasks.add(&epic).unwrap();
        let mut child = cas_types::Task::new("cas-child".to_string(), "Pending merge".to_string());
        child.status = TaskStatus::AwaitingMerge;
        tasks
            .create_atomic(&child, &[], Some(&epic.id), Some("test"))
            .unwrap();

        let agents = crate::store::open_agent_store(&cas_root).unwrap();
        let agent = cas_types::Agent::new("agent-1".to_string(), "Snapshot worker".to_string());
        agents.register(&agent).unwrap();

        let releases = repo.join("docs/release-notes");
        fs::create_dir_all(&releases).unwrap();
        fs::write(
            releases.join("2026-08-11-v9.9.9-slack.md"),
            "# v9.9.9\n\n**Status:** POSTED 2026-08-11\n",
        )
        .unwrap();

        write_current_state_snapshot(&cas_root).unwrap();
        // A second run must update the same page rather than creating another.
        write_current_state_snapshot(&cas_root).unwrap();

        let knowledge = SqliteKnowledgeStore::open(&cas_root).unwrap();
        let page = knowledge
            .get_page_by_rel_path(CURRENT_STATE_REL_PATH)
            .unwrap()
            .expect("SessionEnd snapshot page");
        let body = knowledge.read_body(&page.rel_path).unwrap();
        assert_eq!(
            knowledge
                .list_pages()
                .unwrap()
                .iter()
                .filter(|page| page.rel_path == CURRENT_STATE_REL_PATH)
                .count(),
            1
        );
        assert!(body.contains("HEAD:"), "snapshot: {body}");
        assert!(body.contains("cas-epic — Snapshot epic (1 subtasks; 1 open)"));
        assert!(body.contains("cas-child — Pending merge"));
        assert!(body.contains("Snapshot worker (agent-1"));
        assert!(body.contains("2026-08-11-v9.9.9-slack.md"));
    }

    #[test]
    fn porcelain_clean_tree_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Empty repo has no changes.
        let entries = porcelain_status(tmp.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn porcelain_reports_untracked_and_modified() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        // Commit an initial file.
        fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["add", "a.txt"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();

        // Modify committed file and drop an untracked one.
        fs::write(tmp.path().join("a.txt"), "changed").unwrap();
        fs::write(tmp.path().join("b.txt"), "new").unwrap();

        let entries = porcelain_status(tmp.path()).unwrap();
        let untracked = entries.iter().filter(|e| e.is_untracked()).count();
        let modified = entries.iter().filter(|e| !e.is_untracked()).count();
        assert_eq!(untracked, 1);
        assert_eq!(modified, 1);
    }

    #[test]
    fn write_session_end_manifest_appends_to_daily_log() {
        let tmp = tempfile::tempdir().unwrap();
        // cas_root lives *inside* the repo, so repo == cas_root.parent().
        let repo = tmp.path();
        init_repo(repo);
        fs::write(repo.join("leftover.txt"), "oops").unwrap();

        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let path = write_session_end_manifest(
            &cas_root,
            "session-abc",
            Some("lively-pelican-94"),
            Some("worker"),
        )
        .expect("manifest written");

        assert!(path.exists());
        let contents = fs::read_to_string(&path).unwrap();
        let event: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(event["event"], "session_end");
        assert_eq!(event["factory_session"], "session-abc");
        assert_eq!(event["agent"], "lively-pelican-94");
        assert_eq!(event["role"], "worker");
        assert_eq!(event["git_status"], "dirty");
        assert_eq!(event["git_status_total"], "1");
        assert_eq!(event["git_status_untracked"], "1");
        assert!(
            !contents.contains("leftover.txt"),
            "session-end summary must not dump individual paths"
        );
    }

    #[test]
    fn structured_event_writer_appends_greppable_json_line() {
        let tmp = tempfile::tempdir().unwrap();
        let cas_root = tmp.path().join(".cas");
        let path = append_factory_session_event_with_context(
            &cas_root,
            "task_started",
            "factory-session-abc",
            Some("worker-a"),
            Some("worker"),
            &[("task_id", "cas-beb0"), ("assignee", "worker-a")],
        )
        .expect("event written");

        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.contains(r#""event":"task_started""#));
        assert!(contents.contains(r#""task_id":"cas-beb0""#));
        assert!(contents.contains(r#""assignee":"worker-a""#));
        serde_json::from_str::<serde_json::Value>(contents.trim()).expect("valid JSON line");
    }

    #[test]
    fn session_end_summarizes_large_git_status_without_path_dump() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        for index in 0..125 {
            fs::write(repo.join(format!("untracked-{index:03}.txt")), "wip").unwrap();
        }
        let cas_root = repo.join(".cas");

        let log = write_session_end_manifest(&cas_root, "sess-large", None, None)
            .expect("summary written");
        let body = fs::read_to_string(log).unwrap();
        let event: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(event["git_status_total"], "125");
        assert_eq!(event["git_status_untracked"], "125");
        assert_eq!(body.lines().count(), 1, "one bounded JSON event expected");
        assert!(
            !body.contains("untracked-124.txt"),
            "individual git status paths must not be dumped"
        );
        assert!(body.len() < 1_000, "summary should remain compact");
    }

    #[test]
    fn wip_candidates_surfaces_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        fs::write(repo.join("wip.rs"), "// todo").unwrap();

        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let summary = wip_candidates(&cas_root).expect("summary");
        assert!(!summary.is_clean());
        assert_eq!(summary.untracked_count(), 1);
        assert_eq!(summary.modified_count(), 0);
    }

    /// Table-drive `label()` across every documented porcelain code so a silent
    /// rename of an arm (e.g. 'modified-staged' → 'staged') fails loudly.
    #[test]
    fn porcelain_entry_label_covers_documented_codes() {
        let cases: &[(&str, &str)] = &[
            ("??", "untracked"),
            (" M", "modified"),
            ("M ", "modified-staged"),
            ("MM", "modified-staged"),
            ("AM", "modified-staged"),
            ("A ", "added"),
            ("D ", "deleted"),
            (" D", "deleted"),
            ("R ", "changed"), // Rename falls through today; guard arm.
            ("UU", "changed"), // Unmerged falls through today.
        ];
        for (code, expected) in cases {
            let entry = PorcelainEntry {
                status: (*code).to_string(),
                path: "x".into(),
            };
            assert_eq!(
                entry.label(),
                *expected,
                "label mismatch for porcelain code {code:?}"
            );
        }
    }

    /// Multiple `write_session_end_manifest` calls in the same day must append
    /// rather than overwrite — the daily log is the cross-session breadcrumb
    /// trail; losing history silently defeats the feature.
    #[test]
    fn manifest_is_append_only_across_multiple_sessions_same_day() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let p1 = write_session_end_manifest(&cas_root, "sess-one", None, None)
            .expect("first manifest");
        let p2 = write_session_end_manifest(&cas_root, "sess-two", Some("worker-b"), Some("worker"))
            .expect("second manifest");
        assert_eq!(p1, p2, "same daily log path expected");

        let body = fs::read_to_string(&p1).unwrap();
        let events: Vec<serde_json::Value> = body
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2, "each session end is one JSON line");
        assert_eq!(events[0]["factory_session"], "sess-one");
        assert_eq!(events[1]["factory_session"], "sess-two");
        assert_eq!(events[0]["agent"], "unknown");
        assert_eq!(events[0]["role"], "unknown");
        assert_eq!(events[1]["agent"], "worker-b");
        assert_eq!(events[1]["role"], "worker");
    }

    /// A clean worktree records `git_status=clean` so audits can tell the
    /// difference between "nothing was wrong" and "manifest never wrote".
    #[test]
    fn manifest_records_clean_tree_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        // Commit so the tree is fully clean (empty repo also counts, but
        // committing exercises the "tree exists + clean" code path).
        fs::write(repo.join(".gitkeep"), "").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", ".gitkeep"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();

        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let log = write_session_end_manifest(&cas_root, "sess-clean", None, None)
            .expect("manifest written");
        let body = fs::read_to_string(&log).unwrap();
        let event: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert!(
            event["git_status"] == "clean" && event["git_status_total"] == "0",
            "clean worktree should be recorded, got: {body}"
        );
    }

    /// When `cas_root` lives under a linked worktree (the factory layout:
    /// `<repo>/.cas/worktrees/<name>/.cas`), `main_worktree_path` must resolve
    /// to the main repo — not the linked worktree — otherwise the hygiene
    /// manifest attributes the worker's own WIP as "main worktree" and
    /// inverts the supervisor triage promise (cas-a9ab adversarial finding).
    #[test]
    fn main_worktree_path_resolves_to_main_repo_from_linked_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        fs::create_dir_all(&main).unwrap();
        init_repo(&main);
        // A commit is required so the repo has HEAD before linking a worktree.
        fs::write(main.join("seed.txt"), "").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["add", "seed.txt"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["commit", "-q", "-m", "seed"])
            .status()
            .unwrap();

        let linked = tmp.path().join("linked");
        let status = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add", "-b", "feature"])
            .arg(&linked)
            .status()
            .unwrap();
        assert!(status.success(), "git worktree add must succeed for this test");

        // Worker-style layout: cas_root is <linked>/.cas.
        let linked_cas = linked.join(".cas");
        fs::create_dir_all(&linked_cas).unwrap();

        let resolved = main_worktree_path(&linked_cas).expect("main path resolved");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            main.canonicalize().unwrap(),
            "linked-worktree cas_root must resolve upward to the main repo, got {resolved:?}"
        );
    }

    /// extract_task_id must pick the first canonical `cas-xxxx` (4 hex) token
    /// and reject non-hex / too-long / non-terminated variants so a commit
    /// subject like "refactor cas-module" does not falsely attribute.
    #[test]
    fn extract_task_id_accepts_canonical_and_rejects_garbage() {
        // Happy path: first 4-hex token wins, case-insensitive, boundary aware.
        assert_eq!(
            extract_task_id("fix(foo): ship cas-a9ab and follow-up cas-4181"),
            Some("cas-a9ab".to_string())
        );
        assert_eq!(
            extract_task_id("CAS-4181 uppercase"),
            Some("cas-4181".to_string())
        );
        assert_eq!(
            extract_task_id("see cas-d0f9."),
            Some("cas-d0f9".to_string())
        );

        // Non-hex characters in the 4-char window are rejected.
        assert_eq!(extract_task_id("cas-zzzz is fake"), None);
        // Too-long hex run (> 4 chars without a boundary) is rejected.
        assert_eq!(extract_task_id("cas-a9abc is nope"), None);
        // Non-boundary alphanumeric (e.g. cas-module) is rejected.
        assert_eq!(extract_task_id("refactor cas-module"), None);
        assert_eq!(extract_task_id("nothing here"), None);
    }

    /// attribute_file_to_task must resolve a modified file to the cas-id in
    /// the last commit that touched it. Confirms the core attribution primitive
    /// used by the SessionStart banner.
    #[test]
    fn attribute_file_to_task_finds_cas_id_in_last_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        fs::write(repo.join("a.txt"), "v1").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", "a.txt"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "-m", "feat(a): initial ship (cas-a9ab)"])
            .status()
            .unwrap();

        assert_eq!(
            attribute_file_to_task(repo, "a.txt"),
            Some("cas-a9ab".to_string())
        );

        // An untracked file has no git history → None.
        fs::write(repo.join("new.txt"), "").unwrap();
        assert_eq!(attribute_file_to_task(repo, "new.txt"), None);

        // Commit without a cas-id still returns None (no false positives).
        fs::write(repo.join("b.txt"), "").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", "b.txt"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "-m", "chore(b): no task id here"])
            .status()
            .unwrap();
        assert_eq!(attribute_file_to_task(repo, "b.txt"), None);
    }

    /// On a pathological dirty tree (hundreds of files) the banner must
    /// cap its inline rows and direct the supervisor to `gc_report` for
    /// the overflow. Guards against SessionStart latency regressions —
    /// every rendered row spawns `git log -1`, so an uncapped banner
    /// stalls session boot (cas-aeec adversarial P1).
    #[test]
    fn build_session_start_wip_banner_caps_rows_on_large_wip_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        // Untracked files don't need committing — faster than seeding
        // real history and exercises the 'unattributed' path which
        // still must be capped.
        let total = WIP_BANNER_MAX_ENTRIES + 7;
        for i in 0..total {
            fs::write(repo.join(format!("wip_{i:03}.tmp")), "").unwrap();
        }
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let banner =
            build_session_start_wip_banner(&cas_root).expect("banner for dirty tree");
        // The "[" opens each inline row. Count occurrences.
        let rows = banner.matches('[').count();
        assert_eq!(
            rows, WIP_BANNER_MAX_ENTRIES,
            "banner must cap inline rows at WIP_BANNER_MAX_ENTRIES, got {rows}"
        );
        assert!(
            banner.contains(&format!("and {} more", total - WIP_BANNER_MAX_ENTRIES)),
            "banner must announce the overflow count"
        );
        assert!(
            banner.contains("gc_report"),
            "overflow line must direct supervisor to gc_report"
        );
    }

    /// The SessionStart banner must surface the attribution line and the
    /// triage instruction; returning None on a clean tree prevents noise.
    #[test]
    fn build_session_start_wip_banner_renders_attribution_and_skips_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);

        // Seed + commit + modify: modified entry should carry attribution.
        fs::write(repo.join("src.rs"), "v1").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", "src.rs"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "-m", "feat: ship (cas-4181)"])
            .status()
            .unwrap();
        fs::write(repo.join("src.rs"), "v2").unwrap();
        // Also drop an untracked file to exercise the unattributed branch.
        fs::write(repo.join("scratch.tmp"), "").unwrap();

        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let banner = build_session_start_wip_banner(&cas_root)
            .expect("banner rendered for dirty worktree");
        assert!(banner.contains("Prior-factory WIP detected"));
        assert!(banner.contains("src.rs"));
        assert!(banner.contains("(last touched by cas-4181)"));
        assert!(banner.contains("scratch.tmp"));
        assert!(banner.contains("unattributed"));
        assert!(banner.contains("Triage BEFORE spawning workers"));

        // Clean the tree → banner suppressed, no noise on normal sessions.
        fs::remove_file(repo.join("scratch.tmp")).unwrap();
        fs::write(repo.join("src.rs"), "v1").unwrap();
        assert!(
            build_session_start_wip_banner(&cas_root).is_none(),
            "clean tree must not emit a banner"
        );
    }

    /// cas-b7dd (GH #88): a session starting with a leftover registration from
    /// a dead session must be told, with the port named and an adopt-or-kill
    /// instruction — the alternative is inheriting it silently and meeting it
    /// as EADDRINUSE later.
    #[test]
    fn orphan_banner_names_dead_session_leftovers_and_never_kills() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        // A registration owned by a session that is not running, whose process
        // is this test process (so it reads as genuinely live).
        let self_pid = std::process::id();
        let record = crate::ui::factory::server_registry::RegisteredServer {
            id: "srv-banner".to_string(),
            name: "vite".to_string(),
            command: "npm run dev".to_string(),
            cwd: cas_root.clone(),
            pid: self_pid,
            pgid: None,
            pid_starttime: crate::mcp::daemon::read_pid_starttime(self_pid),
            expected_port: Some(5173),
            owner_task: None,
            owner_worker: None,
            factory_session: Some("session-that-died".to_string()),
            shared: false,
            cgroup: None,
            log_path: None,
            started_at: chrono::Utc::now(),
            state: crate::ui::factory::server_registry::ServerState::Running,
            ended_at: None,
            ended_detail: None,
        };
        crate::ui::factory::server_registry::write_record(&cas_root, &record).unwrap();

        let banner = build_session_start_orphan_banner(&cas_root)
            .expect("leftover from a dead session must be surfaced");
        assert!(banner.contains("vite"), "banner: {banner}");
        assert!(banner.contains("session-that-died"), "banner: {banner}");
        assert!(banner.contains("Adopt or kill"), "banner: {banner}");
        assert!(
            banner.contains("gc_cleanup"),
            "the banner must name the reclaim command: {banner}"
        );

        // Visibility only: the banner must not have signalled anything.
        assert!(
            crate::mcp::daemon::pid_alive(self_pid),
            "building a banner must never kill"
        );
        assert!(
            crate::ui::factory::server_registry::find(&cas_root, &record.id)
                .unwrap()
                .is_some(),
            "and must never clear records"
        );
    }

    /// cas-20f27 detector 1: staged reports at the `docs/requests/` root are
    /// invisible to everyone outside the checkout. The banner must name the
    /// count and the remediation skill; an empty or archive-only directory must
    /// stay silent so the detector never cries wolf.
    #[test]
    fn unfiled_reports_banner_counts_staged_reports_and_ignores_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let requests = repo.join("docs").join("requests");
        fs::create_dir_all(requests.join("completed")).unwrap();

        // Empty root (README + completed/ only) → silent.
        fs::write(requests.join("README.md"), "# intake").unwrap();
        fs::write(
            requests.join("completed").join("BUG-already-filed.md"),
            "archived",
        )
        .unwrap();
        assert!(
            build_session_start_unfiled_reports_banner_sized(&cas_root).is_none(),
            "archived reports are not a backlog — the banner must stay quiet"
        );

        // Stage two reports at the root.
        fs::write(requests.join("BUG-close-hangs.md"), "report").unwrap();
        fs::write(requests.join("FEATURE-detectors.md"), "report").unwrap();

        let banner = build_session_start_unfiled_reports_banner_sized(&cas_root)
            .expect("staged reports must be surfaced");
        assert!(banner.full.contains("2 staged"), "banner: {}", banner.full);
        assert!(banner.full.contains("docs/requests/BUG-close-hangs.md"));
        assert!(banner.full.contains("docs/requests/FEATURE-detectors.md"));
        assert!(
            banner.full.contains("cas-github-issues"),
            "the banner must name the remediation skill: {}",
            banner.full
        );
        assert!(banner.compact.contains("2 staged"));
        assert!(banner.compact.contains("cas-github-issues"));
        assert!(
            !banner.full.contains("BUG-already-filed"),
            "completed/ must never be scanned: {}",
            banner.full
        );

        // Condition clears → banner goes quiet.
        fs::remove_file(requests.join("BUG-close-hangs.md")).unwrap();
        fs::remove_file(requests.join("FEATURE-detectors.md")).unwrap();
        assert!(
            build_session_start_unfiled_reports_banner_sized(&cas_root).is_none(),
            "the banner must disappear once the reports are filed"
        );
    }

    /// A single staged report gets the single-file filing command rather than
    /// the sweep skill, and a large backlog caps its inline rows.
    #[test]
    fn unfiled_reports_banner_scales_from_one_report_to_a_capped_list() {
        let one = render_unfiled_reports_banner(&["BUG-only.md".to_string()]);
        assert!(one.full.contains("1 staged"));
        assert!(
            one.full.contains("gh issue create"),
            "a single report gets the direct filing command: {}",
            one.full
        );

        let many: Vec<String> = (0..UNFILED_REPORTS_BANNER_MAX_ENTRIES + 3)
            .map(|i| format!("BUG-report-{i:03}.md"))
            .collect();
        let banner = render_unfiled_reports_banner(&many);
        let rows = banner.full.matches("  ! docs/requests/").count();
        assert_eq!(rows, UNFILED_REPORTS_BANNER_MAX_ENTRIES);
        assert!(banner.full.contains("and 3 more"), "{}", banner.full);
        assert!(banner.full.contains("cas-github-issues"));
    }

    /// cas-20f27 detector 2: an unset `issues.repo` in a project that stages
    /// requests must be surfaced with the literal config command — and must
    /// never propose a value derived from the `origin` remote, which in a
    /// downstream project is the consumer's own tracker.
    #[test]
    fn issues_target_banner_fires_only_when_unset_and_never_suggests_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        // A configured remote that must not leak into the banner.
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/downstream-consumer/private-app.git",
            ])
            .status();
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let unset = crate::config::Config::default();

        // No docs/requests/ → the project does not use the flow → silent.
        assert!(
            build_session_start_issues_target_banner_sized(&cas_root, &unset).is_none(),
            "projects that do not stage requests must not be nagged"
        );

        fs::create_dir_all(repo.join("docs").join("requests")).unwrap();
        let banner = build_session_start_issues_target_banner_sized(&cas_root, &unset)
            .expect("unset target with a requests dir must be surfaced");
        let full = banner.full.as_str();
        assert!(
            full.contains("cas config set issues.repo owner/repo"),
            "banner must name the exact command: {full}"
        );
        assert!(
            !banner.full.contains("downstream-consumer")
                && !banner.full.contains("private-app")
                && !banner.compact.contains("downstream-consumer"),
            "the banner must never derive a value from origin: {}",
            banner.full
        );

        // Set → silent. Whitespace-only is treated as unset.
        let mut set = crate::config::Config::default();
        set.issues = Some(crate::config::IssuesConfig {
            repo: Some("owner/cas".to_string()),
            ..crate::config::IssuesConfig::default()
        });
        assert!(
            build_session_start_issues_target_banner_sized(&cas_root, &set).is_none(),
            "a configured target must silence the banner"
        );

        let mut blank = crate::config::Config::default();
        blank.issues = Some(crate::config::IssuesConfig {
            repo: Some("   ".to_string()),
            ..crate::config::IssuesConfig::default()
        });
        assert!(
            build_session_start_issues_target_banner_sized(&cas_root, &blank).is_some(),
            "a blank repo value is not a target"
        );
    }

    #[test]
    fn orphan_banner_is_silent_when_there_is_nothing_to_report() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        assert!(
            build_session_start_orphan_banner(&cas_root).is_none(),
            "no leftovers must mean no banner — this fires on every session start"
        );
    }
}
