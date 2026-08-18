use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::harness::HarnessCapabilities;
use crate::pty::PtyConfig;

pub(crate) static GROK: Grok = Grok;

pub(crate) struct Grok;

impl Backend for Grok {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_hooks: true,
            supports_subagents: true,
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
        );
        config
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
