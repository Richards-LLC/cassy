//! Read-only source navigation; rule lifecycle operations remain in MCP.
use std::{collections::BTreeSet, process::Command};

use anyhow::{Context, bail};
use clap::Subcommand;

use super::Cli;

#[derive(Debug, Subcommand)]
pub enum RulesCommands {
    /// List tracked source/test comments citing a rule, without accessing credentials
    Refs { rule_id: String },
}

fn bounded(text: &str, full: bool) -> String {
    if full || ratatui::text::Line::raw(text).width() <= 78 {
        return text.into();
    }
    let mut width = 0;
    let prefix: String = text
        .chars()
        .take_while(|character| {
            width += ratatui::text::Span::raw(character.to_string()).width();
            width <= 75
        })
        .collect();
    format!("{prefix}...")
}

pub fn execute(command: &RulesCommands, cli: &Cli) -> anyhow::Result<()> {
    let RulesCommands::Refs { rule_id } = command;
    let root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("cannot find repository")?;
    if !root.status.success() {
        bail!("Rule references need a Git repository. Run from its checkout.");
    }
    let root = String::from_utf8(root.stdout)?;
    let root = std::path::PathBuf::from(root.strip_suffix('\n').unwrap_or(&root));
    let sites = crate::rule_refs::scan(&root, None, &BTreeSet::from([rule_id.clone()])).map_err(
        |error| {
            let detail = error
                .to_string()
                .lines()
                .map(|line| format!("  {}", bounded(line, cli.full || cli.json)))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::anyhow!(
                "Rule references unavailable\n{detail}\nUse --full for complete diagnostics."
            )
        },
    )?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&sites)?);
    } else {
        println!("{} rule reference(s)", sites.len());
        println!("Use --full or --json for complete paths and scopes.");
        for site in sites {
            let text = format!(
                "{}:{}  {}",
                serde_json::to_string(&site.path)?,
                site.line,
                serde_json::to_string(&site.scope)?
            );
            println!("  {}", bounded(&text, cli.full));
        }
    }
    Ok(())
}
