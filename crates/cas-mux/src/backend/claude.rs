use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::harness::HarnessCapabilities;
use crate::pty::PtyConfig;

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
        config.apply_claude_config_dir(launch.config_dir, launch.config_dir_source);
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            launch.config_dir,
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

    fn turn_cancel_bytes(&self) -> &'static [u8] {
        &[0x1b]
    }
}
