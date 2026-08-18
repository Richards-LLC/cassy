//! `server_start` / `server_stop` / `server_list` (cas-7c93, GH #87).
//!
//! The sanctioned lifecycle for servers an agent needs to keep running. The
//! process work lives in [`crate::ui::factory::server_registry`]; this file is
//! the MCP surface over it, and the operator-facing rendering that answers
//! "what is listening and who started it" without `ps`/`lsof` archaeology.

use crate::mcp::tools::service::imports::*;
use crate::ui::factory::server_registry::{
    self, RegisteredServer, ServerLiveness, ServerSpec, ServerState, StopOutcome,
};

/// How a registry entry reads in `server_list`.
///
/// Kept pure and separate from the handler so the rendering rules — including
/// "a shared entry must say it survives teardown" — are unit-testable without
/// spawning processes.
pub(super) fn render_server_line(
    record: &RegisteredServer,
    liveness: ServerLiveness,
    observed_ports: &[u16],
) -> String {
    let state = match (record.state, liveness) {
        // The record's own claim is only trusted once reality agrees.
        (ServerState::Running, ServerLiveness::Live) => "running".to_string(),
        (ServerState::Running, ServerLiveness::Replaced) => "dead (pid reused)".to_string(),
        (ServerState::Running, ServerLiveness::Unverifiable) => "unverified".to_string(),
        (ServerState::Running, ServerLiveness::Gone) => "dead".to_string(),
        (state, _) => state.label().to_string(),
    };

    let ports = if !observed_ports.is_empty() {
        format!(
            " listening on {}",
            observed_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    } else if let Some(port) = record.expected_port {
        format!(" expected port {port} (not bound)")
    } else {
        String::new()
    };

    let owner = match (record.owner_task.as_deref(), record.owner_worker.as_deref()) {
        (Some(task), Some(worker)) => format!(" — started by {worker} for {task}"),
        (Some(task), None) => format!(" — for {task}"),
        (None, Some(worker)) => format!(" — started by {worker}"),
        (None, None) => String::new(),
    };

    let survival = if record.shared {
        " [shared: survives worker teardown]"
    } else {
        " [private: dies with its worker]"
    };

    format!(
        "  {} ({}) pid {}{} — {}{}{}\n     cmd: {}\n     cwd: {}",
        record.name,
        record.id,
        record.pid,
        ports,
        state,
        owner,
        survival,
        record.command,
        record.cwd.display(),
    )
}

impl CasService {
    /// Launch a long-running server under Cassy supervision.
    pub(super) async fn factory_server_start(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        let command = req
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "server_start requires `command` (for example command=\"npm run dev\")",
                )
            })?;

        let cwd = match req.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::env::current_dir().map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("server_start could not resolve the current directory: {e}"),
                )
            })?,
        };

        let port = match req.port {
            Some(port) if !(1..=65535).contains(&port) => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("server_start port {port} is outside 1-65535"),
                ));
            }
            Some(port) => Some(port as u16),
            None => None,
        };

        let name = req
            .id
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_server_name(command));

        if let Some(existing) = server_registry::find(&self.inner.cas_root, &name)
            .ok()
            .flatten()
            .filter(|record| {
                record.state == ServerState::Running
                    && matches!(server_registry::liveness(record), ServerLiveness::Live)
            })
        {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "a server named '{}' is already running (id {}, pid {}). Stop it first \
                     (`server_stop id={}`) or start this one under a different `id`.",
                    existing.name, existing.id, existing.pid, existing.id
                ),
            ));
        }

        let shared = req.shared.unwrap_or(false);
        let spec = ServerSpec {
            name,
            command: command.to_string(),
            cwd,
            expected_port: port,
            owner_task: req.task_id.clone(),
            owner_worker: std::env::var("CAS_AGENT_NAME").ok(),
            factory_session: std::env::var("CAS_FACTORY_SESSION").ok(),
            shared,
        };

        let record = server_registry::start(&self.inner.cas_root, &spec).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("server_start failed: {e}"),
            )
        })?;

        let survival = if shared {
            "shared: it is outside worker containment and survives worker teardown — \
             stop it with `server_stop` when the work is done"
        } else {
            "private: it stays in this worker's containment scope and dies at teardown. \
             Pass shared=true for a service that must outlive the task"
        };

        Ok(Self::success(format!(
            "Started server '{}' (id {})\n  pid: {}\n  cwd: {}\n  cmd: {}\n  {}\n  logs: {}\n\n\
             Query it with `coordination action=server_list`; stop it with \
             `coordination action=server_stop id={}`.",
            record.name,
            record.id,
            record.pid,
            record.cwd.display(),
            record.command,
            survival,
            record
                .log_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none)".to_string()),
            record.id,
        )))
    }

    /// Stop a registered server.
    pub(super) async fn factory_server_stop(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        let handle = req
            .id
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "server_stop requires `id` (the server id or name from server_list)",
                )
            })?;

        let record = server_registry::find(&self.inner.cas_root, handle)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to read the server registry: {e}"),
                )
            })?
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "no registered server matches '{handle}' — \
                         run `coordination action=server_list` to see the registry"
                    ),
                )
            })?;

        let outcome = server_registry::stop(&self.inner.cas_root, &record).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("server_stop failed: {e}"),
            )
        })?;

        let message = match outcome {
            StopOutcome::Stopped { pid, ref ports } => {
                let freed = if ports.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n  freed port(s): {}",
                        ports
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                };
                format!(
                    "Stopped server '{}' (id {})\n  pid: {pid}{freed}",
                    record.name, record.id
                )
            }
            StopOutcome::AlreadyGone => format!(
                "Server '{}' (id {}) was already gone; the registry entry is now marked dead.",
                record.name, record.id
            ),
            StopOutcome::RefusedUnverified(liveness) => format!(
                "Refused to signal server '{}' (id {}): pid {} {}.\n\n\
                 Nothing was killed. The entry is marked dead — Cassy never signals a pid it \
                 cannot prove is still the process it started, because the pid may now belong \
                 to something else entirely.",
                record.name,
                record.id,
                record.pid,
                match liveness {
                    ServerLiveness::Replaced => "now belongs to a different process",
                    _ => "could not be verified",
                }
            ),
        };

        Ok(Self::success(message))
    }

    /// "What is listening, and who started it?"
    pub(super) async fn factory_server_list(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        // Reconcile before reporting: a listing that shows a long-dead pid as
        // running is worse than no listing at all.
        let records = server_registry::refresh(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to read the server registry: {e}"),
            )
        })?;

        // Keep the registry bounded: terminal entries age out, live ones never
        // do. Best-effort — a failed prune must not deny the listing.
        let _ = server_registry::prune_history(&self.inner.cas_root, &records);

        let task_filter = req
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let records: Vec<_> = records
            .into_iter()
            .filter(|record| match task_filter {
                Some(task) => record.owner_task.as_deref() == Some(task),
                None => true,
            })
            .collect();

        if records.is_empty() {
            return Ok(Self::success(format!(
                "No registered servers{}.\n\n\
                 Long-running servers belong in the registry: \
                 `coordination action=server_start command=\"npm run dev\" port=5173` \
                 (add shared=true when it must outlive the task). A raw `npm run dev &` is \
                 killed at worker teardown and is invisible here.",
                task_filter.map(|t| format!(" for {t}")).unwrap_or_default()
            )));
        }

        let (mut live, mut history) = (Vec::new(), Vec::new());
        for record in &records {
            let liveness = server_registry::liveness(record);
            let ports = if matches!(liveness, ServerLiveness::Live) {
                server_registry::listening_ports(record)
            } else {
                Vec::new()
            };
            let line = render_server_line(record, liveness, &ports);
            if record.state == ServerState::Running && matches!(liveness, ServerLiveness::Live) {
                live.push(line);
            } else {
                history.push(line);
            }
        }

        let mut out = String::new();
        if live.is_empty() {
            out.push_str("No servers currently running.\n");
        } else {
            out.push_str(&format!("Running servers ({}):\n", live.len()));
            out.push_str(&live.join("\n"));
            out.push('\n');
        }
        if !history.is_empty() {
            out.push_str(&format!("\nRecent history ({}):\n", history.len()));
            out.push_str(&history.join("\n"));
            out.push('\n');
        }
        Ok(Self::success(out))
    }
}

/// Name a server after its command when the caller did not name it, so
/// `server_list` reads as something other than a wall of ids.
pub(super) fn default_server_name(command: &str) -> String {
    let stem: String = command
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join("-");
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        "server".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
#[path = "server_ops_tests.rs"]
mod tests;
