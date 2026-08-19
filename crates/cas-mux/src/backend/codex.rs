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

    fn prepare_workdir(&self, cwd: &Path, config_dir: Option<&str>) -> Result<()> {
        // A per-worker CODEX_HOME is only installed in the child environment
        // below. Trust must therefore target that explicit home here, before
        // the CLI starts; consulting the daemon's own home leaves alternate
        // accounts at Codex's interactive hooks-review prompt.
        let trust = match config_dir.filter(|dir| !dir.trim().is_empty()) {
            Some(home) => {
                let config = Path::new(home).join("config.toml");
                match cas_pty::ensure_project_trusted_in(&config, cwd)? {
                    cas_pty::CodexTrustOutcome::Added(_)
                    | cas_pty::CodexTrustOutcome::AlreadyPresent => {
                        cas_pty::ensure_cas_hooks_trusted_in(
                            &config,
                            &Path::new(home).join("hooks.json"),
                        )?;
                        Ok(())
                    }
                    cas_pty::CodexTrustOutcome::Skipped(reason) => Err(Error::pty(format!(
                        "refusing to launch Codex before its project trust is verified: {reason}"
                    ))),
                }
            }
            None => match cas_pty::ensure_project_trusted(cwd)? {
                cas_pty::CodexTrustOutcome::Added(_)
                | cas_pty::CodexTrustOutcome::AlreadyPresent => Ok(()),
                cas_pty::CodexTrustOutcome::Skipped(reason) => Err(Error::pty(format!(
                    "refusing to launch Codex before its project trust is verified: {reason}"
                ))),
            },
        };
        trust
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

#[cfg(test)]
mod tests {
    use super::{Backend, CODEX};

    #[test]
    fn alternate_codex_home_trusts_project_and_home_hook_paths() {
        let root = std::env::temp_dir().join(format!(
            "cas-mux-alt-codex-home-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let worktree = root.join("worktree");
        let home = root.join("alt-home");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse"}]}],"PostToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse"}]}]}}"#,
        )
        .unwrap();

        CODEX
            .prepare_workdir(&worktree, Some(home.to_str().unwrap()))
            .unwrap();

        let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(config.contains("trust_level = \"trusted\""));
        let hooks = home.join("hooks.json").to_string_lossy().to_string();
        assert!(config.contains(&hooks));
        assert!(config.contains(":pre_tool_use:0:0"));
        assert!(config.contains(":post_tool_use:0:0"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
