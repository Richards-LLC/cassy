//! `cas integrate <platform> <action>` — auto-integration scaffolding for
//! Vercel, Neon, and GitHub.
//!
//! This module is the **foundation** for EPIC cas-b65f. It owns:
//!
//! 1. The clap subcommand surface (`cas integrate vercel|neon|github init|refresh|verify`).
//! 2. Shared types in [`types`] returned by every platform handler.
//! 3. The keep-block helper in [`keep_block`] for `<!-- keep -->` … `<!-- /keep -->`
//!    block round-tripping. All three platform handlers reuse this.
//!
//! Platform handlers ([`vercel`], [`neon`], [`github`]) are full
//! implementations (cas-8e37, cas-1ece, cas-f425 — all closed). They share
//! the helpers in [`fs`], [`md`], and [`keep_block`] so atomic-write
//! semantics, symlink defense, file-size cap, markdown cell escaping, and
//! the `<!-- cas:full_name=... -->` identity tag all behave uniformly
//! across platforms (cas-fc38).
//!
//! ## Template loading convention
//!
//! Platform handlers embed their SKILL.md templates from a sibling
//! `templates/<platform>/` directory via `include_str!` at compile time
//! (cas-8e37 established this layout — neon and github mirror it):
//!
//! ```ignore
//! const TEMPLATE_CLAUDE_SKILL: &str =
//!     include_str!("templates/vercel/SKILL.md.tmpl");
//! const TEMPLATE_CURSOR_SKILL: &str =
//!     include_str!("templates/vercel/cursor.md.tmpl");
//! ```
//!
//! Templates contain `<!-- keep -->` / `<!-- /keep -->` markers around the
//! ID payloads they want preserved across regeneration. Use **named**
//! markers (e.g. `<!-- keep vercel-ids -->`) so reordering or adding a new
//! block in a future template revision does not silently misroute user
//! content. See [`keep_block::merge`] and [`keep_block::orphaned_existing`]
//! for the merge semantics and the recommended call shape for handlers.
//!
//! ## Live MCP transport
//!
//! Platform handlers backed by an upstream MCP server (vercel, neon,
//! and any future github-with-API client) wrap [`proxy::ProxyClient`]
//! rather than reimplementing the runtime + engine lifecycle. The shared
//! module owns config loading, lazy engine init, call dispatch, envelope
//! unwrap, and Drop-time shutdown (cas-36fd0). New live clients should
//! follow the `vercel::mcp_proxy_client::ProxyVercelClient` /
//! `neon::LiveNeonClient` shape: a thin wrapper over
//! `ProxyClient::new("<server>")` that maps trait calls onto
//! `ProxyClient::call(tool, args)` and a per-platform parser.

pub mod doctor;
pub mod fs;
pub mod github;
pub mod integrations;
pub mod keep_block;
pub mod lock;
pub mod md;
/// Machine-scoped MechaCassy setup. Gated on `mcp-proxy` because the whole
/// point of the command is to write a proxy registration and prove it with an
/// authenticated `tools/list`; without that feature there is nothing to write
/// against, so the subcommand reports the rebuild instead of half-working.
#[cfg(feature = "mcp-proxy")]
pub mod mecha_cassy;
pub mod neon;
#[cfg(test)]
mod neon_parsers_test;
#[cfg(feature = "mcp-proxy")]
pub mod proxy;
pub mod types;
pub mod vercel;

use clap::Subcommand;

use super::Cli;
use types::{IntegrationAction, IntegrationOutcome};

/// `cas integrate <platform>` — pick a platform.
#[derive(Subcommand, Debug)]
pub enum IntegrateCommands {
    /// Integrate the project with Vercel (project, team, env→branch mappings).
    Vercel {
        #[command(subcommand)]
        action: PlatformAction,
    },
    /// Integrate the project with Neon (project, branches, org).
    Neon {
        #[command(subcommand)]
        action: PlatformAction,
    },
    /// Integrate the project with GitHub (repo path from `git remote -v`).
    ///
    /// Uses a github-specific action enum so `init` and `refresh` can accept
    /// `--repo OWNER/REPO` to override auto-detection. See
    /// [`github::GithubAction`].
    Github {
        #[command(subcommand)]
        action: github::GithubAction,
    },
    /// Set this machine up for the MechaCassy Slack hub: one user-level proxy
    /// registration (every project inherits it), the Claude Code and Codex MCP
    /// entries, and an authenticated `tools/list` as the receipt.
    ///
    /// Takes no `init|refresh|verify` verb: the operation is idempotent, so
    /// re-running it *is* refresh, and it always ends in a verification.
    #[cfg(feature = "mcp-proxy")]
    #[command(name = "mecha-cassy")]
    MechaCassy(mecha_cassy::MechaCassyArgs),
}

/// `cas integrate <platform> <action>` — pick an action.
#[derive(Subcommand, Debug, Clone, Copy)]
pub enum PlatformAction {
    /// First-time setup: detect platform, prompt, write SKILL files.
    Init,
    /// Re-run detection; update outer content, preserve user-owned keep blocks.
    Refresh,
    /// Read recorded IDs, ping the platform's MCP, return a staleness report.
    Verify,
}

impl From<PlatformAction> for IntegrationAction {
    fn from(p: PlatformAction) -> Self {
        match p {
            PlatformAction::Init => IntegrationAction::Init,
            PlatformAction::Refresh => IntegrationAction::Refresh,
            PlatformAction::Verify => IntegrationAction::Verify,
        }
    }
}

/// CLI dispatch. Each platform handler is currently a stub that returns an
/// error pointing at its owning task — see module docs.
pub fn execute(cmd: &IntegrateCommands, _cli: &Cli) -> anyhow::Result<()> {
    let outcome = match cmd {
        IntegrateCommands::Vercel { action } => vercel::execute((*action).into())?,
        IntegrateCommands::Neon { action } => neon::execute((*action).into())?,
        IntegrateCommands::Github { action } => github::execute(action.clone())?,
        #[cfg(feature = "mcp-proxy")]
        IntegrateCommands::MechaCassy(args) => mecha_cassy::execute(args, _cli.json)?,
    };
    render_outcome(&outcome);
    Ok(())
}

fn render_outcome(outcome: &IntegrationOutcome) {
    println!(
        "{} {}: {}",
        outcome.platform.as_str(),
        outcome.action.as_str(),
        outcome.status.as_str()
    );
    for line in &outcome.summary {
        println!("  {line}");
    }
    for f in &outcome.files {
        println!("  wrote {}", f.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::integrate::types::IntegrationStatus;
    use crate::test_support::TestEnvGuard;
    use clap::Parser;

    /// Minimal clap harness so we can drive the subcommand parser without
    /// pulling in the full `Cli` from `super::Cli`.
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        cmd: IntegrateCommands,
    }

    fn parse(args: &[&str]) -> IntegrateCommands {
        let mut all = vec!["test"];
        all.extend_from_slice(args);
        TestCli::try_parse_from(all).unwrap().cmd
    }

    fn dispatch(cmd: &IntegrateCommands) -> anyhow::Result<IntegrationOutcome> {
        match cmd {
            IntegrateCommands::Vercel { action } => vercel::execute((*action).into()),
            IntegrateCommands::Neon { action } => neon::execute((*action).into()),
            IntegrateCommands::Github { action } => github::execute(action.clone()),
            // Deliberately not dispatched here: `mecha-cassy` writes to
            // machine-scoped paths outside the repo, so the parser-level tests
            // in this module must never execute it. Its behaviour is covered
            // against a tempdir + fake environment in `mecha_cassy::tests`.
            #[cfg(feature = "mcp-proxy")]
            IntegrateCommands::MechaCassy(_) => {
                anyhow::bail!("mecha-cassy is exercised in mecha_cassy::tests, not via dispatch")
            }
        }
    }

    // The three vercel_*_stub_points_at_task_cas_8e37 tests from foundation
    // (cas-e6b6) are obsolete now that cas-8e37 has implemented the vercel
    // handler. Vercel-specific behaviour lives in `vercel::tests` and exercises
    // the real init/refresh/verify paths against MockVercelClient. The neon
    // and github stub tests below are unchanged — those handlers remain stubs
    // until cas-1ece and cas-f425 land.

    #[test]
    fn vercel_subcommand_dispatches_into_handler() {
        // Without vercel.json or a configured client, init takes the
        // `not detected` Skipped path and returns Ok — i.e. the subcommand
        // is wired through to the real handler, not the old stub.
        let tmp = tempfile::TempDir::new().unwrap();
        let mut env = TestEnvGuard::new();
        env.set_current_dir(tmp.path());
        let cmd = parse(&["vercel", "init"]);
        let result = dispatch(&cmd);
        // Either Ok(Skipped) (no vercel.json + default no-mcp-proxy client
        // never reached) or an Err pointing at the mcp-proxy not built — but
        // never the old stub error.
        match result {
            Ok(o) => {
                assert!(matches!(
                    o.status,
                    IntegrationStatus::Skipped | IntegrationStatus::AlreadyConfigured
                ));
            }
            Err(e) => {
                let s = e.to_string();
                assert!(
                    !s.contains("not yet implemented"),
                    "should no longer return the foundation stub error: {s}"
                );
            }
        }
    }

    #[test]
    fn neon_subcommand_parses_cleanly() {
        // cas-1ece landed the real handler. The dispatch itself may error in
        // a sandbox (LiveNeonClient is a placeholder; no detection signals in
        // the test cwd), but parsing must succeed for all three verbs and the
        // dispatch arm must exist — assert on the error shape so a regression
        // that removes the arm or swaps it for a clap-parse error is caught.
        for action in ["init", "refresh", "verify"] {
            let cmd = parse(&["neon", action]);
            match dispatch(&cmd) {
                Ok(_) => {}
                Err(e) => {
                    let msg = format!("{e:#}").to_lowercase();
                    assert!(
                        !msg.contains("unrecognized") && !msg.contains("unknown subcommand"),
                        "{action} dispatch produced a parse-shaped error: {msg}"
                    );
                }
            }
        }
    }

    // GitHub handler is no longer a stub (cas-f425). Its full test coverage
    // lives in `super::github::tests`. We just sanity-check that the clap
    // surface still parses all three actions and that the `--repo` override
    // round-trips into the action enum.
    #[test]
    fn github_clap_surface_parses_all_actions() {
        for action in ["init", "refresh", "verify"] {
            let _ = parse(&["github", action]);
        }
    }

    #[test]
    fn github_init_accepts_repo_override_flag() {
        let cmd = parse(&["github", "init", "--repo", "Richards-LLC/gabber-studio"]);
        match cmd {
            IntegrateCommands::Github {
                action: github::GithubAction::Init { repo },
            } => assert_eq!(repo.as_deref(), Some("Richards-LLC/gabber-studio")),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn github_refresh_accepts_repo_override_flag() {
        let cmd = parse(&["github", "refresh", "--repo", "acme/widget"]);
        match cmd {
            IntegrateCommands::Github {
                action: github::GithubAction::Refresh { repo },
            } => assert_eq!(repo.as_deref(), Some("acme/widget")),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
