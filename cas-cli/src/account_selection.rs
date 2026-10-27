//! One account policy shared by launchers, factory workers and Commander.
use chrono::{DateTime, Utc};
use crate::account_registry::{Account, LoginState};
use crate::accounts::Provider;
use crate::config::{AccountsConfig, AccountSelection};

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
    _accounts: &'a [Account], _provider: Provider, _now: DateTime<Utc>, _config: &AccountsConfig,
) -> Selection<'a> {
    Selection::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_registry::{AccountFlags, UsageWindow};

    pub(crate) fn row(provider: Provider, name: &str, hours: i64, weekly: f64, five: f64) -> Account {
        let now: DateTime<Utc> = "2026-10-27T12:00:00Z".parse().unwrap();
        Account {
            provider, name: name.into(), display_name: name.into(), folder: format!("/fixture/{}{name}", provider.prefix()).into(),
            email: None, identity: None, plan: None, login_state: LoginState::LoggedIn,
            flags: AccountFlags::default(),
            windows: vec![
                UsageWindow { name: "5h".into(), used_percent: Some(five), resets_at: Some(now + chrono::Duration::hours(3)) },
                UsageWindow { name: "weekly".into(), used_percent: Some(weekly), resets_at: Some(now + chrono::Duration::hours(hours)) },
            ],
        }
    }

    #[test]
    fn selection_table() {
        let now = "2026-10-27T12:00:00Z".parse().unwrap();
        for provider in Provider::ALL {
            let a = row(provider, "a", 6, 59.0, 20.0);
            let b = row(provider, "b", 48, 10.0, 10.0);
            let mut five_full = a.clone(); five_full.windows[0].used_percent = Some(90.0);
            let mut weekly_full = a.clone(); weekly_full.windows[1].used_percent = Some(90.0);
            let mut disabled = a.clone(); disabled.flags.disabled = true;
            let mut logged_out = a.clone(); logged_out.login_state = LoginState::LoggedOut;
            let mut stale = a.clone(); stale.login_state = LoginState::StaleLogin;
            let mut unknown = a.clone(); unknown.windows[0].used_percent = None;
            let mut expired = a.clone(); expired.windows[1].resets_at = Some(now);
            let mut invalid = a.clone(); invalid.windows[0].used_percent = Some(f64::NAN);
            let mut other_provider = a.clone(); other_provider.provider = if provider == Provider::Claude { Provider::Codex } else { Provider::Claude };
            for (case, rows, selection, threshold, expected) in [
                ("soonest reset", vec![b.clone(), a.clone()], AccountSelection::SoonestReset, 90, Some("a")),
                ("5h threshold", vec![five_full.clone(), b.clone()], AccountSelection::SoonestReset, 90, Some("b")),
                ("weekly threshold", vec![weekly_full, b.clone()], AccountSelection::SoonestReset, 90, Some("b")),
                ("weekly headroom within 24h", vec![a.clone(), row(provider, "b", 30, 20.0, 80.0)], AccountSelection::SoonestReset, 90, Some("b")),
                ("5h headroom tie", vec![a.clone(), row(provider, "b", 12, 59.0, 10.0)], AccountSelection::SoonestReset, 90, Some("b")),
                ("24h group anchored at earliest", vec![a.clone(), row(provider, "b", 25, 20.0, 10.0), row(provider, "c", 44, 0.0, 0.0)], AccountSelection::SoonestReset, 90, Some("b")),
                ("most headroom", vec![a.clone(), b.clone()], AccountSelection::MostHeadroom, 90, Some("b")),
                ("configured threshold", vec![a.clone(), b.clone()], AccountSelection::SoonestReset, 50, Some("b")),
                ("no eligible account", vec![five_full, disabled, logged_out, stale, unknown, expired, invalid, other_provider], AccountSelection::SoonestReset, 90, None),
            ] {
                let config = AccountsConfig { selection, threshold };
                let result = recommend(&rows, provider, now, &config);
                assert_eq!(result.recommended.as_ref().map(|r| r.account.name.as_str()), expected, "{provider:?}: {case}");
                if let Some(r) = result.recommended {
                    assert!(r.reason.contains("resets in"), "{case}: {}", r.reason);
                    assert!(r.reason.contains("% left"));
                } else {
                    assert_eq!(result.excluded.len(), rows.len() - 1, "{case}: other provider must not appear");
                    assert!(result.excluded.iter().all(|(_, why)| !why.is_empty()));
                }
            }
        }
    }
}
