//! Local bridge server for external orchestrators (e.g., OpenClaw).

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::Cli;
use std::fmt;

#[derive(Args, Debug, Clone)]
pub struct BridgeArgs {
    #[command(subcommand)]
    pub command: BridgeCommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum BridgeCommands {
    /// Run a local HTTP server exposing a small control/status API
    Serve(ServeArgs),
}

#[derive(Args, Clone)]
pub struct ServeArgs {
    /// Bind address (default: 127.0.0.1)
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Port to listen on (0 = auto)
    #[arg(long, default_value = "0")]
    pub port: u16,

    /// Optional explicit Cassy root directory (path to a `.cas/` dir).
    ///
    /// This is used as a fallback when a session has no `project_dir` metadata, or when
    /// Cassy root detection fails for that `project_dir`.
    #[arg(long)]
    pub cas_root: Option<std::path::PathBuf>,

    /// Bearer token for authorization (default: auto-generate).
    ///
    /// Reads from the `CAS_SERVE_TOKEN` env var if not passed on argv. Prefer
    /// the env var: passing the token on argv exposes it via `/proc/<pid>/cmdline`,
    /// `systemctl status`, and `ps -ef`.
    #[arg(long, env = "CAS_SERVE_TOKEN")]
    pub token: Option<String>,

    /// Disable authorization (not recommended; still binds to localhost by default)
    #[arg(long)]
    pub no_auth: bool,

    /// Set CORS allow-origin header (e.g., "*" or "https://openclaw.ai")
    #[arg(long)]
    pub cors_allow_origin: Option<String>,
}

pub fn execute(args: &BridgeArgs, cli: &Cli) -> Result<()> {
    match &args.command {
        BridgeCommands::Serve(s) => crate::bridge::server::serve(s, cli),
    }
}

// `--token` is a bearer; clap's derived Debug would print it whenever these
// args are logged or included in a panic message.
impl fmt::Debug for ServeArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServeArgs")
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field("cas_root", &self.cas_root)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod credential_redaction_tests {
    use super::*;

    #[test]
    fn the_serve_args_debug_never_prints_the_bearer() {
        let args = ServeArgs {
            bind: "127.0.0.1".to_string(),
            port: 0,
            cas_root: None,
            token: Some("SECRET-tok-9f3a1c".to_string()),
            no_auth: false,
            cors_allow_origin: None,
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains("SECRET-tok-9f3a1c"), "{rendered}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(rendered.contains("127.0.0.1"), "{rendered}");
    }
}
