//! `cas default <provider>` — persist the supervisor harness without launching.
//!
//! This is the standalone "persist + exit" surface of the provider-shortcut
//! feature (cas-7f2c). The `--default` flag on `cas factory --supervisor-cli`
//! covers "launch AND persist"; this module covers "persist only" (no launch).

use clap::Args;

use crate::config::{Config, LlmConfig, LlmRoleConfig};
use crate::store::known_repos::host_cas_dir;

/// Arguments for `cas default`
#[derive(Args, Debug, Clone)]
pub struct DefaultArgs {
    /// Provider to persist as the default supervisor harness (`claude`, `codex`, or `grok`)
    pub provider: String,
}

/// Persist `[llm.supervisor] harness = "<provider>"` to `~/.cas/config.toml`
/// and print a one-line confirmation.  Exits 0 on success; non-zero on an
/// unknown provider or a filesystem error.
pub fn execute(args: &DefaultArgs) -> anyhow::Result<()> {
    let provider = validate_provider(&args.provider)?;

    let host_dir = host_cas_dir();
    std::fs::create_dir_all(&host_dir)?;

    // Config::load returns defaults only when no config exists; preserve its
    // parse error so this mutation cannot overwrite a malformed document.
    let mut config = Config::load(&host_dir)?;
    set_supervisor_harness(&mut config, provider);
    config.save(&host_dir)?;

    println!("supervisor default set to {provider}");
    Ok(())
}

/// Validate that `s` is "claude", "codex", or "grok"; return the normalised
/// lowercase string on success, or a descriptive error on failure.
pub(crate) fn validate_provider(s: &str) -> anyhow::Result<&str> {
    match s.trim() {
        "claude" => Ok("claude"),
        "codex" => Ok("codex"),
        "grok" => Ok("grok"),
        other => anyhow::bail!(
            "Unknown provider '{other}'. Use 'claude', 'codex', or 'grok'."
        ),
    }
}

/// Write `[llm.supervisor] harness = <provider>` into `config`, creating
/// the `llm` / `supervisor` sections if absent.
pub(crate) fn set_supervisor_harness(config: &mut Config, provider: &str) {
    let llm = config.llm.get_or_insert_with(LlmConfig::default);
    let supervisor = llm.supervisor.get_or_insert_with(LlmRoleConfig::default);
    supervisor.harness = Some(provider.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_provider_accepts_claude_codex_grok() {
        assert_eq!(validate_provider("claude").unwrap(), "claude");
        assert_eq!(validate_provider("codex").unwrap(), "codex");
        assert_eq!(validate_provider("grok").unwrap(), "grok");
    }

    #[test]
    fn validate_provider_trims_whitespace() {
        assert_eq!(validate_provider("  grok  ").unwrap(), "grok");
    }

    #[test]
    fn validate_provider_rejects_unknown_and_mentions_grok_in_error() {
        let err = validate_provider("openai").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown provider 'openai'"), "got: {msg}");
        assert!(msg.contains("grok"), "error should mention grok: {msg}");
    }

    #[test]
    fn set_supervisor_harness_writes_grok() {
        let mut config = Config::default();
        set_supervisor_harness(&mut config, "grok");
        assert_eq!(
            config.llm.as_ref().unwrap().supervisor.as_ref().unwrap().harness.as_deref(),
            Some("grok")
        );
    }

    #[test]
    fn execute_preserves_malformed_host_config() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let host_dir = host_cas_dir();
        std::fs::create_dir_all(&host_dir).unwrap();
        let config_path = host_dir.join("config.toml");
        let original = "[project]\naliases = []\nlse\n";
        std::fs::write(&config_path, original).unwrap();

        let error = execute(&DefaultArgs {
            provider: "codex".to_string(),
        })
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("Failed to parse config.toml at line 3"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), original);
    }
}
