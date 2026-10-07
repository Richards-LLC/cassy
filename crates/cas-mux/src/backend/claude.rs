use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::Result;
use crate::error::Error;
use crate::harness::HarnessCapabilities;
use crate::pty::PtyConfig;
use std::path::Path;

pub(crate) static CLAUDE: Claude = Claude;

pub(crate) struct Claude;

impl Backend for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_hooks: true,
            supports_subagents: true,
            supports_textbox_submit: true,
            requires_bracketed_paste_injection: false,
            tool_prefix: "mcp__cas__",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::claude(
            launch.name,
            "worker",
            launch.cwd,
            launch.cas_root,
            Some(launch.supervisor_name),
            None,
            launch.model,
            launch.effort,
            launch.teams,
        );
        config.apply_claude_account(
            launch.config_dir,
            launch.secure_storage_dir,
            launch.config_dir_source,
        );
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            launch.config_dir,
            launch.cas_root,
        );
        config
    }

    fn build_supervisor_config(&self, launch: SupervisorLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::claude(
            launch.name,
            "supervisor",
            launch.cwd,
            launch.cas_root,
            None,
            Some(launch.worker_cli.backend().name()),
            launch.model,
            launch.effort,
            launch.teams,
        );
        finish_supervisor_config(&mut config, self.name(), launch.worker_names);
        config
    }

    /// cas-0f5b: Claude Code runs no hooks in an untrusted workspace, and the
    /// factory's IS_DEMO launch skips the trust dialog without trusting it, so
    /// every Claude agent ran with no CAS guard. Record the trust for this cwd
    /// in the agent's own Claude config before it starts, or refuse to start.
    /// The launch canary (SessionStart marker) still proves hooks run.
    fn prepare_workdir(&self, cwd: &Path, config_dir: Option<&str>) -> Result<()> {
        let config = cas_pty::claude_global_config_path(config_dir).ok_or_else(|| {
            Error::pty(
                "refusing to launch Claude: no CLAUDE_CONFIG_DIR or HOME to record workspace trust in",
            )
        })?;
        match cas_pty::ensure_claude_project_trusted_in(&config, cwd) {
            Ok(cas_pty::ClaudeTrustOutcome::Added(_) | cas_pty::ClaudeTrustOutcome::AlreadyPresent) => Ok(()),
            Ok(cas_pty::ClaudeTrustOutcome::Skipped(reason)) => Err(Error::pty(format!(
                "refusing to launch Claude before its workspace trust is recorded: {reason}"
            ))),
            Err(error) => Err(Error::pty(format!(
                "refusing to launch Claude: could not record workspace trust for {} in {}: {error}",
                cwd.display(),
                config.display()
            ))),
        }
    }

    fn turn_cancel_bytes(&self) -> &'static [u8] {
        &[0x1b]
    }
}
