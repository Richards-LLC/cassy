use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::harness::HarnessCapabilities;
use crate::pty::PtyConfig;

pub(crate) static OPENCODE: OpenCode = OpenCode;

pub(crate) struct OpenCode;

impl Backend for OpenCode {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            // Documentation establishes candidate plugin and subagent surfaces,
            // but support is gated on the retained live-conformance matrix.
            supports_hooks: false,
            supports_subagents: false,
            supports_textbox_submit: false,
            // Raw versus bracketed-paste framing has not been measured. This
            // false value adds no framing sequence; it is not a support claim.
            requires_bracketed_paste_injection: false,
            tool_prefix: "cas_",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        // Preserve the caller's requested spelling. Model-aware validation is
        // owned by the spawn/policy layer; this adapter must not silently map
        // one effort to another.
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let mut config = PtyConfig::opencode(
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
        let mut config = PtyConfig::opencode(
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
        // Deliberately unavailable until task 7 measures OpenCode's TUI. An
        // empty write is safer than copying Esc or Ctrl-C from another CLI.
        &[]
    }
}
