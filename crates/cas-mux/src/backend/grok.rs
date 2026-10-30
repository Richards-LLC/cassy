use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::harness::{CapabilityState, HarnessCapabilities};
use crate::pty::PtyConfig;

pub(crate) static GROK: Grok = Grok;

pub(crate) struct Grok;

impl Backend for Grok {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_hooks: CapabilityState::Supported,
            supports_subagents: CapabilityState::Supported,
            supports_textbox_submit: true,
            requires_bracketed_paste_injection: false,
            tool_prefix: "cas__",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::grok(
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
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            None,
            launch.cas_root,
        );
        // Grok skips project skills in an untrusted folder and resolves a
        // non-interactive launch to untrusted. Per launch, not the shared trust
        // store: a Cassy-created worker checkout is what Claude and Codex
        // already record as trusted.
        config.env.push(("GROK_FOLDER_TRUST".to_string(), "false".to_string()));
        config
    }

    fn prepare_worker_catalog(&self, cwd: &std::path::Path) {
        let home = std::env::var_os("GROK_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".grok")));
        let Some(home) = home else { return };
        if let Err(error) = super::skill_catalog::prepare_grok_project_skills(cwd, &home) {
            tracing::warn!(%error, cwd = %cwd.display(), "Grok worker skill projection failed; full listing kept");
        }
    }

    fn build_supervisor_config(&self, launch: SupervisorLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::grok(
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

    fn turn_cancel_bytes(&self) -> &'static [u8] {
        &[0x03]
    }

    fn has_turn_event_stream(&self) -> bool {
        true
    }
}
