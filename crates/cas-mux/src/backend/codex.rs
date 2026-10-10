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
            launch.cas_root,
        );
        config.args.push("-c".to_string());
        config.args.push(format!(
            "mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"{}\"",
            launch.supervisor_cli.backend().name()
        ));
        // cas-8ec53 (GH #985): Codex starts its MCP servers with a restricted
        // environment, so the account dir finish_worker_config put in the pane
        // env never reached `cas serve`. Registration then recorded no
        // worker_account_dir, and worker_status printed "default/inherited"
        // for a worker pinned to another account. Forward the same value
        // explicitly, as for the other factory identity fields. A JSON string
        // literal is a valid TOML basic string for any path.
        if let Some(config_dir) = launch
            .config_dir
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
        {
            let value =
                serde_json::to_string(config_dir).expect("serializing a string cannot fail");
            config.args.push("-c".to_string());
            config.args.push(format!(
                "mcp_servers.cs.env.CAS_FACTORY_WORKER_ACCOUNT_DIR={value}"
            ));
        }
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

    fn launch(config_dir: Option<&str>) -> crate::pty::PtyConfig {
        CODEX.build_worker_config(super::WorkerLaunchConfig {
            name: "codex-alt-worker",
            cwd: std::env::temp_dir(),
            cas_root: None,
            supervisor_name: "supervisor",
            supervisor_cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir,
            config_dir_source: config_dir.map(|_| "explicit"),
            secure_storage_dir: None,
            teams: None,
            active_workers: None,
        })
    }

    /// cas-8ec53 (GH #985): a Codex worker spawned with `config_dir` must
    /// carry that account into its `cs` MCP server, which does not inherit
    /// the pane env, so registration records it and worker_status shows it.
    #[test]
    fn codex_worker_forwards_its_account_dir_to_the_cas_mcp_server_cas_8ec53() {
        let config = launch(Some("~/.codex-alt"));
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_WORKER_ACCOUNT_DIR=\"~/.codex-alt\""),
            "{all_args}"
        );
        assert!(
            config
                .env
                .iter()
                .any(|(key, value)| key == "CAS_FACTORY_WORKER_ACCOUNT_DIR"
                    && value == "~/.codex-alt"),
            "the pane env keeps the same value: {:?}",
            config.env
        );

        let inherited = launch(None).args.join(" ");
        assert!(
            !inherited.contains("CAS_FACTORY_WORKER_ACCOUNT_DIR"),
            "an inherited account stays unset: {inherited}"
        );
    }

    fn launch_in(cwd: std::path::PathBuf, config_dir: &str) -> crate::pty::PtyConfig {
        CODEX.build_worker_config(super::WorkerLaunchConfig {
            name: "codex-dedupe-worker",
            cwd,
            cas_root: None,
            supervisor_name: "supervisor",
            supervisor_cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: Some(config_dir),
            config_dir_source: Some("explicit"),
            secure_storage_dir: None,
            teams: None,
            active_workers: None,
        })
    }

    fn disable_arg(key: &str) -> String {
        format!("mcp_servers.{key}={{ command = \"cas\", args = [\"serve\"], enabled = false }}")
    }

    /// cas-8a20: a Codex worker loads every `[mcp_servers.*]` layer plus the
    /// spawn-injected `cs` server. A user-level `[mcp_servers.cas]` (the
    /// historical `cas update` key) or any other Cassy entry under a key other
    /// than `cs` started a second `cas serve` per worker, doubling SQLite
    /// write contention. The launch must disable every shadow Cassy server so
    /// exactly one (`cs`) runs, while leaving non-Cassy servers untouched.
    #[test]
    fn codex_worker_disables_shadow_cas_servers_cas_8a20() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex-home");
        let worktree = root.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(worktree.join(".codex")).unwrap();
        std::fs::create_dir_all(worktree.join(".git")).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[mcp_servers.cas]\ncommand = \"cas\"\nargs = [\"serve\"]\n\n\
             [mcp_servers.cas.env]\nCAS_CODEX_FALLBACK_SESSION = \"1\"\n\n\
             [mcp_servers.context7]\nurl = \"https://mcp.context7.com/mcp\"\n\n\
             [mcp_servers.already_off]\ncommand = \"cas\"\nargs = [\"serve\"]\nenabled = false\n",
        )
        .unwrap();
        std::fs::write(
            worktree.join(".codex/config.toml"),
            "[mcp_servers.cs]\ncommand = \"cas\"\nargs = [\"serve\"]\n\n\
             [mcp_servers.legacy]\ncommand = \"/usr/local/bin/cas\"\nargs = [\"serve\"]\n\n\
             [mcp_servers.\"dotted.cas\"]\ncommand = \"cas\"\nargs = [\"serve\"]\n\n\
             [mcp_servers.neon]\ncommand = \"npx\"\nargs = [\"-y\", \"neon\"]\n",
        )
        .unwrap();

        let config = launch_in(worktree.clone(), home.to_str().unwrap());
        let args = &config.args;

        for key in ["cas", "legacy"] {
            let expected = disable_arg(key);
            assert!(
                args.windows(2).any(|pair| pair[0] == "-c" && pair[1] == expected),
                "shadow Cassy server `{key}` must be disabled with `{expected}`: {args:?}"
            );
        }
        let dotted = "mcp_servers={ \"dotted.cas\" = { command = \"cas\", args = [\"serve\"], enabled = false } }";
        assert!(
            args.windows(2).any(|pair| pair[0] == "-c" && pair[1] == dotted),
            "a dotted shadow key is addressed through the parent table: {args:?}"
        );
        for key in ["cs", "context7", "neon", "already_off"] {
            assert!(
                !args.iter().any(|arg| arg.starts_with(&format!("mcp_servers.{key}="))
                    || arg.starts_with(&format!("mcp_servers.{key}.enabled"))),
                "`{key}` must not be disabled: {args:?}"
            );
        }
        assert!(
            args.iter().any(|arg| arg == "mcp_servers.cs.command=\"cas\""),
            "the single canonical `cs` server stays injected: {args:?}"
        );
    }

    /// cas-8a20: a project with no `.codex/config.toml` and a user config
    /// without any Cassy entry still gets the injected `cs` server and no
    /// disable overrides at all.
    #[test]
    fn codex_worker_without_project_config_keeps_injected_cs_cas_8a20() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex-home");
        let worktree = root.path().join("bare-project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(worktree.join(".git")).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[mcp_servers.context7]\nurl = \"https://mcp.context7.com/mcp\"\n",
        )
        .unwrap();

        let config = launch_in(worktree, home.to_str().unwrap());
        let all_args = config.args.join(" ");
        assert!(all_args.contains("mcp_servers.cs.command=\"cas\""), "{all_args}");
        assert!(all_args.contains("mcp_servers.cs.args=[\"serve\"]"), "{all_args}");
        assert!(!all_args.contains("enabled = false"), "{all_args}");
    }

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
