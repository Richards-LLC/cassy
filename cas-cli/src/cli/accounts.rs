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
    /// List Claude and Codex subscriptions, usage, resets and login findings
    List,
    /// Permanently delete a named account (shared base items stay intact)
    Remove {
        /// Folder alias or email; use claude:<name> or codex:<name> if ambiguous
        account: String,
        /// Skip the default-no confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Move a named account to a new folder alias
    Rename { account: String, new_name: String },
    /// Keep a login but exclude it from automatic account selection
    Disable { account: String },
    /// Allow automatic selection of an account again
    Enable { account: String },
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
        AccountsCommand::Remove { account, yes } => {
            let target = crate::account_lifecycle::resolve(&home, account)?;
            crate::account_lifecycle::ensure_idle(&target, &home)?;
            if !yes {
                // Prompts go to stderr so --json still emits one document.
                use std::io::Write;
                eprintln!(
                    "Delete {} account {}?",
                    target.provider.name(),
                    target.email.as_deref().unwrap_or(&target.name)
                );
                eprintln!("Folder: {}", target.folder.display());
                for alias in crate::account_lifecycle::removal_aliases(&target, &home)? {
                    eprintln!("Alias removed too: {}", alias.display());
                }
                eprint!("Permanently remove this folder? [y/N] ");
                std::io::stderr().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    return mutation_receipt(cli.json, "cancelled", &target);
                }
            }
            crate::account_lifecycle::remove(&target, &home)?;
            mutation_receipt(cli.json, "removed", &target)
        }
        AccountsCommand::Rename { account, new_name } => {
            let mut target = crate::account_lifecycle::resolve(&home, account)?;
            crate::account_lifecycle::rename(&target, &home, new_name)?;
            target.name = new_name.clone();
            target.folder = home.join(format!("{}{new_name}", target.provider.prefix()));
            mutation_receipt(cli.json, "renamed", &target)
        }
        AccountsCommand::Disable { account } | AccountsCommand::Enable { account } => {
            let target = crate::account_lifecycle::resolve(&home, account)?;
            let disabled = matches!(args.command, AccountsCommand::Disable { .. });
            crate::account_lifecycle::set_disabled(&home, &target.folder, disabled)?;
            mutation_receipt(
                cli.json,
                if disabled { "disabled" } else { "enabled" },
                &target,
            )
        }
        AccountsCommand::List => {
            let accounts = crate::account_registry::collect(&home)?;
            print!("{}", render_list(&accounts, cli.json, &home)?);
            Ok(())
        }
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

/// Both the command and its contract test use this serialization/render path.
fn render_list(
    accounts: &[crate::account_registry::Account],
    json: bool,
    home: &std::path::Path,
) -> Result<String> {
    if json {
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({"accounts": accounts}))?
        ));
    }
    let flagged = accounts
        .iter()
        .filter(|a| {
            a.flags.duplicate_of.is_some()
                || a.flags.never_logged_in
                || a.flags.stale_login
                || a.flags.disabled
        })
        .count();
    let mut out = format!(
        "[{}] {} accounts, {} flagged\n",
        if flagged == 0 { "OK" } else { "WARN" },
        accounts.len(),
        flagged
    );
    for provider in Provider::ALL {
        if provider != Provider::Claude {
            out.push('\n');
        }
        out.push_str(&format!("{}\n", provider.name()));
        out.push_str(&format!("  {:<34} {:<12} {}\n", "ACCOUNT", "PLAN", "LOGIN"));
        for a in accounts.iter().filter(|a| a.provider == provider) {
            use crate::account_registry::LoginState;
            let login = match a.login_state {
                LoginState::LoggedIn => "logged in",
                LoginState::LoggedOut => "logged out",
                LoginState::NeverLoggedIn => "never logged in",
                LoginState::StaleLogin => "stale login",
                LoginState::Unknown => "login unknown",
            };
            out.push_str(&format!(
                "  {:<34} {:<12} {}\n",
                cell(&a.display_name, 34),
                cell(a.plan.as_deref().unwrap_or("unknown"), 12),
                login
            ));
            let folder = a
                .folder
                .strip_prefix(home)
                .map(|rest| format!("~/{}", rest.display()))
                .unwrap_or_else(|_| a.folder.display().to_string());
            out.push_str(&format!("    folder: {}\n", cell(&folder, 64)));
            for name in ["5h", "weekly"] {
                let w = a.windows.iter().find(|w| w.name == name);
                if w.and_then(|w| w.resets_at)
                    .is_some_and(|reset| reset <= chrono::Utc::now())
                {
                    out.push_str(&format!(
                        "    {name:>6}: reset, usage unknown until next session\n"
                    ));
                    continue;
                }
                let used = w
                    .and_then(|w| w.used_percent)
                    .map(|used| format!("{used:.0}%"));
                let reset = w
                    .and_then(|w| w.resets_at)
                    .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string());
                out.push_str(&format!(
                    "    {name:>6}: {:>13}  resets {}\n",
                    used.as_deref().unwrap_or("usage unknown"),
                    reset.as_deref().unwrap_or("unknown")
                ));
            }
            if let Some(folder) = &a.flags.duplicate_of {
                out.push_str(&format!(
                    "    duplicate of: {}\n",
                    cell(
                        &folder.file_name().unwrap_or_default().to_string_lossy(),
                        60
                    )
                ));
            }
            if a.flags.disabled {
                out.push_str("    flag: disabled\n");
            }
            if a.flags.never_logged_in {
                out.push_str("    flag: never logged in\n");
            }
            if a.flags.stale_login {
                out.push_str("    flag: credential rejected\n");
            }
        }
        if !accounts.iter().any(|a| a.provider == provider) {
            out.push_str("  No accounts found.\n");
        }
    }
    out.push_str("\nFull values: cas accounts list --json\n");
    Ok(out)
}

// Keep terminal cells ASCII (also under LC_ALL=C), one line and bounded.
// JSON preserves the exact values, including non-ASCII account names.
fn cell(value: &str, width: usize) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if value.len() <= width {
        value
    } else {
        format!("{}...", &value[..width.saturating_sub(3)])
    }
}

fn mutation_receipt(
    json: bool,
    action: &str,
    target: &crate::account_lifecycle::Target,
) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({"action": action, "provider": target.provider, "account": target.name, "folder": target.folder})
        );
    } else {
        println!(
            "[OK] {} {} account {}",
            action,
            target.provider.name(),
            cell(&target.name, 48)
        );
        if target.name.len() > 48 {
            println!("Full values: cas accounts list --json");
        }
    }
    Ok(())
}
