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
    let sites = crate::rule_refs::scan(&root, None, &BTreeSet::from([rule_id.clone()]))?;
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
            if cli.full || ratatui::text::Line::raw(text.as_str()).width() <= 78 {
                println!("  {text}");
            } else {
                // Full fidelity is the JSON contract; narrow human rows stay
                // legible and always name that escape above the first row.
                let mut width = 0;
                let prefix: String = text
                    .chars()
                    .take_while(|character| {
                        width += ratatui::text::Span::raw(character.to_string()).width();
                        width <= 75
                    })
                    .collect();
                println!("  {prefix}...");
            }
        }
    }
    Ok(())
}
