use crate::mcp::tools::core::imports::*;

impl CasCore {
    // ========================================================================
    // Maintenance Tools (for embedded daemon)
    // ========================================================================

    /// Get maintenance daemon status
    pub async fn cas_maintenance_status(&self) -> Result<CallToolResult, McpError> {
        // Record activity
        self.touch();

        if let Some(status) = self.daemon_status().await {
            let mut output = "Maintenance Daemon Status\n=========================\n\n".to_string();

            output.push_str(&format!(
                "Running: {}\n",
                if status.running { "Yes" } else { "No" }
            ));
            output.push_str(&format!(
                "Idle: {} ({} seconds since last request)\n",
                if status.is_idle { "Yes" } else { "No" },
                status.idle_seconds
            ));

            if let Some(last) = status.last_maintenance {
                output.push_str(&format!(
                    "Last maintenance: {}\n",
                    last.format("%Y-%m-%d %H:%M:%S")
                ));
            }

            if let Some(next) = status.next_maintenance {
                output.push_str(&format!(
                    "Next scheduled: {}\n",
                    next.format("%Y-%m-%d %H:%M:%S")
                ));
            }

            output.push_str("\nSession totals:\n");
            output.push_str(&format!(
                "- Observations processed: {}\n",
                status.observations_processed
            ));
            output.push_str(&format!("- Decay applied: {}\n", status.decay_applied));

            if let Some(err) = status.last_error {
                output.push_str(&format!("\nLast error: {err}\n"));
            }

            Ok(Self::success(output))
        } else {
            Ok(Self::success(
                "Maintenance daemon is not running.\n\nThe daemon runs automatically when the MCP server is started with daemon support enabled.",
            ))
        }
    }

    /// Trigger immediate maintenance
    pub async fn cas_maintenance_run(
        &self,
        Parameters(_req): Parameters<MaintenanceRunRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Record activity
        self.touch();

        match self.trigger_maintenance().await {
            Ok(result) => Ok(Self::success(result)),
            Err(e) => {
                // Fallback: run maintenance directly if daemon not available
                let cas_config = crate::config::Config::load(&self.cas_root).unwrap_or_default();
                let archive_max_bytes = cas_config.daemon().archive_max_bytes;
                let memory_decay = cas_config.memory().decay;
                let daemon_config = crate::daemon::DaemonConfig {
                    cas_root: self.cas_root.clone(),
                    archive_max_bytes,
                    curated_importance_floor: memory_decay.curated_importance_floor,
                    promote_on_access: memory_decay.promote_on_access,
                    ..Default::default()
                };

                match crate::daemon::run_maintenance(&daemon_config) {
                    Ok(result) => Ok(Self::success(format!(
                        "Maintenance completed in {:.2}s:\n- Observations: {}\n- Decay applied: {}\n- Trace archives evicted: {}\n- Errors: {}",
                        result.duration_secs,
                        result.observations_processed,
                        result.decay_applied,
                        result.trace_archives_evicted,
                        result.errors.len()
                    ))),
                    Err(run_err) => Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Daemon error: {}. Direct run error: {}", e.message, run_err),
                    )),
                }
            }
        }
    }
}
