use std::path::Path;

use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config, push_plain_factory_session, sanitize_toml_arg,
};
use crate::error::Error;
use crate::harness::HarnessCapabilities;
use crate::pty::PtyConfig;
use crate::{Effort, Result};

pub(crate) static CODEX: Codex = Codex;

pub(crate) struct Codex;

impl Backend for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_hooks: false,
            supports_subagents: false,
            supports_textbox_submit: false,
            requires_bracketed_paste_injection: true,
            tool_prefix: "mcp__cs__",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::codex(
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
        // cas-9cc3: pin this worker to the requested ChatGPT account. Omitted
        // config_dir keeps plain inheritance, exactly as before.
        config.apply_codex_home(launch.config_dir, launch.config_dir_source);
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            launch.config_dir,
        );
        config.args.push("-c".to_string());
        config.args.push(format!(
            "mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"{}\"",
            launch.supervisor_cli.backend().name()
        ));
        config
    }

    fn build_supervisor_config(&self, launch: SupervisorLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::codex(
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

    fn prepare_workdir(&self, cwd: &Path) -> Result<()> {
        match cas_pty::ensure_project_trusted(cwd)? {
            cas_pty::CodexTrustOutcome::Added(_) | cas_pty::CodexTrustOutcome::AlreadyPresent => {
                Ok(())
            }
            cas_pty::CodexTrustOutcome::Skipped(reason) => Err(Error::pty(format!(
                "refusing to launch Codex before its project trust is verified: {reason}"
            ))),
        }
    }

    fn push_factory_session(&self, config: &mut PtyConfig, session: &str) {
        push_plain_factory_session(config, session);
        let session = sanitize_toml_arg(session);
        config.args.push("-c".to_string());
        config.args.push(format!(
            "mcp_servers.cs.env.CAS_FACTORY_SESSION=\"{session}\""
        ));
    }

    fn turn_cancel_bytes(&self) -> &'static [u8] {
        &[0x1b]
    }
}
