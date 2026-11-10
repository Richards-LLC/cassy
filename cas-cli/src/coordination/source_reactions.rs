//! Receipted source-post status, reconciled by the owning supervisor.
use super::external_work::SqliteRequestJournal;
use crate::cli::violet_hub::{VioletError, decode_tool_result};
use cas_types::{AgentRole, AgentStatus, MergeCiAdmission, Task, TaskStatus};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

const MARKS: [&str; 3] = ["eyes", "pick", "white_check_mark"];
const REFRESH_SECONDS: i64 = 60;
const LEASE_SECONDS: i64 = 90;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: String,
    pub tasks: Vec<String>,
    pub desired: Option<String>,
    pub owner: Option<String>,
    pub observed: Vec<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub pending: bool,
    pub error: Option<Value>,
    pub next_attempt: Option<DateTime<Utc>>,
}

#[derive(Clone)]
struct Target {
    source: String,
    channel: String,
    message: String,
}

impl Target {
    fn parse(source: &str) -> Option<Self> {
        let url = url::Url::parse(source.trim()).ok()?;
        let parts: Vec<_> = url.path_segments()?.collect();
        if url.scheme() != "https"
            || !url.host_str()?.ends_with(".slack.com")
            || parts.len() != 3
            || parts[0] != "archives"
            || parts[1].is_empty()
        {
            return None;
        }
        let digits = parts[2].strip_prefix('p')?;
        if digits.len() <= 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let split = digits.len() - 6;
        Some(Self {
            source: format!(
                "https://{}/archives/{}/p{digits}",
                url.host_str()?,
                parts[1]
            ),
            channel: parts[1].into(),
            message: format!("{}.{}", &digits[..split], &digits[split..]),
        })
    }
}

/// A green task merge must actually be contained in the remote staging/main
/// history. Closing or landing on an epic branch alone is insufficient.
fn staged(root: &Path, task: &Task) -> bool {
    if task.status != TaskStatus::Closed
        || task.deliverables.merge_ci_admission.unwrap_or_default() != MergeCiAdmission::Green
    {
        return false;
    }
    let (Some(merge), Some(source), Some(target)) = (
        task.deliverables.green_merge_commit.as_deref(),
        task.deliverables.factory_branch_anchor.as_deref(),
        task.deliverables.work_target.as_ref(),
    ) else {
        return false;
    };
    // Resolve the task's own repository; never use the daemon's checkout for
    // another project's delivery or fall back after a stale binding refusal.
    let Ok(repo) = crate::mcp::tools::core::task::repo_context::resolve_repo_context(root, target)
    else {
        return false;
    };
    let deadline = crate::bounded_process::Deadline::after(std::time::Duration::from_secs(3));
    let contains = |ancestor: &str, descendant: &str| {
        crate::bounded_process::run_command(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo.repo_root)
                .args(["merge-base", "--is-ancestor", ancestor, descendant]),
            deadline,
            std::time::Duration::from_secs(1),
        )
        .is_ok_and(|out| out.status.success())
    };
    contains(source, merge)
        && ["refs/remotes/origin/staging", "refs/remotes/origin/main"]
            .iter()
            .any(|reference| contains(merge, reference))
}

fn sources(root: &Path) -> anyhow::Result<BTreeMap<String, (Target, Vec<Task>)>> {
    let mut grouped: BTreeMap<String, (Target, Vec<Task>)> = BTreeMap::new();
    for task in crate::store::open_task_store(root)?.list(None)? {
        if let Some(target) = task.external_ref.as_deref().and_then(Target::parse) {
            grouped
                .entry(target.source.clone())
                .or_insert_with(|| (target, Vec::new()))
                .1
                .push(task);
        }
    }
    Ok(grouped)
}

fn desired(root: &Path, tasks: &[Task], prove_staging: bool) -> Option<String> {
    let tasks: Vec<_> = tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Cancelled)
        .collect();
    if tasks.is_empty() {
        return None;
    }
    Some(
        if prove_staging && tasks.iter().all(|task| staged(root, task)) {
            "white_check_mark"
        } else if tasks.iter().all(|task| task.status == TaskStatus::Open) {
            "eyes"
        } else {
            "pick"
        }
        .into(),
    )
}

struct Journal {
    conn: Arc<Mutex<Connection>>,
}

impl Journal {
    fn open(root: &Path) -> anyhow::Result<Self> {
        let conn = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
        conn.lock().map_err(|_| anyhow::anyhow!("source status database poisoned"))?.execute_batch(
            "CREATE TABLE IF NOT EXISTS source_reactions (
                source TEXT PRIMARY KEY, snapshot TEXT NOT NULL, next_attempt INTEGER NOT NULL DEFAULT 0,
                permanent INTEGER NOT NULL DEFAULT 0, lease_until INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS source_reaction_receipts (
                id INTEGER PRIMARY KEY, source TEXT NOT NULL, at TEXT NOT NULL,
                request TEXT NOT NULL, receipt TEXT NOT NULL);"
        )?;
        Ok(Self { conn })
    }

    fn snapshots(&self) -> anyhow::Result<BTreeMap<String, SourceStatus>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("source status database poisoned"))?;
        let mut stmt = conn.prepare("SELECT source, snapshot FROM source_reactions")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (source, data) = row?;
            Ok((source, serde_json::from_str(&data)?))
        })
        .collect()
    }

    fn save(&self, status: &SourceStatus, permanent: bool) -> anyhow::Result<()> {
        self.conn.lock().map_err(|_| anyhow::anyhow!("source status database poisoned"))?.execute(
            "UPDATE source_reactions SET snapshot=?2, next_attempt=?3, permanent=?4, lease_until=0 WHERE source=?1",
            params![status.source, serde_json::to_string(status)?, status.next_attempt.map(|at| at.timestamp_millis()).unwrap_or(0), permanent],
        )?;
        Ok(())
    }

    fn receipt(
        &self,
        source: &str,
        now: DateTime<Utc>,
        request: &Value,
        receipt: &Value,
    ) -> anyhow::Result<()> {
        self.conn.lock().map_err(|_| anyhow::anyhow!("source status database poisoned"))?.execute(
            "INSERT INTO source_reaction_receipts(source,at,request,receipt) VALUES (?1,?2,?3,?4)",
            params![source, now.to_rfc3339(), request.to_string(), receipt.to_string()],
        )?;
        Ok(())
    }
}

fn current_status(
    root: &Path,
    journal: &SqliteRequestJournal,
    target: &Target,
    tasks: &[Task],
    prove_staging: bool,
) -> anyhow::Result<SourceStatus> {
    let project = crate::cloud::resolve_canonical_id(root).unwrap_or_default();
    // Keep the original link for thread_ts when a source is a reply in a
    // managed thread. Exact-post grouping intentionally ignores query aliases.
    let source = tasks
        .iter()
        .find_map(|t| t.external_ref.as_deref())
        .unwrap_or(&target.source);
    let owner = journal.source_status_owner(source, &project)?;
    Ok(SourceStatus {
        source: target.source.clone(),
        tasks: tasks.iter().map(|task| task.id.clone()).collect(),
        desired: desired(root, tasks, prove_staging),
        owner,
        observed: Vec::new(),
        observed_at: None,
        pending: true,
        error: None,
        next_attempt: None,
    })
}

/// Local audit: expected state is recomputed now, observed state is explicitly
/// timestamped. Never infer a Slack read or successful update from a task close.
pub fn audit(root: &Path, retry: bool) -> anyhow::Result<Vec<SourceStatus>> {
    audit_at(root, retry, Utc::now())
}

fn audit_at(root: &Path, retry: bool, now: DateTime<Utc>) -> anyhow::Result<Vec<SourceStatus>> {
    let state = Journal::open(root)?;
    if retry {
        state
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("source status database poisoned"))?
            .execute(
                "UPDATE source_reactions SET permanent=0,next_attempt=0 WHERE permanent=1",
                [],
            )?;
    }
    let old = state.snapshots()?;
    let authority = SqliteRequestJournal::open(root)?;
    sources(root)?
        .into_values()
        .map(|(target, tasks)| {
            let mut current = current_status(root, &authority, &target, &tasks, true)?;
            if let Some(previous) = old.get(&target.source) {
                let changed = current.desired != previous.desired
                    || current.owner != previous.owner
                    || current.tasks != previous.tasks;
                current.observed = previous.observed.clone();
                current.observed_at = previous.observed_at;
                current.pending = previous.pending
                    || changed
                    || previous.observed_at.is_none_or(|at| {
                        now.signed_duration_since(at)
                            > chrono::Duration::seconds(REFRESH_SECONDS * 2)
                    });
                current.error = previous.error.clone();
                current.next_attempt = previous.next_attempt;
            }
            Ok(current)
        })
        .collect()
}

pub(crate) fn reconcile_with_clock(
    root: &Path,
    registration: &str,
    session: &str,
    clock: &dyn Fn() -> DateTime<Utc>,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, VioletError>,
) -> anyhow::Result<()> {
    let now = clock();
    let agents = crate::store::open_agent_store(root)?;
    let agent = agents.get(registration)?;
    if agent.role != AgentRole::Supervisor
        || agent.factory_session.as_deref() != Some(session)
        || !matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
    {
        return Ok(());
    }
    let state = Journal::open(root)?;
    let authority = SqliteRequestJournal::open(root)?;
    let previous = state.snapshots()?;
    let mut due = Vec::new();
    for (target, tasks) in sources(root)?.into_values() {
        let current = current_status(root, &authority, &target, &tasks, false)?;
        if current.owner.as_deref() != Some(registration) {
            continue;
        }
        let changed = previous
            .get(&target.source)
            .is_none_or(|old| old.owner != current.owner);
        let conn = state
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("source status database poisoned"))?;
        conn.execute(
            "INSERT OR IGNORE INTO source_reactions(source,snapshot) VALUES (?1,?2)",
            params![target.source, serde_json::to_string(&current)?],
        )?;
        // New ownership may retry a refusal, but cannot shorten Retry-After
        // or another sender's lease. Prove staging only for admitted sources;
        // the remaining backlog does no Git work on this tick.
        if changed {
            conn.execute(
                "UPDATE source_reactions SET permanent=0 WHERE source=?1",
                [&target.source],
            )?;
        }
        let next: Option<i64> = conn.query_row(
            "SELECT next_attempt FROM source_reactions WHERE source=?1 AND permanent=0 AND next_attempt<=?2 AND lease_until<=?2",
            params![target.source, now.timestamp_millis()], |row| row.get(0),
        ).optional()?;
        if let Some(next) = next {
            due.push((next, target, tasks, current));
        }
    }
    due.sort_by_key(|(next, _, _, _)| *next);
    for (_, target, tasks, mut status) in due.into_iter().take(2) {
        let now = clock();
        let acquired = state.conn.lock().map_err(|_| anyhow::anyhow!("source status database poisoned"))?.execute(
            "UPDATE source_reactions SET lease_until=?3 WHERE source=?1 AND permanent=0 AND next_attempt<=?2 AND lease_until<=?2",
            params![target.source, now.timestamp_millis(), now.timestamp_millis()+LEASE_SECONDS * 1000],
        )?;
        if acquired == 0 {
            continue;
        }
        status.desired = desired(root, &tasks, true);
        if let Some(old) = previous.get(&target.source) {
            status.observed = old.observed.clone();
            status.observed_at = old.observed_at;
        }
        let result = reconcile_one(
            root,
            &authority,
            &state,
            &target,
            &tasks,
            registration,
            clock,
            &mut status,
            call,
        );
        status.next_attempt = Some(clock() + chrono::Duration::seconds(REFRESH_SECONDS));
        let mut permanent = false;
        match result {
            Ok(()) => {
                status.pending = false;
                status.error = None;
            }
            Err(error) => {
                status.pending = true;
                let envelope = error.to_json();
                // Violet's structured Retry-After is seconds. Keep the full
                // failure envelope and never retry sooner, even after a new
                // lifecycle state replaces this pending mark.
                let retry = [
                    "/error/retry_after",
                    "/error/retry_after_seconds",
                    "/error/detail/retry_after",
                    "/error/detail/retry_after_seconds",
                    "/retry_after",
                ]
                .iter()
                .filter_map(|path| envelope.pointer(path))
                .filter_map(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                .max()
                .unwrap_or(REFRESH_SECONDS)
                .max(1);
                status.next_attempt = Some(
                    chrono::Duration::try_seconds(retry)
                        .and_then(|delay| clock().checked_add_signed(delay))
                        .unwrap_or(DateTime::<Utc>::MAX_UTC),
                );
                permanent = !error.retryable
                    && !matches!(
                        error.code.as_str(),
                        "rate_limited"
                            | "source_state_changed"
                            | "source_state_unavailable"
                            | "receipt_write_failed"
                    );
                status.error = Some(envelope);
            }
        }
        state.save(&status, permanent)?;
    }
    Ok(())
}

use rusqlite::OptionalExtension;

#[allow(clippy::too_many_arguments)]
fn reconcile_one(
    root: &Path,
    authority: &SqliteRequestJournal,
    state: &Journal,
    target: &Target,
    tasks: &[Task],
    registration: &str,
    clock: &dyn Fn() -> DateTime<Utc>,
    status: &mut SourceStatus,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, VioletError>,
) -> Result<(), VioletError> {
    let receipt = decode_tool_result(&call(
        "violet_read",
        json!({
            "channel":target.channel, "message_id":target.message,
            "include_threads":false, "include_files":false, "include_channels":false,
        }),
    )?)?;
    if receipt["ok"] != true {
        return Err(VioletError::from_receipt(receipt));
    }
    let selected = receipt["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|message| message["message_id"] == target.message)
        .ok_or_else(|| {
            VioletError::local("message_missing", "Violet did not return the source post")
        })?;
    if selected["permalink"]
        .as_str()
        .and_then(Target::parse)
        .is_none_or(|read| read.source != target.source)
        || receipt["complete"] != true
    {
        return Err(VioletError::local(
            "identity_mismatch",
            "Violet did not confirm the exact source post",
        ));
    }
    status.observed = selected["reactions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|reaction| reaction["name"].as_str())
        .filter(|name| MARKS.contains(name))
        .map(str::to_string)
        .collect();
    status.observed.sort();
    status.observed.dedup();
    status.observed_at = Some(clock());
    let mut operations: Vec<(&str, String)> = status
        .observed
        .iter()
        .filter(|mark| Some(mark.as_str()) != status.desired.as_deref())
        .map(|mark| ("remove", mark.clone()))
        .collect();
    if let Some(mark) = status
        .desired
        .as_ref()
        .filter(|mark| !status.observed.contains(mark))
    {
        operations.push(("add", mark.clone()));
    }
    let mut foreign_marks = false;
    for (action, mark) in operations {
        // Observe takeover and reverse transitions again before every write,
        // particularly before publishing check after a slow read/removal.
        let latest = sources(root).map_err(|_| {
            VioletError::local("source_state_unavailable", "Cannot read current tasks")
        })?;
        let current_tasks = latest
            .get(&target.source)
            .map(|(_, tasks)| tasks.as_slice())
            .unwrap_or(&[]);
        let current =
            current_status(root, authority, target, current_tasks, true).map_err(|_| {
                VioletError::local("source_state_unavailable", "Cannot read source owner")
            })?;
        if current.owner.as_deref() != Some(registration)
            || current.desired != status.desired
            || current.tasks != status.tasks
            || tasks.is_empty()
        {
            return Err(VioletError::local(
                "source_state_changed",
                "Source owner or task state changed during reconciliation",
            ));
        }
        let request = json!({"kind":"reaction","channel":target.channel,"message_id":target.message,"reaction":mark,"action":action});
        let result =
            call("violet_post", request.clone()).and_then(|response| decode_tool_result(&response));
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                state
                    .receipt(&target.source, clock(), &request, &error.to_json())
                    .map_err(|_| {
                        VioletError::local(
                            "receipt_write_failed",
                            "Cannot preserve reaction failure",
                        )
                    })?;
                return Err(error);
            }
        };
        state
            .receipt(&target.source, clock(), &request, &receipt)
            .map_err(|_| {
                VioletError::local(
                    "receipt_write_failed",
                    "Cannot preserve Violet reaction receipt",
                )
            })?;
        if receipt["ok"] != true {
            return Err(VioletError::from_receipt(receipt));
        }
        if receipt["reaction"]["name"] != mark
            || receipt["reaction"]["action"] != action
            || !receipt["reaction"]["changed"].is_boolean()
        {
            return Err(VioletError::local(
                "invalid_reaction_receipt",
                "Violet did not confirm the requested reaction",
            ));
        }
        // This projection is backed by Violet receipts, not a second cached
        // message body. Keep only status names; audit exposes its timestamp.
        if action == "remove" {
            let count = selected["reactions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|reaction| reaction["name"] == mark)
                .and_then(|reaction| reaction["count"].as_u64())
                .unwrap_or(1);
            if count > 1 || receipt["reaction"]["changed"] != true {
                // Violet removes only its own mark; a human's same-name
                // reaction survives. Do not project the whole name away.
                foreign_marks = true;
            } else {
                status.observed.retain(|name| name != &mark);
            }
        } else {
            status.observed.push(mark);
        }
    }
    if foreign_marks {
        return Err(VioletError::local(
            "foreign_reaction_remains",
            "Other users may retain an obsolete status reaction; inspect the source post",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
