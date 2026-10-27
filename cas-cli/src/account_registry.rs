//! Shared read model for Claude and Codex accounts. No credentials leave this
//! module or the limits adapter. Legacy folder aliases remain launch keys;
//! display names are presentation only. Consumers should require `LoggedIn`,
//! known usage, and their own flag policy before automatically choosing an account.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::accounts::{self, Provider};

/// One deadline for the entire account list, including login and usage probes.
pub const USAGE_DEADLINE: Duration = Duration::from_secs(4);

/// A failed probe is unknown, never evidence of rejection or missing credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginState {
    LoggedIn,
    /// Provider probe reports logged out; normalized to NeverLoggedIn or
    /// StaleLogin when credential evidence permits that distinction.
    LoggedOut,
    NeverLoggedIn,
    StaleLogin,
    Unknown,
}

/// Independently extensible findings; consumers must not infer login from flags.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct AccountFlags {
    /// Excluded from automatic selection; manual choices remain available.
    pub disabled: bool,
    /// Canonical account folder with the same provider identity, if any.
    pub duplicate_of: Option<PathBuf>,
    /// Neither a stored credential nor evidence of a previous login was found.
    pub never_logged_in: bool,
    /// Stored credentials were explicitly rejected (timeouts do not count).
    pub stale_login: bool,
}

/// Capacity data, without localized display strings. Missing values are unknown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageWindow {
    /// Provider window label, including `5h` and `weekly`.
    pub name: String,
    pub used_percent: Option<f64>,
    /// Absolute reset instant; serialized as RFC3339 in UTC.
    pub resets_at: Option<DateTime<Utc>>,
}

/// A provider subscription at a particular folder. Several folders may share
/// one identity, while one email may own several different subscriptions.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Account {
    /// Provider namespace; identities and duplicate flags never cross it.
    pub provider: Provider,
    /// Existing launch alias: `main`, `alt`, or the suffix of the folder name.
    pub name: String,
    /// Full account directory, also the stable key for mutations and usage.
    pub folder: PathBuf,
    /// Login email read from provider metadata; never inferred from a folder.
    pub email: Option<String>,
    /// Login email when known, otherwise the folder alias; never a launch key.
    pub display_name: String,
    /// Non-secret account ID plus organization/workspace. Email fallback is
    /// prefixed `email:` and used only when provider IDs are unavailable.
    pub identity: Option<String>,
    /// Subscription tier from credentials/claims or the usage API, if known.
    pub plan: Option<String>,
    /// Current probe result, with explicit rejection and missing-login states.
    pub login_state: LoginState,
    /// Empty means usage unknown, not zero usage.
    pub windows: Vec<UsageWindow>,
    /// Independent findings; selection policies decide which to exclude.
    pub flags: AccountFlags,
}

fn json_file(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn codex_claims(auth: &serde_json::Value) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let token = auth.pointer("/tokens/id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn string_at(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn metadata(provider: Provider, home: &Path, folder: &Path) -> (Option<String>, Option<String>) {
    match provider {
        Provider::Claude => {
            let value = if folder == home.join(".claude") {
                json_file(&home.join(".claude.json"))
                    .or_else(|| json_file(&folder.join(".claude.json")))
            } else {
                json_file(&folder.join(".claude.json"))
            };
            let identity = value.as_ref().and_then(|value| {
                let account = string_at(value, "/oauthAccount/accountUuid")?;
                let org = string_at(value, "/oauthAccount/organizationUuid").unwrap_or_default();
                Some(serde_json::json!([account, org]).to_string())
            });
            let plan = json_file(&folder.join(provider.credentials()))
                .and_then(|value| string_at(&value, "/claudeAiOauth/subscriptionType"));
            (identity, plan)
        }
        Provider::Codex => {
            let auth = json_file(&folder.join(provider.credentials())).unwrap_or_default();
            let claims = codex_claims(&auth).unwrap_or_default();
            let account = string_at(&auth, "/tokens/account_id")
                .or_else(|| string_at(&auth, "/account_id"))
                .or_else(|| {
                    string_at(
                        &claims,
                        "/https:~1~1api.openai.com~1auth/chatgpt_account_id",
                    )
                });
            let identity = account.map(|account| {
                let workspace = string_at(&auth, "/tokens/workspace_id")
                    .or_else(|| string_at(&auth, "/workspace_id"))
                    .unwrap_or_default();
                serde_json::json!([account, workspace]).to_string()
            });
            let plan = string_at(&claims, "/https:~1~1api.openai.com~1auth/chatgpt_plan_type");
            (identity, plan)
        }
    }
}

fn account(provider: Provider, home: &Path, folder: PathBuf) -> Account {
    let label = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let name = label
        .strip_prefix(provider.prefix())
        .unwrap_or("main")
        .to_string();
    let email = provider.login_email(home, &folder);
    let (identity, plan) = metadata(provider, home, &folder);
    let identity = identity.or_else(|| email.as_ref().map(|email| format!("email:{email}")));
    let never =
        !folder.join(provider.credentials()).exists() && email.is_none() && identity.is_none();
    let disabled = crate::account_lifecycle::disabled(home, &folder);
    Account {
        provider,
        display_name: email.clone().unwrap_or_else(|| name.clone()),
        name,
        folder,
        email,
        identity,
        plan,
        windows: Vec::new(),
        login_state: if never {
            LoginState::NeverLoggedIn
        } else {
            LoginState::Unknown
        },
        flags: AccountFlags {
            never_logged_in: never,
            disabled,
            ..Default::default()
        },
    }
}

fn set_login(account: &mut Account, state: LoginState) {
    let stored = account.folder.join(account.provider.credentials()).exists();
    account.login_state = match state {
        LoginState::LoggedOut if stored => LoginState::StaleLogin,
        LoginState::LoggedOut if account.flags.never_logged_in => LoginState::NeverLoggedIn,
        state => state,
    };
    // A successful CLI probe also covers credentials held in macOS Keychain.
    if account.login_state == LoginState::LoggedIn {
        account.flags.never_logged_in = false;
    }
    account.flags.stale_login = account.login_state == LoginState::StaleLogin;
}

fn mark_duplicates(accounts: &mut [Account]) {
    // Prefer a folder named after the email over a legacy alias, then use
    // folder order. The canonical choice is stable across probe completion order.
    let mut order: Vec<usize> = (0..accounts.len()).collect();
    order.sort_by_key(|&i| {
        let a = &accounts[i];
        (
            a.email.as_deref() != Some(a.name.as_str()),
            a.folder.clone(),
        )
    });
    let mut seen: std::collections::HashMap<(&str, String), PathBuf> =
        std::collections::HashMap::new();
    for i in order {
        let a = &mut accounts[i];
        a.flags.duplicate_of = None;
        if let Some(identity) = &a.identity {
            let key = (a.provider.name(), identity.clone());
            if let Some(folder) = seen.get(&key) {
                a.flags.duplicate_of = Some(folder.clone());
            } else {
                seen.insert(key, a.folder.clone());
            }
        }
    }
}

/// Discover one provider with an injectable login probe. Preserves non-account
/// suffix filtering through `accounts::discover`; performs no usage requests.
/// Pickers include a virtual `main` row for first login via `include_main`.
pub fn discover_with(
    provider: Provider,
    home: &Path,
    include_main: bool,
    mut probe: impl FnMut(&str, &Path) -> LoginState,
) -> std::io::Result<Vec<Account>> {
    let mut accounts: Vec<_> = accounts::discover(provider, home)?
        .into_iter()
        .map(|dir| account(provider, home, dir.path))
        .collect();
    let main = home.join(format!(".{}", provider.name()));
    if include_main && !accounts.iter().any(|a| a.folder == main) {
        accounts.insert(0, account(provider, home, main));
    }
    for a in &mut accounts {
        set_login(a, probe(&a.name, &a.folder));
    }
    mark_duplicates(&mut accounts);
    Ok(accounts)
}

/// List every real Claude and Codex folder. All provider work runs concurrently
/// behind a single four-second deadline; slow or unavailable accounts remain
/// visible with unknown data. No environment or account directory is modified.
pub fn collect(home: &Path) -> std::io::Result<Vec<Account>> {
    let mut accounts = Vec::new();
    for provider in Provider::ALL {
        accounts.extend(discover_with(provider, home, false, |_, _| {
            LoginState::Unknown
        })?);
    }
    refresh(&mut accounts);
    Ok(accounts)
}

/// Collect only the requested provider for a launch decision, with one deadline.
pub fn collect_for(provider: Provider, home: &Path, include_main: bool) -> std::io::Result<Vec<Account>> {
    let mut accounts = discover_with(provider, home, include_main, |_, _| LoginState::Unknown)?;
    refresh(&mut accounts);
    Ok(accounts)
}

fn refresh(accounts: &mut [Account]) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let until = Instant::now() + USAGE_DEADLINE;
    for (i, a) in accounts.iter().enumerate() {
        let sender = sender.clone();
        let a = a.clone();
        let usage_sender = sender.clone();
        let usage_account = a.clone();
        std::thread::spawn(move || {
            let usage =
                crate::cli::limits::account_usage(usage_account.provider, &usage_account.folder);
            let _ = usage_sender.send((i, None, Some(usage)));
        });
        std::thread::spawn(move || {
            let state = match a.provider {
                Provider::Claude => crate::cli::claude::probe_login_state(&a.name, &a.folder),
                Provider::Codex => crate::cli::codex::probe_login_state(&a.name, &a.folder),
            };
            let _ = sender.send((i, Some(state), None));
        });
    }
    drop(sender);
    receive_updates(accounts, receiver, until);
    mark_duplicates(accounts);
}

type ProbeUpdate = (
    usize,
    Option<LoginState>,
    Option<crate::cli::limits::AccountUsage>,
);

fn receive_updates(
    accounts: &mut [Account],
    receiver: std::sync::mpsc::Receiver<ProbeUpdate>,
    until: Instant,
) {
    let mut states = vec![None; accounts.len()];
    let mut usage = (0..accounts.len()).map(|_| None).collect::<Vec<_>>();
    // Completed messages remain useful even at the deadline. Waiting for an
    // unfinished probe never extends it; the channel has at most 2N updates.
    while let Ok((i, state, snapshot)) =
        receiver.recv_timeout(until.saturating_duration_since(Instant::now()))
    {
        if state.is_some() {
            states[i] = state;
        }
        if snapshot.is_some() {
            usage[i] = snapshot;
        }
    }
    for (i, a) in accounts.iter_mut().enumerate() {
        if let Some(state) = states[i] {
            set_login(a, state);
        }
        if let Some(snapshot) = usage[i].take() {
            a.plan = snapshot.plan.or_else(|| a.plan.take());
            // Live provider acceptance/rejection beats a local or unknown
            // login probe, independent of thread completion order.
            if snapshot.rejected {
                set_login(a, LoginState::StaleLogin);
            } else if snapshot.authenticated {
                set_login(a, LoginState::LoggedIn);
            }
            a.windows = snapshot.windows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_keeps_completed_usage_and_leaves_pending_accounts_unknown() {
        let home = tempfile::tempdir().unwrap();
        let mut accounts = vec![
            account(Provider::Codex, home.path(), home.path().join(".codex-a")),
            account(Provider::Claude, home.path(), home.path().join(".claude-b")),
        ];
        let (sender, receiver) = std::sync::mpsc::channel();
        let reset = "2026-10-28T13:00:01Z".parse().unwrap();
        sender
            .send((
                0,
                Some(LoginState::Unknown),
                Some(crate::cli::limits::AccountUsage {
                    plan: Some("pro".into()),
                    rejected: false,
                    authenticated: false,
                    windows: vec![UsageWindow {
                        name: "5h".into(),
                        used_percent: Some(42.0),
                        resets_at: Some(reset),
                    }],
                }),
            ))
            .unwrap();
        // Keep the sender alive: another provider has not answered. No sleeps
        // or live processes are needed to prove the deadline's boundary.
        receive_updates(&mut accounts, receiver, Instant::now());
        assert_eq!(accounts[0].windows[0].resets_at, Some(reset));
        assert_eq!(accounts[0].windows[0].used_percent, Some(42.0));
        assert!(accounts[1].windows.is_empty());
        assert!(!accounts[1].flags.stale_login);
        assert_eq!(USAGE_DEADLINE.as_secs(), 4);
        drop(sender);
    }

    #[test]
    fn fixture_identities_and_login_evidence_are_provider_scoped() {
        use base64::Engine as _;
        let home = tempfile::tempdir().unwrap();
        for provider in Provider::ALL {
            // Same email + same IDs duplicate; same email + different account
            // or workspace is a separate subscription. Email fallback is
            // exercised with and without a different email.
            for (name, id, workspace, email, duplicate) in [
                ("a", Some("one"), "org", "same@example.com", false),
                ("b", Some("one"), "org", "same@example.com", true),
                ("c", Some("two"), "org", "same@example.com", false),
                ("d", Some("one"), "other", "same@example.com", false),
                ("e", None, "", "fallback@example.com", false),
                ("f", None, "", "fallback@example.com", true),
                ("g", None, "", "different@example.com", false),
            ] {
                let dir = home.path().join(format!("{}{name}", provider.prefix()));
                std::fs::create_dir(&dir).unwrap();
                match provider {
                    Provider::Claude => {
                        std::fs::write(dir.join(".claude.json"), serde_json::json!({"oauthAccount": {
                            "emailAddress": email, "accountUuid": id, "organizationUuid": workspace,
                        }}).to_string()).unwrap();
                        std::fs::write(dir.join(provider.credentials()), "{}").unwrap();
                    }
                    Provider::Codex => {
                        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(serde_json::json!({"email": email}).to_string());
                        std::fs::write(dir.join(provider.credentials()), serde_json::json!({"tokens": {
                            "id_token": format!("h.{claims}.s"), "account_id": id, "workspace_id": workspace,
                        }}).to_string()).unwrap();
                    }
                }
                let rows = discover_with(provider, home.path(), false, |_, _| LoginState::LoggedIn)
                    .unwrap();
                let row = rows.iter().find(|a| a.name == name).unwrap();
                assert_eq!(
                    row.flags.duplicate_of.is_some(),
                    duplicate,
                    "{provider:?}/{name}"
                );
                assert_eq!(row.display_name, email);
            }
            for name in [
                "empty",
                "stale",
                "unknown",
                "scratch.lock",
                "scratch.tmp.123",
            ] {
                let dir = home.path().join(format!("{}{name}", provider.prefix()));
                std::fs::create_dir(&dir).unwrap();
                if matches!(name, "stale" | "unknown") {
                    std::fs::write(dir.join(provider.credentials()), "{}").unwrap();
                }
            }
            let rows = discover_with(provider, home.path(), false, |name, _| match name {
                "empty" | "stale" => LoginState::LoggedOut,
                "unknown" => LoginState::Unknown,
                _ => LoginState::LoggedIn,
            })
            .unwrap();
            assert_eq!(rows.len(), 10);
            let row = |name| rows.iter().find(|a| a.name == name).unwrap();
            assert_eq!(row("empty").login_state, LoginState::NeverLoggedIn);
            assert!(row("empty").flags.never_logged_in);
            assert_eq!(row("stale").login_state, LoginState::StaleLogin);
            assert!(row("stale").flags.stale_login);
            assert_eq!(row("unknown").login_state, LoginState::Unknown);
            assert!(!row("unknown").flags.stale_login);
        }
    }
}
