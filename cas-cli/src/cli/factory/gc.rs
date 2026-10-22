//! CLI entry points for the factory GC MCP actions.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

use super::FactoryGcCommands;

pub(super) fn execute(command: &FactoryGcCommands, cli: &crate::cli::Cli, cas_root: Option<&Path>) -> Result<()> {
    let cas_root =
        cas_root.ok_or_else(|| anyhow::anyhow!("factory GC requires a Cassy project"))?;

    let request = match command {
        FactoryGcCommands::Report => json!({ "action": "gc_report", "json": cli.json, "full": cli.full }),
        FactoryGcCommands::Cleanup {
            force,
            dry_run,
            no_dry_run,
        } => json!({
            "action": "gc_cleanup",
            "force": force,
            // dry_run gates only the stricter cache/process/journey tiers;
            // earlier metadata cleanup and force-only tiers keep their gates.
            "dry_run": *dry_run || !*no_dry_run,
        }),
    };
    let request =
        serde_json::from_value::<crate::mcp::tools::service::CoordinationRequest>(request)?;

    let core = crate::mcp::CasCore::with_daemon(cas_root.to_path_buf(), None, None);
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::CasService::new(core);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response = runtime
        .block_on(service.factory(Parameters(request)))
        .map_err(|error| anyhow::Error::msg(error.message.to_string()))?;

    let mut stdout = std::io::stdout().lock();
    let mut wrote_text = false;
    for content in response.content {
        if let Some(text) = content.as_text() {
            stdout.write_all(text.text.as_bytes())?;
            wrote_text = true;
        }
    }
    if !wrote_text {
        bail!("factory GC handler returned no text report");
    }
    Ok(())
}
