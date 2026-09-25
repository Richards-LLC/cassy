//! `db_branch_create` / `db_branch_show` / `db_branch_delete` (cas-0033,
//! GH #1005 item 3), plus teardown when a task ends.
//!
//! Supervisor-only. The supervisor's `cas serve` holds the Neon credential
//! through the MCP proxy (`neon.*` is allowlisted for supervisors only), so a
//! worker never holds `NEON_API_KEY` and never sees a connection string: it
//! reads `.env.cas-db` in its own worktree. The provider-independent logic is
//! in [`crate::db_branch`].

use crate::db_branch::{self, DbBranchRecord};
#[cfg(feature = "mcp-proxy")]
use crate::db_branch::{NeonBranchApi, NeonBranchInfo};
use crate::mcp::tools::service::imports::*;

/// The proxy server name Neon is registered under in `.cas/proxy.toml`.
#[cfg(feature = "mcp-proxy")]
const NEON_SERVER: &str = "neon";

/// [`NeonBranchApi`] over the supervisor's MCP proxy.
#[cfg(feature = "mcp-proxy")]
struct ProxyNeonApi<'a> {
    proxy: &'a cmcp_core::ProxyEngine,
    caller: cmcp_core::ProxyCaller,
}

#[cfg(feature = "mcp-proxy")]
impl ProxyNeonApi<'_> {
    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<String, String> {
        let code =
            serde_json::json!({ "server": NEON_SERVER, "tool": tool, "args": args }).to_string();
        let result = self
            .proxy
            .execute(&self.caller, &code, Some(400_000))
            .await
            .map_err(|error| {
                db_branch::redact_connection_strings(&cmcp_core::describe_upstream_call_error(
                    &error,
                ))
            })?;
        if result.is_error {
            let text: String = db_branch::redact_connection_strings(&result.text)
                .chars()
                .take(400)
                .collect();
            return Err(format!("{NEON_SERVER}.{tool} failed: {text}"));
        }
        Ok(result.text)
    }
}

#[cfg(feature = "mcp-proxy")]
impl NeonBranchApi for ProxyNeonApi<'_> {
    async fn list_branches(&self, project_id: &str) -> Result<Vec<NeonBranchInfo>, String> {
        let text = self
            .call(
                "list_branches",
                serde_json::json!({ "project_id": project_id }),
            )
            .await?;
        Ok(db_branch::parse_branches(&text))
    }

    async fn create_branch(
        &self,
        project_id: &str,
        name: &str,
        parent_id: &str,
    ) -> Result<NeonBranchInfo, String> {
        let text = self
            .call(
                "create_branch",
                serde_json::json!({
                    "project_id": project_id,
                    "name": name,
                    "parent_id": parent_id,
                    // Cost cap: a small autoscaling ceiling that suspends when idle.
                    "compute": {
                        "min_cu": 0.25,
                        "max_cu": db_branch::MAX_CU,
                        "suspend_timeout_seconds": 300
                    }
                }),
            )
            .await?;
        let created = db_branch::parse_branches(&text);
        created
            .iter()
            .find(|branch| branch.name == name)
            .or_else(|| created.first())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{NEON_SERVER}.create_branch returned no branch id for {name}; check the Neon console for a stray branch"
                )
            })
    }

    async fn connection_string(&self, project_id: &str, branch_id: &str) -> Result<String, String> {
        let text = self
            .call(
                "get_connection_string",
                serde_json::json!({ "project_id": project_id, "branch_id": branch_id }),
            )
            .await?;
        db_branch::extract_connection_string(&text).ok_or_else(|| {
            format!(
                "{NEON_SERVER}.get_connection_string returned no postgres:// URL for {branch_id}"
            )
        })
    }

    async fn delete_branch(&self, project_id: &str, branch_id: &str) -> Result<(), String> {
        self.call(
            "delete_branch",
            serde_json::json!({ "project_id": project_id, "branch_id": branch_id }),
        )
        .await
        .map(|_| ())
    }
}

/// Whether a task has ended (closed, cancelled, or no longer exists).
fn task_ended(tasks: Option<&std::sync::Arc<dyn cas_store::TaskStore>>, task_id: &str) -> bool {
    match tasks.map(|store| store.get(task_id)) {
        Some(Ok(task)) => matches!(
            task.status,
            cas_types::TaskStatus::Closed | cas_types::TaskStatus::Cancelled
        ),
        Some(Err(_)) => true,
        None => false,
    }
}

fn note_line(text: &str) -> String {
    format!(
        "[{}] 🗄️ DB BRANCH {text}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M")
    )
}

impl CasService {
    fn db_branch_note(&self, task_id: &str, text: &str) {
        if let Ok(store) = self.inner.open_task_store()
            && let Err(error) = store.append_note(task_id, &note_line(text))
        {
            tracing::warn!(task_id = %task_id, error = %error, "cas-0033: db branch note not recorded");
        }
    }

    /// The worktree `.env.cas-db` goes to: an explicit worker name
    /// (`target`), else the task's assignee.
    fn db_branch_worktree(
        &self,
        task: &cas_types::Task,
        target: Option<&str>,
    ) -> Result<std::path::PathBuf, McpError> {
        let named = target
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| task.assignee.clone())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "{} has no assignee; assign a worker first or pass target=<worker name>",
                        task.id
                    ),
                )
            })?;
        let agent = self.inner.open_agent_store().ok().and_then(|store| {
            store.get(&named).ok().or_else(|| {
                store
                    .list(None)
                    .ok()?
                    .into_iter()
                    .find(|agent| agent.name == named)
            })
        });
        let name = agent
            .as_ref()
            .map_or(named.clone(), |agent| agent.name.clone());
        let candidates = [
            agent
                .as_ref()
                .and_then(|agent| agent.metadata.get("clone_path"))
                .map(std::path::PathBuf::from),
            Some(self.inner.cas_root.join("worktrees").join(&name)),
        ];
        candidates
            .into_iter()
            .flatten()
            .find(|path| path.is_dir())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "no worktree on disk for worker {name} (looked for clone_path metadata and {}); spawn it with isolate=true or pass target=<worker name>",
                        self.inner.cas_root.join("worktrees").join(&name).display()
                    ),
                )
            })
    }

    #[cfg(feature = "mcp-proxy")]
    fn db_branch_api(&self) -> Result<ProxyNeonApi<'_>, McpError> {
        let proxy = self.proxy.as_deref().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "db_branch needs the Neon MCP server in .cas/proxy.toml (allowlisted as supervisor:neon.*)",
            )
        })?;
        Ok(ProxyNeonApi {
            proxy,
            caller: self.proxy_caller()?,
        })
    }

    /// Provision a disposable branch for a task.
    pub(super) async fn db_branch_create(
        &self,
        req: &CoordinationRequest,
    ) -> Result<CallToolResult, McpError> {
        db_branch::role_gate(
            crate::harness_policy::is_supervisor_from_env(),
            "db_branch_create",
        )
        .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let task_id = req
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "db_branch_create requires task_id",
                )
            })?;
        let task = self
            .inner
            .open_task_store()?
            .get(task_id)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("task {task_id}: {error}"),
                )
            })?;
        if matches!(
            task.status,
            cas_types::TaskStatus::Closed | cas_types::TaskStatus::Cancelled
        ) {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("{task_id} has ended; a disposable branch is only for work in progress"),
            ));
        }
        let worktree = self.db_branch_worktree(&task, req.target.as_deref())?;
        let project_root = self
            .inner
            .cas_root
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| self.inner.cas_root.clone());
        let binding = db_branch::project_binding(&project_root)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        #[cfg(feature = "mcp-proxy")]
        {
            let api = self.db_branch_api()?;
            let record = db_branch::provision(
                &api,
                &self.inner.cas_root,
                &binding,
                task_id,
                &worktree,
                req.branch.as_deref(),
                chrono::Utc::now(),
            )
            .await
            .map_err(|message| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("db_branch_create failed: {message}"),
                )
            })?;
            self.db_branch_note(
                task_id,
                &format!(
                    "created {} ({}) from {} in project {}; {} holds DATABASE_URL (mode 600, git-excluded). It is deleted when {task_id} closes.",
                    record.name,
                    record.branch_id,
                    record.parent_name,
                    record.project_id,
                    record.env_file.display()
                ),
            );
            Ok(Self::success(format!(
                "Created Neon branch {} ({}) for {task_id} from {} ({}).\nThe worker's DATABASE_URL is in {} (mode 600, git-excluded); the connection string is not shown here.\nIt expires in {}h and is deleted when {task_id} closes or is cancelled.",
                record.name,
                record.branch_id,
                record.parent_name,
                record.parent_id,
                record.env_file.display(),
                db_branch::TTL_HOURS,
            )))
        }
        #[cfg(not(feature = "mcp-proxy"))]
        {
            let _ = (worktree, binding);
            Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "db_branch_create needs a build with the mcp-proxy feature",
            ))
        }
    }

    /// What branches exist, per task or for all tasks. Never shows a
    /// connection string.
    pub(super) async fn db_branch_show(
        &self,
        req: &CoordinationRequest,
    ) -> Result<CallToolResult, McpError> {
        db_branch::role_gate(
            crate::harness_policy::is_supervisor_from_env(),
            "db_branch_show",
        )
        .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let all = match req
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(task_id) => vec![(
                task_id.to_string(),
                db_branch::load(&self.inner.cas_root, task_id),
            )],
            None => db_branch::load_all(&self.inner.cas_root),
        };
        let mut out = String::new();
        for (task_id, records) in &all {
            let shown: Vec<&DbBranchRecord> = records
                .iter()
                .filter(|record| req.task_id.is_some() || record.is_open())
                .collect();
            if shown.is_empty() {
                continue;
            }
            out.push_str(&format!("{task_id}:\n"));
            for record in shown {
                out.push_str(&db_branch::render_record(record));
                out.push('\n');
            }
        }
        if out.is_empty() {
            out.push_str("No disposable database branches recorded.\n");
        }
        Ok(Self::success(out))
    }

    /// Delete a task's branches now (all open ones, or `id=<branch id>`).
    pub(super) async fn db_branch_delete(
        &self,
        req: &CoordinationRequest,
    ) -> Result<CallToolResult, McpError> {
        db_branch::role_gate(
            crate::harness_policy::is_supervisor_from_env(),
            "db_branch_delete",
        )
        .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let task_id = req
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "db_branch_delete requires task_id",
                )
            })?;
        let only = req
            .id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        let report = self
            .db_branch_teardown(task_id, |record| {
                only.as_deref()
                    .is_none_or(|id| record.branch_id == id || record.name == id)
            })
            .await?;
        let text = self.db_branch_report_text(task_id, &report, "deleted on request");
        Ok(Self::success(text))
    }

    async fn db_branch_teardown(
        &self,
        task_id: &str,
        select: impl Fn(&DbBranchRecord) -> bool,
    ) -> Result<db_branch::TeardownReport, McpError> {
        #[cfg(feature = "mcp-proxy")]
        {
            let api = self.db_branch_api()?;
            db_branch::teardown(
                &api,
                &self.inner.cas_root,
                task_id,
                select,
                chrono::Utc::now(),
            )
            .await
            .map_err(|message| Self::error(ErrorCode::INTERNAL_ERROR, message))
        }
        #[cfg(not(feature = "mcp-proxy"))]
        {
            let _ = (task_id, select);
            Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "db_branch needs a build with the mcp-proxy feature",
            ))
        }
    }

    fn db_branch_report_text(
        &self,
        task_id: &str,
        report: &db_branch::TeardownReport,
        why: &str,
    ) -> String {
        let mut lines = Vec::new();
        for record in &report.deleted {
            let line = format!("deleted {} ({}): {why}", record.name, record.branch_id);
            self.db_branch_note(task_id, &line);
            lines.push(line);
        }
        for (record, error) in &report.failed {
            let line = format!(
                "could not delete {} ({}): {error}; it stays queued and is retried on the supervisor's next call",
                record.name, record.branch_id
            );
            self.db_branch_note(task_id, &line);
            lines.push(line);
        }
        if lines.is_empty() {
            format!("{task_id} has no open disposable database branch to delete.")
        } else {
            lines.join("\n")
        }
    }

    /// Called after a task close or cancel succeeds. Deletes the task's
    /// branches when this caller can reach Neon (the supervisor); otherwise
    /// queues them for the supervisor. Never fails the close.
    pub(super) async fn db_branch_after_task_end(&self, task_id: &str) {
        if db_branch::load(&self.inner.cas_root, task_id)
            .iter()
            .all(|record| !record.is_open())
        {
            return;
        }
        let tasks = self.inner.open_task_store().ok();
        if !task_ended(tasks.as_ref(), task_id) {
            // Parked for merge, awaiting verification, and so on: not ended yet.
            return;
        }
        if crate::harness_policy::is_supervisor_from_env()
            && let Ok(report) = self.db_branch_teardown(task_id, |_| true).await
        {
            let _ = self.db_branch_report_text(task_id, &report, "its task ended");
            return;
        }
        match db_branch::mark_pending_delete(&self.inner.cas_root, task_id) {
            Ok(marked) if !marked.is_empty() => {
                let names: Vec<String> = marked
                    .iter()
                    .map(|record| format!("{} ({})", record.name, record.branch_id))
                    .collect();
                self.db_branch_note(
                    task_id,
                    &format!(
                        "queued for deletion: {}. The supervisor's next coordination call deletes it.",
                        names.join(", ")
                    ),
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(task_id = %task_id, error = %error, "cas-0033: could not queue db branch deletion");
            }
        }
    }

    /// Supervisor-side sweep: delete queued branches, branches whose task has
    /// ended, and branches whose worktree is gone (after `worktree_cleanup`).
    /// Cheap when nothing is due; a failure waits `RETRY_AFTER_SECS`.
    pub(super) async fn db_branch_sweep(&self) {
        if !crate::harness_policy::is_supervisor_from_env() {
            return;
        }
        let all = db_branch::load_all(&self.inner.cas_root);
        if all
            .iter()
            .all(|(_, records)| records.iter().all(|record| !record.is_open()))
        {
            return;
        }
        let tasks = self.inner.open_task_store().ok();
        let now = chrono::Utc::now();
        for (task_id, records) in &all {
            let ended = task_ended(tasks.as_ref(), task_id);
            if !records
                .iter()
                .any(|record| db_branch::due_for_sweep(record, ended, now))
            {
                continue;
            }
            let why = if ended {
                "its task ended"
            } else {
                "it was queued or its worktree is gone"
            };
            match self
                .db_branch_teardown(task_id, |record| {
                    db_branch::due_for_sweep(record, ended, now)
                })
                .await
            {
                Ok(report) => {
                    let _ = self.db_branch_report_text(task_id, &report, why);
                }
                Err(error) => {
                    tracing::warn!(task_id = %task_id, error = %error.message, "cas-0033: db branch sweep skipped");
                    return;
                }
            }
        }
    }

    /// `gc_report` section: open branches past their task, worktree or TTL.
    pub(super) fn db_branch_gc_section(&self) -> String {
        let all = db_branch::load_all(&self.inner.cas_root);
        let tasks = self.inner.open_task_store().ok();
        let flags = db_branch::gc_flags(
            &all,
            |task_id| task_ended(tasks.as_ref(), task_id),
            chrono::Utc::now(),
        );
        if flags.is_empty() {
            return String::new();
        }
        let mut out = format!(
            "\nDisposable database branches needing attention ({}):\n",
            flags.len()
        );
        for flag in flags {
            out.push_str(&format!("  {flag}\n"));
        }
        out.push_str("Delete with `factory action=db_branch_delete task_id=<id>`; the supervisor's calls also retry queued deletions.\n");
        out
    }
}
