//! Machine-local account catalog and allowlisted lifecycle. Browser keys are
//! provider-qualified aliases; credentials and absolute profile paths never leave here.
use crate::account_registry::{Account, LoginState, UsageWindow};
use crate::accounts::Provider;
use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize)]
pub(super) struct Catalog {
    pub accounts: Vec<Row>,
}
#[derive(Serialize)]
pub(super) struct Row {
    key: String,
    provider: String,
    name: String,
    display_name: String,
    /// Where the profile lives, relative to the machine's home ("~/.claude-work").
    /// Never absolute: a folder outside home shows its own name only.
    location: String,
    login_state: LoginState,
    disabled: bool,
    duplicate_of: Option<String>,
    is_default: bool,
    recommendation: Option<String>,
    windows: Vec<UsageWindow>,
    usage_observed_at: Option<DateTime<Utc>>,
    /// None means supported; a string explains the refusal.
    capabilities: BTreeMap<&'static str, Option<&'static str>>,
}
const OPERATIONS: [&str; 5] = ["remove", "disable", "enable", "rename", "set_default"];

pub(super) fn catalog(home: &Path) -> Result<Catalog> {
    let accounts = crate::account_registry::collect(home)?;
    let config = crate::config::Config::load(&home.join(".cas"))?;
    let defaults = config.hub.as_ref().and_then(|h| h.launch_profiles.as_ref());
    let policy = crate::account_selection::load_policy(None);
    let mut rows = Vec::new();
    for provider in Provider::ALL {
        let selection =
            crate::account_selection::recommend(&accounts, provider, Utc::now(), &policy);
        let configured = defaults.and_then(|d| match provider {
            Provider::Claude => d.claude.as_deref(),
            Provider::Codex => d.codex.as_deref(),
        });
        let ambient = std::env::var_os(match provider {
            Provider::Claude => "CLAUDE_CONFIG_DIR",
            Provider::Codex => "CODEX_HOME",
        });
        let ambient = ambient.as_ref().and_then(|p| {
            let cli = match provider {
                Provider::Claude => cas_mux::SupervisorCli::Claude,
                Provider::Codex => cas_mux::SupervisorCli::Codex,
            };
            super::launch_env::profile_name_from_dir(cli, home, Path::new(p))
        });
        let default = configured.or(ambient.as_deref()).unwrap_or("main");
        for account in accounts.iter().filter(|a| a.provider == provider) {
            let recommendation = selection
                .recommended
                .as_ref()
                .filter(|r| r.account.name == account.name)
                .map(|r| r.reason.clone());
            rows.push(project(account, &accounts, default, recommendation, home));
        }
    }
    rows.push(Row {
        key: "grok:default".into(),
        provider: "grok".into(),
        name: "default".into(),
        display_name: "Grok".into(),
        location: "~/.grok".into(),
        login_state: LoginState::Unknown,
        disabled: false,
        duplicate_of: None,
        is_default: true,
        recommendation: None,
        windows: Vec::new(),
        usage_observed_at: None,
        capabilities: OPERATIONS
            .into_iter()
            .map(|op| {
                (
                    op,
                    Some("Grok has one account; named account management is unavailable."),
                )
            })
            .collect(),
    });
    Ok(Catalog { accounts: rows })
}

fn project(
    account: &Account,
    accounts: &[Account],
    default: &str,
    recommendation: Option<String>,
    home: &Path,
) -> Row {
    let protected = account.name == "main";
    let capabilities = OPERATIONS.into_iter().map(|op| {
        let reason = match op {
            "remove" | "rename" if protected => Some("The main account is protected."),
            "rename" if cfg!(target_os="macos") && account.provider == Provider::Claude => Some("Claude logins on macOS are tied to the profile name. Create a new login instead."),
            "disable" if account.flags.disabled => Some("Already disabled."),
            "enable" if !account.flags.disabled => Some("Already enabled."),
            _ => None,
        }; (op, reason)
    }).collect();
    Row {
        key: format!("{}:{}", account.provider.name(), account.name),
        provider: account.provider.name().into(),
        name: account.name.clone(),
        display_name: account.display_name.clone(),
        location: account.folder.strip_prefix(home).map_or_else(
            |_| account.folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            |relative| format!("~/{}", relative.display()),
        ),
        login_state: account.login_state,
        disabled: account.flags.disabled,
        duplicate_of: account
            .flags
            .duplicate_of
            .as_ref()
            .and_then(|path| {
                accounts
                    .iter()
                    .find(|a| a.provider == account.provider && &a.folder == path)
            })
            .map(|a| format!("{}:{}", a.provider.name(), a.name)),
        is_default: account.name == default,
        recommendation,
        windows: account.windows.clone(),
        usage_observed_at: account.usage_observed_at,
        capabilities,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    pub operation: String,
    pub account: String,
    pub confirmation: Option<String>,
    pub new_name: Option<String>,
}
impl Operation {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            OPERATIONS.contains(&self.operation.as_str()),
            "Unsupported account operation"
        );
        let (provider, name) = self
            .account
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("Use a provider-qualified account alias"))?;
        ensure!(
            ["claude", "codex"].contains(&provider),
            "This provider has no named account management"
        );
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && !name.contains(['/', '\\', ':'])
                && !name.chars().any(char::is_control)
                && ![".", ".."].contains(&name),
            "Invalid account alias"
        );
        if ["remove", "disable"].contains(&self.operation.as_str()) {
            ensure!(
                self.confirmation.as_deref() == Some(&self.account),
                "Confirm the provider-qualified account name"
            );
        }
        if self.operation == "rename" {
            let name = self
                .new_name
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("A new account name is required"))?;
            ensure!(
                name == name.trim(),
                "Account name cannot contain surrounding whitespace"
            );
            crate::cli::account_picker::validate_new_profile_name(name, "main")
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}

/// Re-resolve under the hub's operation lock. Reuse the CLI's lifecycle guards
/// and shared-base handling; a browser can never supply a filesystem path.
pub(super) fn apply(home: &Path, op: &Operation) -> Result<()> {
    op.validate()?;
    let target = crate::account_lifecycle::resolve(home, &op.account)?;
    // Load before touching the profile, so malformed configuration fails closed.
    let root = home.join(".cas");
    let mut config = crate::config::Config::load(&root)?;
    let mut defaults = config
        .hub
        .clone()
        .unwrap_or_default()
        .launch_profiles
        .unwrap_or_default();
    let selected = match target.provider {
        Provider::Claude => &mut defaults.claude,
        Provider::Codex => &mut defaults.codex,
    };
    let was_default = selected.as_deref() == Some(&target.name);
    let mut save_default = false;
    match op.operation.as_str() {
        "remove" => {
            crate::account_lifecycle::remove(&target, home)?;
            if was_default {
                *selected = None;
                save_default = true;
            }
        }
        "rename" => {
            let name = op.new_name.as_deref().unwrap();
            crate::account_lifecycle::rename(&target, home, name)?;
            if was_default {
                *selected = Some(name.into());
                save_default = true;
            }
        }
        "disable" | "enable" => {
            crate::account_lifecycle::set_disabled(home, &target.folder, op.operation == "disable")?
        }
        "set_default" => {
            *selected = Some(target.name);
            save_default = true;
        }
        _ => bail!("Unsupported account operation"),
    }
    if save_default {
        config
            .hub
            .get_or_insert_with(Default::default)
            .launch_profiles = Some(defaults);
        std::fs::create_dir_all(&root)?;
        config.save(&root)?;
    }
    Ok(())
}
