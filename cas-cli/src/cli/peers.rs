//! Terminal counterpart of `coordination action=peers`.
use std::path::Path;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct PeersArgs {
    /// Include peer ownership, roots, PR observations and waiting merges
    #[arg(long)]
    pub detail: bool,
    /// Show one supervisor (name or name@machine); implies --detail
    #[arg(long)]
    pub peer: Option<String>,
    /// List every supervisor of your own Cassy Cloud account, in any project
    /// or machine (never another user's)
    #[arg(long, conflicts_with_all = ["detail", "peer"])]
    pub all: bool,
}

pub fn execute(args: &PeersArgs, cli: &super::Cli, cas_root: &Path) -> Result<()> {
    use crate::cloud::peers_detail::{ascii_terminal, discover_peer_report, render_peer_report};
    // Read-only identity resolution: never register the CLI as an agent.
    let self_id = crate::cloud::peers::cli_self_id(cas_root)
        .or_else(|| crate::agent_id::read_session_for_mcp(cas_root).ok());
    if args.all {
        // cas-2a5a: the account-wide directory. A failed lookup exits 1 in
        // text and JSON alike, as plain `cas peers` does.
        match crate::cloud::peers::user_peers_report(
            cas_root,
            self_id.as_deref(),
            cli.json,
            cli.verbose,
        ) {
            Ok(text) if cli.json => println!("{text}"),
            Ok(text) => print!("{text}"),
            Err(error) => {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({"scope": "user", "status": "lookup_failed", "error": error.to_string()})
                    );
                } else {
                    print!(
                        "{}",
                        crate::cloud::presence::wrap_line(&format!("[ERROR] Peers: {error}"))
                    );
                }
                anyhow::bail!("Peer discovery unavailable (lookup_failed)");
            }
        }
        return Ok(());
    }
    let mut report = discover_peer_report(
        cas_root,
        self_id.as_deref(),
        args.detail,
        args.peer.as_deref(),
    )
    .unwrap_or_else(|error| {
        let mut report = crate::cloud::peers_detail::PeerReport::new(
            cas_root,
            self_id.as_deref(),
            args.detail || args.peer.is_some(),
        );
        report.status = "lookup_failed".into();
        report.error = Some(error.to_string());
        report
    });
    report.activation = crate::cloud::activation::observe(cas_root, self_id.as_deref());
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let width = crossterm::terminal::size()
            .ok()
            .map(|(w, _)| usize::from(w))
            .or_else(|| std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()))
            .unwrap_or(80);
        print!("{}", render_peer_report(&report, width, ascii_terminal()));
    }
    if report.error.is_some() {
        anyhow::bail!("Peer discovery unavailable ({})", report.status);
    }
    Ok(())
}
