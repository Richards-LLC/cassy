//! `cas accounts`: one shared base folder for every Claude and Codex account
//! (cas-e1f4). The engine is [`crate::accounts`].
use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use crate::accounts::{self, Provider};
use crate::cli::Cli;

#[derive(Args, Debug, Clone)]
pub struct AccountsArgs {
    #[command(subcommand)]
    pub command: AccountsCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum AccountsCommand {
    /// Link every account's shared items (commands, skills, settings, project
    /// memory, ...) to one base folder; logins stay per account. Retires
    /// ~/.claude, ~/.claude-alt and ~/.codex as accounts. Everything replaced
    /// is backed up first, with a restore script.
    Link {
        /// Print the plan without changing anything
        #[arg(long)]
        dry_run: bool,
        /// Which provider's accounts
        #[arg(long, value_enum, default_value_t = ProviderArg::All)]
        provider: ProviderArg,
    },
    /// List shared items in account folders that no longer link to the base
    Drift {
        #[arg(long, value_enum, default_value_t = ProviderArg::All)]
        provider: ProviderArg,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderArg {
    All,
    Claude,
    Codex,
}

impl ProviderArg {
    fn providers(self) -> Vec<Provider> {
        match self {
            Self::All => Provider::ALL.to_vec(),
            Self::Claude => vec![Provider::Claude],
            Self::Codex => vec![Provider::Codex],
        }
    }
}

pub fn execute(args: &AccountsArgs, cli: &Cli) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    match &args.command {
        AccountsCommand::Link { dry_run, provider } => {
            let plans = provider
                .providers()
                .into_iter()
                .map(|provider| accounts::plan(provider, &home))
                .collect::<Result<Vec<_>>>()?;
            if *dry_run {
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&plans)?);
                } else {
                    for plan in &plans {
                        print!("{}", accounts::render_plan(plan, &home));
                    }
                    println!("Dry run: nothing changed. Apply with `cas accounts link`.");
                }
                return Ok(());
            }
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let mut applied = Vec::new();
            for plan in &plans {
                if !cli.json {
                    print!("{}", accounts::render_plan(plan, &home));
                }
                if plan.is_empty() {
                    continue;
                }
                let done = accounts::apply(plan, &home, &stamp)?;
                if !cli.json {
                    print!(
                        "{}",
                        crate::cloud::presence::wrap_line(&format!(
                        "[OK] {}: {} file{} merged ({} conflict{} kept side by side), {} link{}, {} dir change{}",
                        plan.provider.name(),
                        done.merged,
                        if done.merged == 1 { "" } else { "s" },
                        done.conflicts,
                        if done.conflicts == 1 { "" } else { "s" },
                        done.linked,
                        if done.linked == 1 { "" } else { "s" },
                        done.dir_changes,
                        if done.dir_changes == 1 { "" } else { "s" },
                    ))
                    );
                    println!("  Backup: {}", done.backup.display());
                    println!("  Undo:   bash {}", done.restore.display());
                }
                applied.push(done);
            }
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "plans": plans,
                        "applied": applied,
                    }))?
                );
            }
            Ok(())
        }
        AccountsCommand::Drift { provider } => {
            let mut found = Vec::new();
            for provider in provider.providers() {
                found.extend(accounts::drift(provider, &home)?);
            }
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&found)?);
            } else if !provider
                .providers()
                .iter()
                .any(|provider| provider.base_dir(&home).is_dir())
            {
                println!("[OK] accounts do not share a base folder yet");
                println!("Preview one: cas accounts link --dry-run");
            } else if found.is_empty() {
                println!("[OK] every account links its shared items to the base");
            } else {
                println!(
                    "[WARN] {} shared item{} not linked to the base",
                    found.len(),
                    if found.len() == 1 { "" } else { "s" }
                );
                for drift in &found {
                    println!("  ~/{}/{}", drift.account, drift.item);
                }
                println!("Fix: cas accounts link --dry-run, then cas accounts link");
            }
            Ok(())
        }
    }
}
