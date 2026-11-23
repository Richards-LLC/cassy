use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::Result;
use crate::error::Error;
use crate::harness::{CapabilityState, HarnessCapabilities};
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
            supports_hooks: CapabilityState::Supported,
            supports_subagents: CapabilityState::Supported,
            supports_textbox_submit: true,
            requires_bracketed_paste_injection: true,
            tool_prefix: "mcp__cas__",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let cwd = launch.cwd.clone();
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
        apply_launch_options(&mut config, launch.cas_root, launch.name, false);
        if let Some(account) = super::skill_catalog::claude_account(launch.config_dir) {
            super::skill_catalog::apply_claude(&mut config, &account, &cwd);
        }
        apply_worker_sandbox(&mut config, &cwd);
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            launch.config_dir,
            launch.cas_root,
        );
        crate::gateway::apply(&mut config, launch.cas_root, launch.name);
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
        apply_launch_options(&mut config, launch.cas_root, launch.name, true);
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
            Ok(
                cas_pty::ClaudeTrustOutcome::Added(_) | cas_pty::ClaudeTrustOutcome::AlreadyPresent,
            ) => Ok(()),
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

/// cas-3751a: complete the worker settings' `sandbox` block with this
/// checkout's exact paths and mark the harness as sandboxed, so the stage-A
/// text guards only log for Bash. cas-f157: Cassy's launch inputs are removed
/// from the settings, the checkout's own scratch is granted, and the granted
/// tasks are named in the harness environment so a start of any other task
/// relaunches the worker. Settings without a `sandbox` block (Linux, sandbox
/// off, no settings file) launch unchanged.
fn apply_worker_sandbox(config: &mut PtyConfig, cwd: &Path) {
    use crate::worker_sandbox;
    let Some(flag) = config.args.iter().position(|arg| arg == "--settings") else {
        return;
    };
    let raw = &config.args[flag + 1];
    let text = if raw.trim_start().starts_with('{') {
        raw.clone()
    } else {
        match std::fs::read_to_string(raw) {
            Ok(text) => text,
            Err(_) => return,
        }
    };
    let Ok(serde_json::Value::Object(mut settings)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let launch: worker_sandbox::LaunchInputs = settings
        .remove(worker_sandbox::LAUNCH_KEY)
        .and_then(|inputs| serde_json::from_value(inputs).ok())
        .unwrap_or_default();
    let Some(sandbox) = settings.get_mut("sandbox") else {
        return;
    };
    let git = worker_sandbox::git_paths(cwd);
    for dir in git.iter().flat_map(|git| git.create.iter()) {
        if let Err(error) = std::fs::create_dir_all(dir) {
            tracing::warn!(path = %dir.display(), %error, "worker sandbox: could not pre-create git dir");
        }
    }
    worker_sandbox::apply_worktree(sandbox, cwd, git.as_ref(), &launch.checkout_scratch(cwd));
    config.args[flag + 1] = serde_json::Value::Object(settings).to_string();
    config.env.push((worker_sandbox::SANDBOX_ENV.into(), worker_sandbox::SHADOW.into()));
    if let Some(tasks) = &launch.tasks {
        config.env.push((worker_sandbox::TASKS_ENV.into(), tasks.join(",")));
    }
}

/// One role-aware application for initial, dynamic and recycled Claude panes.
fn apply_launch_options(
    config: &mut PtyConfig,
    root: Option<&std::path::PathBuf>,
    name: &str,
    supervisor: bool,
) {
    let result = (|| -> std::result::Result<(), String> {
        let options =
            crate::worker_resources::load_claude_config(root.map(std::path::PathBuf::as_path))
                .map_err(|error| error.to_string())?;
        let mut flags = Vec::new();
        if supervisor && options.remote_control {
            flags.push(("--remote-control", name.to_owned()));
        }
        if let Some(model) = options.fallback_model {
            flags.push(("--fallback-model", model));
        }
        for (flag, value) in flags {
            if !cas_pty::claude_supports_flag(flag) {
                return Err(format!(
                    "Claude launch requested {flag}, but the installed CLI does not support it; upgrade Claude or clear the corresponding factory.claude option"
                ));
            }
            config.args.extend([flag.to_owned(), value]);
        }
        Ok(())
    })();
    if let Err(reason) = result {
        // Pty::spawn checks this before discovery for every role.
        config
            .env
            .push(("CAS_FACTORY_WORKER_LAUNCH_ERROR".into(), reason));
    }
}
