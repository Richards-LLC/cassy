//! One account policy shared by launchers, factory workers and Commander.
use crate::account_registry::{Account, LoginState};
use crate::accounts::Provider;
use crate::config::{AccountSelection, AccountsConfig};
use chrono::{DateTime, Utc};

#[derive(Debug)]
pub struct Recommendation<'a> {
    pub account: &'a Account,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Selection<'a> {
    pub recommended: Option<Recommendation<'a>>,
    /// Each excluded account and the reason automatic selection cannot use it.
    pub excluded: Vec<(&'a Account, String)>,
}

/// Pure selection: no clock reads, probes, environment or configuration I/O.
/// The 24-hour group is anchored at the earliest eligible reset, so ordering
/// remains deterministic even when A/B and B/C are close but A/C are not.
pub fn recommend<'a>(
    accounts: &'a [Account],
    provider: Provider,
    now: DateTime<Utc>,
    config: &AccountsConfig,
) -> Selection<'a> {
    let threshold = if (1..=100).contains(&config.threshold) {
        config.threshold
    } else {
        90
    };
    let mut result = Selection::default();
    let mut candidates = Vec::new();
    for account in accounts.iter().filter(|a| a.provider == provider) {
        match capacity(account, now, threshold) {
            Ok((reset, weekly, five)) => candidates.push((account, reset, weekly, five)),
            Err(reason) => result.excluded.push((account, reason)),
        }
    }
    let Some(earliest) = candidates.iter().map(|(_, reset, _, _)| *reset).min() else {
        return result;
    };
    if config.selection == AccountSelection::SoonestReset {
        candidates.retain(|(_, reset, _, _)| *reset <= earliest + chrono::Duration::hours(24));
    }
    candidates.sort_by(|a, b| {
        a.2.total_cmp(&b.2)
            .then(a.3.total_cmp(&b.3))
            .then(a.1.cmp(&b.1))
            .then(a.0.folder.cmp(&b.0.folder))
    });
    let (account, reset, weekly, _) = candidates[0];
    let minutes = (reset - now).num_minutes().max(1);
    let interval = if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h", (minutes + 59) / 60)
    };
    result.recommended = Some(Recommendation {
        account,
        reason: format!("resets in {interval}, {:.0}% left", 100.0 - weekly),
    });
    result
}

fn capacity(
    account: &Account,
    now: DateTime<Utc>,
    threshold: u8,
) -> Result<(DateTime<Utc>, f64, f64), String> {
    if account.flags.disabled {
        return Err("disabled".into());
    }
    if account.flags.stale_login || account.login_state == LoginState::StaleLogin {
        return Err("stale login".into());
    }
    if account.login_state != LoginState::LoggedIn {
        return Err("not logged in or login unknown".into());
    }
    let window = |name: &str| -> Result<(DateTime<Utc>, f64), String> {
        let w = account
            .windows
            .iter()
            .find(|w| w.name == name)
            .ok_or_else(|| format!("{name} usage unknown"))?;
        let used = w
            .used_percent
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
            .ok_or_else(|| format!("{name} usage unknown"))?;
        let reset = w.resets_at.ok_or_else(|| format!("{name} reset unknown"))?;
        if reset <= now {
            return Err(format!("{name} usage stale"));
        }
        if used >= f64::from(threshold) {
            return Err(format!("{name} at {used:.0}% (threshold {threshold}%)"));
        }
        Ok((reset, used))
    };
    let (_, five) = window("5h")?;
    let (reset, weekly) = window("weekly")?;
    Ok((reset, weekly, five))
}

/// Project selection/threshold override the global policy. Disabled folders
/// remain the global lifecycle registry, independent of project overrides.
pub fn load_policy(cas_root: Option<&std::path::Path>) -> AccountsConfig {
    let global = crate::config::load_global_config()
        .accounts
        .unwrap_or_default();
    let mut policy = cas_root
        .and_then(|root| crate::config::Config::load(root).ok())
        .and_then(|c| c.accounts)
        .unwrap_or_else(|| global.clone());
    policy.disabled = global.disabled;
    policy
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::account_registry::{AccountFlags, UsageWindow};

    pub(crate) fn row(
        provider: Provider,
        name: &str,
        hours: i64,
        weekly: f64,
        five: f64,
    ) -> Account {
        let now: DateTime<Utc> = "2026-10-27T12:00:00Z".parse().unwrap();
        Account {
            provider,
            name: name.into(),
            display_name: name.into(),
            folder: format!("/fixture/{}{name}", provider.prefix()).into(),
            email: None,
            identity: None,
            plan: None,
            login_state: LoginState::LoggedIn,
            flags: AccountFlags::default(),
            windows: vec![
                UsageWindow {
                    name: "5h".into(),
                    used_percent: Some(five),
                    resets_at: Some(now + chrono::Duration::hours(3)),
                },
                UsageWindow {
                    name: "weekly".into(),
                    used_percent: Some(weekly),
                    resets_at: Some(now + chrono::Duration::hours(hours)),
                },
            ],
        }
    }

    #[test]
    fn selection_table() {
        let now = "2026-10-27T12:00:00Z".parse().unwrap();
        let mut config = crate::config::Config::default();
        assert_eq!(config.get("accounts.threshold").as_deref(), Some("90"));
        for (key, value) in [
            ("accounts.selection", "most-headroom"),
            ("accounts.threshold", "80"),
        ] {
            config.set(key, value).unwrap();
            assert_eq!(config.get(key).as_deref(), Some(value));
            assert!(crate::config::registry().get(key).is_some());
        }
        assert!(config.set("accounts.threshold", "101").is_err());
        assert!(config.set("accounts.selection", "random").is_err());
        config
            .set("accounts.disabled", "/fixture/.claude-disabled")
            .unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let restored: crate::config::Config = toml::from_str(&serialized).unwrap();
        let restored = restored.accounts.unwrap();
        assert_eq!(restored.selection, AccountSelection::MostHeadroom);
        assert_eq!(restored.disabled, ["/fixture/.claude-disabled"]);
        for provider in Provider::ALL {
            let a = row(provider, "a", 6, 59.0, 20.0);
            let b = row(provider, "b", 48, 10.0, 10.0);
            let mut five_full = a.clone();
            five_full.windows[0].used_percent = Some(90.0);
            let mut weekly_full = a.clone();
            weekly_full.windows[1].used_percent = Some(90.0);
            let mut disabled = a.clone();
            disabled.flags.disabled = true;
            let mut logged_out = a.clone();
            logged_out.login_state = LoginState::LoggedOut;
            let mut stale = a.clone();
            stale.login_state = LoginState::StaleLogin;
            let mut unknown = a.clone();
            unknown.windows[0].used_percent = None;
            let mut expired = a.clone();
            expired.windows[1].resets_at = Some(now);
            let mut invalid = a.clone();
            invalid.windows[0].used_percent = Some(f64::NAN);
            let mut other_provider = a.clone();
            other_provider.provider = if provider == Provider::Claude {
                Provider::Codex
            } else {
                Provider::Claude
            };
            for (case, rows, selection, threshold, expected) in [
                (
                    "soonest reset",
                    vec![b.clone(), a.clone()],
                    AccountSelection::SoonestReset,
                    90,
                    Some("a"),
                ),
                (
                    "5h threshold",
                    vec![five_full.clone(), b.clone()],
                    AccountSelection::SoonestReset,
                    90,
                    Some("b"),
                ),
                (
                    "weekly threshold",
                    vec![weekly_full, b.clone()],
                    AccountSelection::SoonestReset,
                    90,
                    Some("b"),
                ),
                (
                    "weekly headroom within 24h",
                    vec![a.clone(), row(provider, "b", 30, 20.0, 80.0)],
                    AccountSelection::SoonestReset,
                    90,
                    Some("b"),
                ),
                (
                    "5h headroom tie",
                    vec![a.clone(), row(provider, "b", 12, 59.0, 10.0)],
                    AccountSelection::SoonestReset,
                    90,
                    Some("b"),
                ),
                (
                    "24h group anchored at earliest",
                    vec![
                        a.clone(),
                        row(provider, "b", 25, 20.0, 10.0),
                        row(provider, "c", 44, 0.0, 0.0),
                    ],
                    AccountSelection::SoonestReset,
                    90,
                    Some("b"),
                ),
                (
                    "most headroom",
                    vec![a.clone(), b.clone()],
                    AccountSelection::MostHeadroom,
                    90,
                    Some("b"),
                ),
                (
                    "configured threshold",
                    vec![a.clone(), b.clone()],
                    AccountSelection::SoonestReset,
                    50,
                    Some("b"),
                ),
                (
                    "no eligible account",
                    vec![
                        five_full,
                        disabled,
                        logged_out,
                        stale,
                        unknown,
                        expired,
                        invalid,
                        other_provider,
                    ],
                    AccountSelection::SoonestReset,
                    90,
                    None,
                ),
            ] {
                let config = AccountsConfig {
                    selection,
                    threshold,
                    ..Default::default()
                };
                let result = recommend(&rows, provider, now, &config);
                assert_eq!(
                    result.recommended.as_ref().map(|r| r.account.name.as_str()),
                    expected,
                    "{provider:?}: {case}"
                );
                if let Some(r) = result.recommended {
                    assert!(r.reason.contains("resets in"), "{case}: {}", r.reason);
                    assert!(r.reason.contains("% left"));
                } else {
                    assert_eq!(
                        result.excluded.len(),
                        rows.len() - 1,
                        "{case}: other provider must not appear"
                    );
                    assert!(result.excluded.iter().all(|(_, why)| !why.is_empty()));
                }
            }
        }
    }
}
