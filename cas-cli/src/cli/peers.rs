//! Terminal counterpart of `coordination action=peers`.
use std::path::Path;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct PeersArgs {
    /// Include peer task claims, live workers and branches
    #[arg(long)]
    pub detail: bool,
    /// Show one supervisor (name or name@machine); implies --detail
    #[arg(long)]
    pub peer: Option<String>,
}

pub fn execute(args: &PeersArgs, cli: &super::Cli, cas_root: &Path) -> Result<()> {
    use crate::cloud::peers_detail::{ascii_terminal, discover_peer_report, render_peer_report};
    // Read-only identity resolution: never register the CLI as an agent.
    let self_id = std::env::var("CAS_SESSION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| crate::agent_id::read_session_for_mcp(cas_root).ok());
    let report = discover_peer_report(
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
