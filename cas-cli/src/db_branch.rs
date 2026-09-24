//! Supervisor-provisioned disposable database branches per task (cas-0033,
//! GH #1005 item 3).
//!
//! A worker that needs a database to reproduce a bug could not get one: the
//! proxy denies it `neon.create_branch`, so the supervisor relayed branch
//! creation and connection strings by hand. Here the supervisor's `cas serve`,
//! which holds the Neon credential, creates `cas-<task-id>-<n>` from a
//! non-production parent, writes the connection string into the worker's
//! worktree as `.env.cas-db` (mode 600, git-excluded), and records the branch in
//! a per-task ledger. Closing or cancelling the task deletes it.
//!
//! The ledger lives at `<cas_root>/db-branches/<task-id>.json`, not in the
//! task row: fleet binaries of different ages share one task store, and an
//! older binary that rewrites a task would silently drop a field it does not
//! know. Each change is also recorded as a task note (never the connection
//! string).
//!
//! This module holds the provider-independent logic, generic over
//! [`NeonBranchApi`] so tests drive it with a stub. The MCP surface and the
//! live proxy-backed client are in `mcp::tools::service::db_branch_ops`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// The env file written into the worker's worktree.
pub const ENV_FILE: &str = ".env.cas-db";
/// At most this many live branches per task.
pub const MAX_LIVE_PER_TASK: usize = 3;
/// A branch is flagged by `gc_report` once it outlives this.
pub const TTL_HOURS: i64 = 72;
/// Compute ceiling for a disposable branch (Neon compute units).
pub const MAX_CU: f64 = 1.0;
/// Skip a failed deletion for this long before the next automatic retry.
pub const RETRY_AFTER_SECS: i64 = 300;

/// Neon branch operations the supervisor's credential allows.
#[allow(async_fn_in_trait)]
pub trait NeonBranchApi {
    async fn list_branches(&self, project_id: &str) -> Result<Vec<NeonBranchInfo>, String>;
    async fn create_branch(
        &self,
        project_id: &str,
        name: &str,
        parent_id: &str,
    ) -> Result<NeonBranchInfo, String>;
    async fn connection_string(&self, project_id: &str, branch_id: &str) -> Result<String, String>;
    async fn delete_branch(&self, project_id: &str, branch_id: &str) -> Result<(), String>;
}

/// One branch as Neon reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonBranchInfo {
    pub id: String,
    pub name: String,
    /// Neon's default (primary) branch: the one a call without a branch id
    /// uses, which is production.
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbBranchState {
    Live,
    /// Its task ended but the deletion has not succeeded yet.
    PendingDelete,
    Deleted,
}

/// A branch this task owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DbBranchRecord {
    pub task_id: String,
    pub name: String,
    pub branch_id: String,
    pub project_id: String,
    pub parent_id: String,
    pub parent_name: String,
    pub worktree: PathBuf,
    pub env_file: PathBuf,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: DbBranchState,
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl DbBranchRecord {
    pub fn is_open(&self) -> bool {
        self.state != DbBranchState::Deleted
    }
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

fn name_prefix(task_id: &str) -> String {
    format!("cas-{task_id}-")
}

/// `cas-<task-id>-<n>`.
pub fn branch_name(task_id: &str, index: u32) -> String {
    format!("{}{index}", name_prefix(task_id))
}

/// Whether `name` is one of this task's branch names: the task prefix
/// followed by digits only, so `cas-cas-1-2` never matches task `cas-1-2`'s
/// sibling `cas-1`.
pub fn owns_branch_name(task_id: &str, name: &str) -> bool {
    name.strip_prefix(&name_prefix(task_id))
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// The next free index for this task, above every existing name it owns.
pub fn next_index<'a>(task_id: &str, existing: impl IntoIterator<Item = &'a str>) -> u32 {
    existing
        .into_iter()
        .filter(|name| owns_branch_name(task_id, name))
        .filter_map(|name| name[name_prefix(task_id).len()..].parse::<u32>().ok())
        .max()
        .map_or(1, |max| max + 1)
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

pub fn ledger_dir(cas_root: &Path) -> PathBuf {
    cas_root.join("db-branches")
}

fn safe_task_id(task_id: &str) -> Result<&str, String> {
    if !task_id.is_empty()
        && task_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(task_id)
    } else {
        Err(format!("invalid task id {task_id:?}"))
    }
}

pub fn ledger_path(cas_root: &Path, task_id: &str) -> Result<PathBuf, String> {
    Ok(ledger_dir(cas_root).join(format!("{}.json", safe_task_id(task_id)?)))
}

pub fn load(cas_root: &Path, task_id: &str) -> Vec<DbBranchRecord> {
    let Ok(path) = ledger_path(cas_root, task_id) else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(cas_root: &Path, task_id: &str, records: &[DbBranchRecord]) -> Result<(), String> {
    let path = ledger_path(cas_root, task_id)?;
    let dir = ledger_dir(cas_root);
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let body = serde_json::to_vec_pretty(records).map_err(|error| error.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|error| format!("write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|error| format!("write {}: {error}", path.display()))
}

/// Every task that has a ledger, with its records.
pub fn load_all(cas_root: &Path) -> Vec<(String, Vec<DbBranchRecord>)> {
    let Ok(entries) = std::fs::read_dir(ledger_dir(cas_root)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<DbBranchRecord>)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let task_id = name.strip_suffix(".json")?.to_string();
            safe_task_id(&task_id).ok()?;
            let records = load(cas_root, &task_id);
            Some((task_id, records))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// Neon responses (tolerant: the MCP server has shipped several layouts)
// ---------------------------------------------------------------------------

/// The first JSON value in `text` (tool output may carry a sentence before it).
fn json_in(text: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        return Some(value);
    }
    let start = text.find(['{', '['])?;
    let mut stream =
        serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
    stream.next()?.ok()
}

fn collect_branches(value: &serde_json::Value, out: &mut Vec<NeonBranchInfo>) {
    match value {
        serde_json::Value::Object(map) => {
            let id = map.get("id").and_then(|v| v.as_str());
            let name = map.get("name").and_then(|v| v.as_str());
            if let (Some(id), Some(name)) = (id, name)
                && id.starts_with("br-")
            {
                let flag = |key: &str| map.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
                out.push(NeonBranchInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    is_default: flag("default") || flag("is_default") || flag("primary"),
                });
                return;
            }
            for child in map.values() {
                collect_branches(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_branches(item, out);
            }
        }
        serde_json::Value::String(inner) => {
            // An MCP text envelope nests the payload as a JSON string.
            if let Some(nested) = json_in(inner) {
                collect_branches(&nested, out);
            }
        }
        _ => {}
    }
}

/// Every branch object (`id` starting `br-`, with a `name`) in a tool result.
pub fn parse_branches(text: &str) -> Vec<NeonBranchInfo> {
    let mut out = Vec::new();
    if let Some(value) = json_in(text) {
        collect_branches(&value, &mut out);
    }
    out
}

fn connection_start(text: &str) -> Option<usize> {
    ["postgresql://", "postgres://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min()
}

fn connection_end(rest: &str) -> usize {
    rest.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '\\' | '<' | '>'))
        .unwrap_or(rest.len())
}

/// The first `postgres(ql)://` URL in a tool result.
pub fn extract_connection_string(text: &str) -> Option<String> {
    let start = connection_start(text)?;
    let rest = &text[start..];
    Some(rest[..connection_end(rest)].to_string())
}

/// `text` with every connection string replaced, so upstream errors can be
/// shown without leaking a password.
pub fn redact_connection_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = connection_start(rest) {
        out.push_str(&rest[..start]);
        out.push_str("<connection string redacted>");
        let tail = &rest[start..];
        rest = &tail[connection_end(tail)..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Project binding and the parent guardrail
// ---------------------------------------------------------------------------

/// The Neon project and labelled branches recorded by `cas integrate neon`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonProjectBinding {
    pub project_id: String,
    /// `(env label, branch id)`, e.g. `("staging", "br-…")`.
    pub labels: Vec<(String, String)>,
    pub source: PathBuf,
}

/// Read the project binding from the repo's generated Neon skill file.
pub fn project_binding(project_root: &Path) -> Result<NeonProjectBinding, String> {
    for relative in [
        ".claude/skills/neon-database/SKILL.md",
        ".cursor/skills/neon-database/SKILL.md",
    ] {
        let path = project_root.join(relative);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some((project_id, labels)) = crate::cli::integrate::neon::recorded_branches(&text) {
            return Ok(NeonProjectBinding {
                project_id,
                labels,
                source: path,
            });
        }
    }
    Err(format!(
        "no Neon project is recorded for {}: run `cas integrate neon init` so the supervisor knows the project and its non-production branches",
        project_root.display()
    ))
}

/// Whether a branch is production: Neon's default branch, a branch the skill
/// file labels `production`, or one literally named `main` or `production`
/// (the same rule as the cas-d8fc SQL write guard).
pub fn is_production(binding: &NeonProjectBinding, branch: &NeonBranchInfo) -> bool {
    branch.is_default
        || binding
            .labels
            .iter()
            .any(|(label, id)| label == "production" && id == &branch.id)
        || branch.name.eq_ignore_ascii_case("main")
        || branch.name.eq_ignore_ascii_case("production")
}

/// The parent a new branch copies. With no request, the recorded `dev`
/// branch, else `staging`. A request may be a recorded label, a branch id or a
/// branch name. A production parent is always refused.
pub fn resolve_parent(
    binding: &NeonProjectBinding,
    requested: Option<&str>,
    branches: &[NeonBranchInfo],
) -> Result<NeonBranchInfo, String> {
    let label_id = |label: &str| {
        binding
            .labels
            .iter()
            .find(|(recorded, _)| recorded == label)
            .map(|(_, id)| id.clone())
    };
    let wanted = match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => label_id(value).unwrap_or_else(|| value.to_string()),
        None => label_id("dev").or_else(|| label_id("staging")).ok_or_else(|| {
            format!(
                "{} records no dev or staging branch; pass branch=<a non-production branch> to name the parent",
                binding.source.display()
            )
        })?,
    };
    let parent = branches
        .iter()
        .find(|branch| branch.id == wanted || branch.name == wanted)
        .cloned()
        .ok_or_else(|| {
            format!(
                "parent branch {wanted:?} was not found in Neon project {}",
                binding.project_id
            )
        })?;
    if is_production(binding, &parent) {
        return Err(format!(
            "refused: parent {} ({}) is the production branch; a disposable branch copies dev or staging, never production",
            parent.name, parent.id
        ));
    }
    Ok(parent)
}

// ---------------------------------------------------------------------------
// The worktree env file
// ---------------------------------------------------------------------------

fn git_exclude_path(worktree: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "info/exclude"])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    Some(if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    })
}

/// Keep the env file out of git: add it to the repository's `info/exclude`.
pub fn ensure_git_excluded(worktree: &Path) -> Result<(), String> {
    let exclude = git_exclude_path(worktree)
        .ok_or_else(|| format!("{} is not a git worktree", worktree.display()))?;
    let pattern = format!("/{ENV_FILE}");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current
        .lines()
        .any(|line| line.trim() == pattern || line.trim() == ENV_FILE)
    {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let mut body = current;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("# cas-0033: disposable database branch credentials\n");
    body.push_str(&pattern);
    body.push('\n');
    std::fs::write(&exclude, body).map_err(|error| format!("write {}: {error}", exclude.display()))
}

/// Write `.env.cas-db` (owner read/write only) and exclude it from git.
pub fn write_env_file(
    worktree: &Path,
    task_id: &str,
    branch: &NeonBranchInfo,
    connection: &str,
) -> Result<PathBuf, String> {
    ensure_git_excluded(worktree)?;
    let path = worktree.join(ENV_FILE);
    let body = format!(
        "# Disposable Neon branch for {task_id} (cas-0033). It is deleted when the task closes.\n\
         DATABASE_URL={connection}\n\
         CAS_DB_BRANCH={}\n\
         CAS_DB_BRANCH_ID={}\n",
        branch.name, branch.id
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An existing file keeps its old mode through open(); tighten it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    }
    std::io::Write::write_all(&mut file, body.as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(path)
}

/// Remove the env file if it still names this branch.
pub fn remove_env_file(record: &DbBranchRecord) {
    let Ok(body) = std::fs::read_to_string(&record.env_file) else {
        return;
    };
    if body.contains(&format!("CAS_DB_BRANCH_ID={}\n", record.branch_id)) {
        let _ = std::fs::remove_file(&record.env_file);
    }
}

// ---------------------------------------------------------------------------
// Provision and teardown
// ---------------------------------------------------------------------------

/// Create one branch for `task_id`, write its env file, and record it.
pub async fn provision<A: NeonBranchApi>(
    api: &A,
    cas_root: &Path,
    binding: &NeonProjectBinding,
    task_id: &str,
    worktree: &Path,
    requested_parent: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DbBranchRecord, String> {
    safe_task_id(task_id)?;
    if !worktree.is_dir() {
        return Err(format!(
            "the worktree {} does not exist, so there is nowhere to write {ENV_FILE}",
            worktree.display()
        ));
    }
    let mut records = load(cas_root, task_id);
    let live = records.iter().filter(|record| record.is_open()).count();
    if live >= MAX_LIVE_PER_TASK {
        return Err(format!(
            "{task_id} already has {live} branches (the cap is {MAX_LIVE_PER_TASK}); delete one with db_branch_delete first"
        ));
    }
    let branches = api.list_branches(&binding.project_id).await?;
    let parent = resolve_parent(binding, requested_parent, &branches)?;
    let index = next_index(
        task_id,
        branches
            .iter()
            .map(|branch| branch.name.as_str())
            .chain(records.iter().map(|record| record.name.as_str())),
    );
    let name = branch_name(task_id, index);
    let created = api
        .create_branch(&binding.project_id, &name, &parent.id)
        .await?;
    let finish = async {
        let connection = api
            .connection_string(&binding.project_id, &created.id)
            .await?;
        write_env_file(worktree, task_id, &created, &connection)
    };
    let env_file = match finish.await {
        Ok(path) => path,
        Err(error) => {
            // Never leave a branch nobody recorded.
            let cleanup = match api.delete_branch(&binding.project_id, &created.id).await {
                Ok(()) => "the new branch was deleted again".to_string(),
                Err(delete_error) => format!(
                    "deleting the new branch {} also failed ({}); delete it by hand",
                    created.id,
                    redact_connection_strings(&delete_error)
                ),
            };
            return Err(format!("{}; {cleanup}", redact_connection_strings(&error)));
        }
    };
    let record = DbBranchRecord {
        task_id: task_id.to_string(),
        name: created.name.clone(),
        branch_id: created.id.clone(),
        project_id: binding.project_id.clone(),
        parent_id: parent.id.clone(),
        parent_name: parent.name.clone(),
        worktree: worktree.to_path_buf(),
        env_file,
        created_at: now,
        expires_at: now + Duration::hours(TTL_HOURS),
        state: DbBranchState::Live,
        deleted_at: None,
        last_attempt_at: None,
        last_error: None,
    };
    records.push(record.clone());
    save(cas_root, task_id, &records)?;
    Ok(record)
}

/// What a teardown did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TeardownReport {
    pub deleted: Vec<DbBranchRecord>,
    pub failed: Vec<(DbBranchRecord, String)>,
}

/// Delete this task's open branches that `select` picks. Only names this task
/// owns are ever deleted.
pub async fn teardown<A: NeonBranchApi>(
    api: &A,
    cas_root: &Path,
    task_id: &str,
    select: impl Fn(&DbBranchRecord) -> bool,
    now: DateTime<Utc>,
) -> Result<TeardownReport, String> {
    let mut records = load(cas_root, task_id);
    let mut report = TeardownReport::default();
    for record in records.iter_mut() {
        if !record.is_open() || !select(record) || !owns_branch_name(task_id, &record.name) {
            continue;
        }
        record.last_attempt_at = Some(now);
        match api
            .delete_branch(&record.project_id, &record.branch_id)
            .await
        {
            Ok(()) => {
                record.state = DbBranchState::Deleted;
                record.deleted_at = Some(now);
                record.last_error = None;
                remove_env_file(record);
                report.deleted.push(record.clone());
            }
            Err(error) => {
                let error = redact_connection_strings(&error);
                record.state = DbBranchState::PendingDelete;
                record.last_error = Some(error.clone());
                report.failed.push((record.clone(), error));
            }
        }
    }
    if !report.deleted.is_empty() || !report.failed.is_empty() {
        save(cas_root, task_id, &records)?;
    }
    Ok(report)
}

/// Queue this task's open branches for the supervisor to delete, for a caller
/// that cannot reach Neon itself (a worker closing its own task).
pub fn mark_pending_delete(cas_root: &Path, task_id: &str) -> Result<Vec<DbBranchRecord>, String> {
    let mut records = load(cas_root, task_id);
    let mut marked = Vec::new();
    for record in records.iter_mut() {
        if record.state == DbBranchState::Live {
            record.state = DbBranchState::PendingDelete;
            marked.push(record.clone());
        }
    }
    if !marked.is_empty() {
        save(cas_root, task_id, &records)?;
    }
    Ok(marked)
}

/// Whether an automatic sweep should try to delete this record now: it is
/// queued, its task ended, or its worktree is gone; and a recent failure is
/// given time before the next try.
pub fn due_for_sweep(record: &DbBranchRecord, task_ended: bool, now: DateTime<Utc>) -> bool {
    if !record.is_open() {
        return false;
    }
    let wanted =
        record.state == DbBranchState::PendingDelete || task_ended || !record.worktree.exists();
    let resting = record
        .last_attempt_at
        .is_some_and(|at| now - at < Duration::seconds(RETRY_AFTER_SECS));
    wanted && !resting
}

/// `gc_report` lines for branches that outlived their task, worktree or TTL.
pub fn gc_flags(
    all: &[(String, Vec<DbBranchRecord>)],
    task_ended: impl Fn(&str) -> bool,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (task_id, records) in all {
        for record in records.iter().filter(|record| record.is_open()) {
            let mut reasons = Vec::new();
            if record.state == DbBranchState::PendingDelete {
                reasons.push(match &record.last_error {
                    Some(error) => format!("deletion pending (last error: {error})"),
                    None => "deletion pending".to_string(),
                });
            }
            if task_ended(task_id) && record.state == DbBranchState::Live {
                reasons.push(format!("{task_id} has ended"));
            }
            if !record.worktree.exists() {
                reasons.push(format!("worktree {} is gone", record.worktree.display()));
            }
            if now > record.expires_at {
                reasons.push(format!("past its {TTL_HOURS}h TTL"));
            }
            if !reasons.is_empty() {
                out.push(format!(
                    "{} ({}) for {task_id}: {}",
                    record.name,
                    record.branch_id,
                    reasons.join("; ")
                ));
            }
        }
    }
    out
}

/// One `db_branch_show` line. Never includes the connection string.
pub fn render_record(record: &DbBranchRecord) -> String {
    let state = match record.state {
        DbBranchState::Live => "live".to_string(),
        DbBranchState::PendingDelete => match &record.last_error {
            Some(error) => format!("deletion pending (last error: {error})"),
            None => "deletion pending".to_string(),
        },
        DbBranchState::Deleted => format!(
            "deleted {}",
            record
                .deleted_at
                .map(|at| at.to_rfc3339())
                .unwrap_or_default()
        ),
    };
    format!(
        "  {} ({}) from {} ({}) in project {}: {state}; env file {}; created {}, expires {}",
        record.name,
        record.branch_id,
        record.parent_name,
        record.parent_id,
        record.project_id,
        record.env_file.display(),
        record.created_at.to_rfc3339(),
        record.expires_at.to_rfc3339(),
    )
}

/// Only the supervisor holds the Neon credential. A worker asks for a branch
/// with a blocker message instead.
pub fn role_gate(is_supervisor: bool, action: &str) -> Result<(), String> {
    if is_supervisor {
        Ok(())
    } else {
        Err(format!(
            "coordination {action} rejected: only the supervisor provisions or deletes database branches, and connection strings never reach a worker through MCP. Ask for one with `coordination action=message target=supervisor blocker=true message=\"db branch for <task-id>: <why>\"`; the supervisor's db_branch_create writes {ENV_FILE} into your worktree."
        ))
    }
}

#[cfg(test)]
#[path = "db_branch_tests.rs"]
mod tests;
