//! Machine-local provider capacity snapshots for worker routing.
//!
//! Capacity comes only from official harness output. Claude statusline snapshots
//! and Codex rollout records contain usage metadata, never login credentials.

use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::cli::Cli;
use std::fmt;

// An observation without a known reset cannot establish capacity indefinitely.
// Known future windows remain useful after an idle session, with their age shown.
const CLAUDE_SNAPSHOT_MAX_AGE: i64 = 5 * 60;
const CLAUDE_SNAPSHOT_FILE: &str = ".cas-claude-usage.json";

#[derive(Args, Debug, Clone, Default)]
pub struct LimitsArgs {
    /// Print a compact single-line summary suitable for a factory strip
    #[arg(long)]
    compact: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct LimitsReport {
    accounts: Vec<AccountLimit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct AccountLimit {
    account: String,
    provider: String,
    plan: Option<String>,
    state: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<DateTime<Utc>>,
    windows: Vec<LimitWindow>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct LimitWindow {
    name: String,
    used_percent: Option<f64>,
    reset_at: Option<String>,
    spend_dollars: Option<f64>,
}

/// Only documented, non-secret fields from Claude Code's statusline stdin.
/// Unknown fields are discarded during deserialization and never cached.
#[derive(Debug, Deserialize)]
pub(super) struct ClaudeRateLimits {
    five_hour: Option<ClaudeRateWindow>,
    seven_day: Option<ClaudeRateWindow>,
}

#[derive(Debug, Deserialize)]
struct ClaudeRateWindow {
    used_percentage: Option<f64>,
    resets_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct ClaudeSnapshot {
    identity: Option<String>,
    observed_at: DateTime<Utc>,
    windows: Vec<LimitWindow>,
}

fn claude_snapshot_identity(config_dir: &Path) -> Option<String> {
    // Registry's Claude branch reads only .claude.json account metadata, never
    // .credentials.json. Re-login must not reuse another subscription's cache.
    let home = config_dir.parent()?;
    let account = crate::account_registry::account(crate::accounts::Provider::Claude, home, config_dir.to_owned());
    account.identity.and_then(|identity| serde_json::to_string(&identity).ok())
}

/// Cache only the native binary's documented capacity fields, scoped to the
/// exact profile that emitted them. This is not an authentication signal.
pub(super) fn record_claude_statusline(config_dir: &Path, limits: ClaudeRateLimits) {
    let snapshot = ClaudeSnapshot {
        identity: claude_snapshot_identity(config_dir),
        observed_at: Utc::now(),
        windows: [("5h", limits.five_hour), ("weekly", limits.seven_day)]
            .into_iter().filter_map(|(name, window)| {
                let window = window?;
                let used_percent = window.used_percentage
                    .filter(|used| used.is_finite() && (0.0..=100.0).contains(used));
                let reset_at = window.resets_at.and_then(local_unix);
                (used_percent.is_some() || reset_at.is_some()).then(|| LimitWindow {
                    name: name.into(), used_percent, reset_at, spend_dollars: None,
                })
            }).collect(),
    };
    // Atomic replacement avoids partial JSON when statuslines overlap.
    let write = || -> anyhow::Result<()> {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(config_dir)?;
        serde_json::to_writer(&mut file, &snapshot)?;
        file.flush()?;
        file.persist(config_dir.join(CLAUDE_SNAPSHOT_FILE))?;
        Ok(())
    };
    let _ = write(); // Status display must still work on read-only profiles.
}

pub fn execute(args: &LimitsArgs, cli: &Cli) -> anyhow::Result<()> {
    let report = collect_limits();
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.compact {
        println!("{}", compact_line(&report));
    } else {
        print_human(&report);
    }
    Ok(())
}

fn collect_limits() -> LimitsReport {
    let home = dirs::home_dir().unwrap_or_default();
    let mut accounts = Vec::new();
    // cas-e1f4: every account directory, named ones included, not just the
    // default and alt.
    for (provider, label, dir) in account_dirs(&home) {
        accounts.push(match provider {
            crate::accounts::Provider::Claude => collect_claude(&label, &dir),
            crate::accounts::Provider::Codex => collect_codex(&label, &dir),
        });
    }
    accounts.push(collect_exa());
    LimitsReport { accounts }
}

fn collect_claude(account: &str, config_dir: &Path) -> AccountLimit {
    collect_claude_snapshot(account, config_dir).0
}

fn collect_claude_snapshot(account: &str, config_dir: &Path) -> (AccountLimit, Option<DateTime<Utc>>) {
    let snapshot = fs::read(config_dir.join(CLAUDE_SNAPSHOT_FILE)).ok()
        .and_then(|bytes| serde_json::from_slice::<ClaudeSnapshot>(&bytes).ok());
    let Some(mut snapshot) = snapshot else {
        return (unavailable(account, "claude", "unavailable",
            "usage unknown; no Claude Code statusline snapshot"), None);
    };
    if snapshot.identity != claude_snapshot_identity(config_dir) {
        return (unavailable(account, "claude", "unavailable",
            "usage unknown; account changed since Claude Code statusline snapshot"), None);
    }
    let age = Utc::now().signed_duration_since(snapshot.observed_at).num_seconds();
    if age < 0 || snapshot.windows.is_empty() {
        return (unavailable(account, "claude", "unavailable",
            "usage unknown; Claude Code statusline snapshot empty or from the future"), Some(snapshot.observed_at));
    }
    for window in &mut snapshot.windows {
        let reset = window.reset_at.as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok());
        if expired(window.reset_at.as_deref())
            || (age > CLAUDE_SNAPSHOT_MAX_AGE && !reset.is_some_and(|at| at > Utc::now())) {
            window.used_percent = None;
        }
    }
    (AccountLimit {
        account: account.into(), provider: "claude".into(), plan: None,
        state: "active".into(), source: "Claude Code statusline".into(), observed_at: Some(snapshot.observed_at),
        windows: snapshot.windows, detail: None,
    }, Some(snapshot.observed_at))
}

fn collect_codex(account: &str, root: &Path) -> AccountLimit {
    collect_codex_snapshot(account, root).0
}

fn collect_codex_snapshot(account: &str, root: &Path) -> (AccountLimit, Option<DateTime<Utc>>) {
    let Some((plan, mut windows, observed_at)) = latest_codex_limits(root) else {
        return (
            unavailable(account, "codex", "absent", "no rollout rate_limits found"),
            None,
        );
    };
    for window in &mut windows {
        if expired(window.reset_at.as_deref()) {
            window.used_percent = None;
        }
    }
    (
        AccountLimit {
            account: account.to_string(),
            provider: "codex".to_string(),
            plan,
            state: "active".to_string(),
            source: "live transcript".to_string(),
            observed_at: Some(observed_at),
            windows,
            detail: None,
        },
        Some(observed_at),
    )
}

pub(crate) fn expired(reset: Option<&str>) -> bool {
    reset
        .and_then(|reset| DateTime::parse_from_rfc3339(reset).ok())
        .is_some_and(|reset| reset <= Utc::now())
}

/// Every Claude and Codex account directory with its report label:
/// `claude-main`, `claude-alt` and `codex` for the defaults, else
/// `<provider>-<name>`. Canonical targets are collapsed to avoid duplicate reports.
fn account_dirs(home: &Path) -> Vec<(crate::accounts::Provider, String, std::path::PathBuf)> {
    let mut found = Vec::new();
    for provider in crate::accounts::Provider::ALL {
        for account in crate::accounts::discover(provider, home).unwrap_or_default() {
            let label = match account.label.as_str() {
                ".claude" => "claude-main".to_string(),
                ".claude-alt" => "claude-alt".to_string(),
                ".codex" => "codex".to_string(),
                other => format!(
                    "{}-{}",
                    provider.name(),
                    other.trim_start_matches(provider.prefix())
                ),
            };
            found.push((provider, label, account.path));
        }
    }
    found
}

/// Structured account usage for the shared registry. The collector remains
/// provider-owned and consumes only native harness usage metadata.
#[derive(Clone)]
pub(crate) struct AccountUsage {
    pub observed_at: Option<DateTime<Utc>>,
    pub plan: Option<String>,
    pub windows: Vec<crate::account_registry::UsageWindow>,
    pub rejected: bool,
    pub authenticated: bool,
}

type UsageCache = std::sync::Mutex<std::collections::HashMap<(&'static str, std::path::PathBuf), AccountUsage>>;
fn usage_cache() -> &'static UsageCache {
    static CACHE: std::sync::OnceLock<UsageCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

pub(crate) fn cached_account_usage(provider: crate::accounts::Provider, dir: &Path) -> Option<AccountUsage> {
    if provider == crate::accounts::Provider::Claude {
        // Status reads the small native snapshot too, so an old in-process
        // entry cannot bypass its freshness or account-identity checks.
        return Some(account_usage(provider, dir));
    }
    usage_cache().lock().ok()?.get(&(provider.name(), dir.canonicalize().unwrap_or_else(|_| dir.to_owned()))).cloned()
}

/// Structured transcript capacity, when a harness actually published it.
pub(crate) fn transcript_windows(record: &Value) -> Vec<crate::account_registry::UsageWindow> {
    find_rate_limits(record).map(|limits| report_windows(&AccountLimit {
        account: String::new(), provider: String::new(), plan: None,
        state: String::new(), source: String::new(), observed_at: None, windows: parse_codex_limits(limits), detail: None,
    })).unwrap_or_default()
}

pub(crate) fn account_usage(provider: crate::accounts::Provider, dir: &Path) -> AccountUsage {
    let (limit, observed_at) = match provider {
        crate::accounts::Provider::Claude => collect_claude_snapshot("account", dir),
        crate::accounts::Provider::Codex => collect_codex_snapshot("account", dir),
    };
    let usage = AccountUsage {
        observed_at,
        plan: limit.plan,
        rejected: limit.state == "auth-needed",
        authenticated: false, // Cached usage cannot establish a current login.
        windows: limit
            .windows
            .into_iter()
            .map(|window| crate::account_registry::UsageWindow {
                name: window.name,
                used_percent: window.used_percent,
                resets_at: window.reset_at.as_deref().and_then(|value| {
                    DateTime::parse_from_rfc3339(value)
                        .ok()
                        .map(|time| time.with_timezone(&Utc))
                }),
            })
            .collect(),
    };
    if let Ok(mut cache) = usage_cache().lock() {
        cache.insert((provider.name(), dir.canonicalize().unwrap_or_else(|_| dir.to_owned())), usage.clone());
    }
    usage
}

/// Keep observation age next to capacity rather than implying a live probe.
pub(crate) fn observation_label(observed_at: Option<DateTime<Utc>>) -> String {
    observation_label_at(observed_at, Utc::now())
}

pub(crate) fn observation_label_at(observed_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    observed_at.map(|at| {
        let at = super::release_report::eastern(at);
        let stamp = if at.date_naive() == super::release_report::eastern(now).date_naive() {
            at.format("%-I:%M %p ET").to_string()
        } else {
            at.format("%-d %B %Y · %-I:%M %p ET").to_string()
        };
        format!(" as of {stamp}")
    }).unwrap_or_default()
}

pub(crate) fn summarize_account_at(account: &crate::account_registry::Account, now: DateTime<Utc>) -> Option<String> {
    let observed = account.usage_observed_at.filter(|_| account.provider == crate::accounts::Provider::Claude);
    summarize_windows_at(&account.windows, now)
        .map(|summary| match observed {
            Some(_) => format!("{summary};{}", observation_label_at(observed, now)),
            None => summary,
        })
}

/// Human reset text; the stored/JSON instant stays UTC RFC3339.
pub(crate) fn format_reset(reset: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    format_reset_in(reset, now.with_timezone(&Local))
}

fn format_reset_in<Tz: TimeZone>(reset: Option<DateTime<Utc>>, now: DateTime<Tz>) -> String
where Tz::Offset: fmt::Display {
    let Some(reset) = reset else { return "reset time unknown".into() };
    let local = reset.with_timezone(&now.timezone());
    let seconds = reset.signed_duration_since(now.with_timezone(&Utc)).num_seconds();
    let minutes = (seconds + 59) / 60;
    if local <= now { return "reset, usage unknown until next session".into() }
    let tomorrow = now.date_naive().succ_opt();
    let stamp = if local.date_naive() == now.date_naive() || Some(local.date_naive()) == tomorrow {
        local.format("%a %H:%M").to_string()
    } else { local.format("%a %b %-d %H:%M").to_string() };
    let relative = if minutes < 60 { format!("{}m", minutes.max(1)) }
        else if seconds < 24 * 3600 { format!("{}h", (minutes + 59) / 60) }
        else {
            let days = seconds / (24 * 3600);
            let hours = seconds / 3600 % 24;
            if hours == 0 { format!("{days}d") } else { format!("{days}d {hours}h") }
        };
    format!("resets {stamp} (in {relative})")
}

pub(crate) fn window_summary(window: &crate::account_registry::UsageWindow, now: DateTime<Utc>) -> String {
    let reset = format_reset(window.resets_at, now);
    if window.resets_at.is_some_and(|at| at <= now) { return format!("{} {reset}", window.name) }
    let used = match window.used_percent {
        Some(used) if used >= 100. => "limit hit".into(),
        Some(used) => format!("{used:.0}%"),
        None => "usage unknown".into(),
    };
    format!("{} {used}, {reset}", window.name)
}

pub(crate) fn summarize_windows_at(windows: &[crate::account_registry::UsageWindow], now: DateTime<Utc>) -> Option<String> {
    let summaries = windows.iter().filter(|window| window.name != "extra usage")
        .map(|window| window_summary(window, now)).collect::<Vec<_>>();
    (!summaries.is_empty()).then(|| summaries.join("; "))
}

pub(crate) fn summarize_windows(windows: &[crate::account_registry::UsageWindow]) -> Option<String> {
    summarize_windows_at(windows, Utc::now())
}

fn report_windows(limit: &AccountLimit) -> Vec<crate::account_registry::UsageWindow> {
    limit.windows.iter().filter(|window| window.name != "extra usage").map(|window| crate::account_registry::UsageWindow {
        name: window.name.clone(), used_percent: window.used_percent,
        resets_at: window.reset_at.as_deref().and_then(|at| DateTime::parse_from_rfc3339(at).ok()).map(|at| at.with_timezone(&Utc)),
    }).collect()
}

fn summarize(limit: &AccountLimit) -> Option<String> {
    summarize_windows(&report_windows(limit))
}

/// Shared registry capacity for several accounts at once. Duplicate Codex
/// folders use their newest snapshot; unfinished work is left out at deadline.
pub(crate) fn usage_summaries(
    provider: crate::accounts::Provider,
    accounts: Vec<(String, std::path::PathBuf)>,
    deadline: std::time::Duration,
) -> std::collections::HashMap<String, String> {
    crate::account_registry::picker_usage_summaries(provider, accounts, deadline)
}

fn collect_exa() -> AccountLimit {
    if std::env::var_os("EXA_API_KEY").is_none() {
        return unavailable("exa", "exa", "auth-needed", "EXA_API_KEY is not configured");
    }
    let windows = dirs::home_dir()
        .as_deref()
        .and_then(load_exa_ledger_spend)
        .map(|spend_dollars| LimitWindow {
            name: "local spend".to_string(),
            used_percent: None,
            reset_at: None,
            spend_dollars: Some(spend_dollars),
        })
        .into_iter()
        .collect();
    AccountLimit {
        account: "exa".to_string(),
        provider: "exa".to_string(),
        plan: None,
        state: "unavailable".to_string(),
        source: "ESTIMATE ledger".to_string(),
        observed_at: None,
        windows,
        detail: Some(
            "API key configured; no supported balance endpoint (cost is a local estimate)"
                .to_string(),
        ),
    }
}

/// Sum response costs written by callers to the optional machine-local ledger.
/// The ledger is best-effort: a missing file is not a zero balance.
fn load_exa_ledger_spend(home: &Path) -> Option<f64> {
    let content = fs::read_to_string(home.join(".cas/exa-cost-ledger.jsonl")).ok()?;
    let mut found = false;
    let total = content
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<Value>(line).ok().and_then(|value| {
                let cost = parse_exa_cost(&value)?;
                found = true;
                Some(cost)
            })
        })
        .sum::<f64>();
    found.then_some(total)
}

/// Exa v2 returns `costDollars.total`; keep scalar compatibility for old captures.
fn parse_exa_cost(value: &Value) -> Option<f64> {
    value
        .pointer("/costDollars/total")
        .and_then(Value::as_f64)
        .or_else(|| value.get("costDollars").and_then(Value::as_f64))
        .or_else(|| value.get("cost_dollars").and_then(Value::as_f64))
}

fn unavailable(account: &str, provider: &str, state: &str, detail: &str) -> AccountLimit {
    unavailable_with_plan(account, provider, None, state, detail)
}

fn unavailable_with_plan(
    account: &str,
    provider: &str,
    plan: Option<String>,
    state: &str,
    detail: &str,
) -> AccountLimit {
    AccountLimit {
        account: account.to_string(),
        provider: provider.to_string(),
        plan,
        state: state.to_string(),
        source: "none".to_string(),
        observed_at: None,
        windows: Vec::new(),
        detail: Some(detail.to_string()),
    }
}

fn latest_codex_limits(root: &Path) -> Option<(Option<String>, Vec<LimitWindow>, DateTime<Utc>)> {
    let sessions = root.join("sessions");
    let (modified, newest) = WalkDir::new(sessions)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        })
        .filter_map(|entry| {
            entry.metadata().ok().and_then(|meta| {
                meta.modified()
                    .ok()
                    .map(|modified| (modified, entry.into_path()))
            })
        })
        .max_by_key(|(modified, _)| *modified)?;
    let contents = fs::read_to_string(newest).ok()?;
    for line in contents.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(limits) = find_rate_limits(&value) else {
            continue;
        };
        let windows = parse_codex_limits(limits);
        if !windows.is_empty() {
            return Some((
                limits
                    .get("plan_type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                windows,
                value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
                    .map(|time| time.with_timezone(&Utc))
                    .unwrap_or_else(|| modified.into()),
            ));
        }
    }
    None
}

fn find_rate_limits(value: &Value) -> Option<&Value> {
    value
        .get("rate_limits")
        .or_else(|| value.pointer("/payload/rate_limits"))
}

fn parse_codex_limits(limits: &Value) -> Vec<LimitWindow> {
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| {
            let window = limits.get(key)?;
            let minutes = window.get("window_minutes")?.as_i64()?;
            Some(LimitWindow {
                name: if minutes == 300 {
                    "5h".to_string()
                } else if minutes == 10080 {
                    "weekly".to_string()
                } else {
                    format!("{minutes}m")
                },
                used_percent: window.get("used_percent").and_then(Value::as_f64),
                reset_at: window
                    .get("resets_at")
                    .and_then(Value::as_i64)
                    .and_then(local_unix),
                spend_dollars: None,
            })
        })
        .collect()
}

fn local_unix(timestamp: i64) -> Option<String> {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|time| time.with_timezone(&Utc).to_rfc3339())
}

fn compact_line(report: &LimitsReport) -> String {
    let now = Utc::now();
    let mut entries = report.accounts.iter().map(|account| {
        let windows = report_windows(account);
        let constraining = windows.iter().filter(|w| w.resets_at.is_none_or(|at| at > now))
            .max_by(|a,b| a.used_percent.unwrap_or(-1.).total_cmp(&b.used_percent.unwrap_or(-1.)));
        let alias = display_alias(&account.account, 24);
        let (text, reset) = if let Some(window) = constraining {
            let used = window.used_percent.map(|used| format!("{used:.0}%")).unwrap_or_else(|| "unknown".into());
            let reset = format_reset(window.resets_at, now);
            let observation = observation_label(account.observed_at.filter(|_| account.provider == "claude"));
            (format!("{alias} {used}{observation} {}", reset.split(" (in ").next().unwrap_or(&reset)), window.resets_at)
        } else if !windows.is_empty() { (format!("{alias} reset/unknown"), None) }
        else { (format!("{alias}:{}", account.state), None) };
        (reset, text)
    }).collect::<Vec<_>>();
    entries.sort_by(|a,b| a.0.is_none().cmp(&b.0.is_none()).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut out = String::new();
    for (index, (_, entry)) in entries.iter().enumerate() {
        let separator = if out.is_empty() { "" } else { " | " };
        let remaining = entries.len() - index - 1;
        let reserve = if remaining > 0 { format!(" | +{remaining} more").len() } else { 0 };
        if ratatui::text::Line::from(format!("{out}{separator}{entry}")).width() + reserve > 80 {
            let suffix = format!("+{} more", entries.len() - index);
            if !out.is_empty() { out.push_str(" | "); }
            out.push_str(&suffix);
            break;
        }
        out.push_str(separator); out.push_str(entry);
    }
    out
}

fn display_alias(value: &str, max: usize) -> String {
    if ratatui::text::Line::from(value).width() <= max { return value.to_owned(); }
    let mut short = String::new();
    for c in value.chars() {
        if ratatui::text::Line::from(format!("{short}{c}...")).width() > max { break; }
        short.push(c);
    }
    short.push_str("..."); short
}

fn render_human(report: &LimitsReport) -> String {
    let now = Utc::now();
    let mut out = String::from("Cassy provider limits\n");
    for account in &report.accounts {
        out.push('\n');
        let header = format!("{}{} - {} [{}]", display_alias(&account.account, 48), account.plan.as_ref().map(|plan| format!(" ({plan})")).unwrap_or_default(), account.state, account.source);
        let observation = observation_label(account.observed_at.filter(|_| account.provider == "claude"));
        out.push_str(&crate::cloud::presence::wrap_line(&format!("{header}{observation}")));
        for window in &account.windows {
            let typed = crate::account_registry::UsageWindow {
                name: window.name.clone(), used_percent: window.used_percent,
                resets_at: window.reset_at.as_deref().and_then(|at| DateTime::parse_from_rfc3339(at).ok()).map(|at| at.with_timezone(&Utc)),
            };
            let text = if window.name == "extra usage" {
                window.spend_dollars.map(|spend| format!("extra usage: ${spend:.2} spent"))
                    .unwrap_or_else(|| "extra usage: spend unknown".into())
            } else if account.provider == "exa" {
                window.spend_dollars.map(|spend| format!("{}: ${spend:.2} spent", window.name)).unwrap_or_else(|| window_summary(&typed, now))
            } else {
                let mut text = window_summary(&typed, now);
                if let Some(spend) = window.spend_dollars { text.push_str(&format!(", ${spend:.2} spent")); }
                text
            };
            out.push_str("  "); out.push_str(&text); out.push('\n');
        }
        if let Some(detail) = &account.detail { out.push_str(&crate::cloud::presence::wrap_line(detail)); }
    }
    if report.accounts.iter().any(|a| display_alias(&a.account, 48) != a.account) {
        out.push_str("Full account names: cas limits --json\n");
    }
    out
}

fn print_human(report: &LimitsReport) { print!("{}", render_human(report)); }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_usage_without_native_snapshot_is_unknown_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".credentials.json"), "{}").unwrap();
        let report = collect_claude("fixture", dir.path());
        assert_eq!(report.state, "unavailable");
        assert!(report.detail.as_deref().unwrap().contains("Claude Code statusline"));
        let usage = account_usage(crate::accounts::Provider::Claude, dir.path());
        assert!(!usage.rejected && !usage.authenticated);
    }

    #[test]
    fn limits_rows_format_resets_in_eastern_twelve_hour_time() {
        let now = "2026-10-27T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        for (reset, expected) in [
            (Some("2026-10-27T18:00:00Z"), "5h 34%, resets Tue 2:00 PM ET (in 6h)"),
            (Some("2026-10-28T13:00:00Z"), "5h 34%, resets Wed 9:00 AM ET (in 1d 1h)"),
            (Some("2026-10-31T13:00:00Z"), "5h 34%, resets Sat Oct 31 9:00 AM ET (in 4d 1h)"),
            (Some("2026-10-28T11:59:00Z"), "5h 34%, resets Wed 7:59 AM ET (in 24h)"),
            (Some("2026-10-28T12:00:00Z"), "5h 34%, resets Wed 8:00 AM ET (in 1d)"),
            (Some("2026-10-29T11:56:00Z"), "5h 34%, resets Thu Oct 29 7:56 AM ET (in 1d 23h)"),
            (Some("2026-11-03T11:00:00Z"), "5h 34%, resets Tue Nov 3 6:00 AM ET (in 6d 23h)"),
            (Some("2026-11-01T18:00:00Z"), "5h 34%, resets Sun Nov 1 1:00 PM ET (in 5d 6h)"),
            (Some("2026-10-27T11:00:00Z"), "5h reset, usage unknown until next session"),
            (None, "5h 34%, reset time unknown"),
        ] {
            let window = crate::account_registry::UsageWindow {
                name: "5h".into(),
                used_percent: Some(34.0),
                resets_at: reset.map(|value| value.parse().unwrap()),
            };
            assert_eq!(window_summary(&window, now), expected);
        }
    }

    #[test]
    fn parses_codex_primary_and_secondary_windows() {
        let value: Value = serde_json::json!({"primary":{"used_percent":26.0,"window_minutes":10080,"resets_at":1787011690},"secondary":{"used_percent":7.0,"window_minutes":300,"resets_at":1786651690},"plan_type":"pro"});
        let windows = parse_codex_limits(&value);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].name, "weekly");
        assert_eq!(windows[1].name, "5h");
        assert_eq!(windows[0].used_percent, Some(26.0));
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("sessions")).unwrap();
        std::fs::write(
            home.path().join("sessions/rollout.jsonl"),
            serde_json::json!({"payload": {"rate_limits": value}}).to_string(),
        )
        .unwrap();
        let usage = account_usage(crate::accounts::Provider::Codex, home.path());
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert_eq!(usage.windows[0].resets_at.unwrap().timestamp(), 1787011690);
        assert_eq!(usage.windows[1].used_percent, None);
        assert!(
            summarize_windows(&usage.windows)
                .unwrap()
                .contains("reset, usage unknown until next session")
        );
        assert!(
            !usage.authenticated,
            "a transcript does not prove a current login"
        );
    }

    #[test]
    fn finds_nested_codex_rate_limits() {
        let value: Value = serde_json::json!({"payload":{"rate_limits":{"primary":{"used_percent":1.0,"window_minutes":300,"resets_at":100}}}});
        assert_eq!(
            parse_codex_limits(find_rate_limits(&value).unwrap()).len(),
            1
        );
    }

    #[test]
    fn weekly_observation_date_follows_the_eastern_day() {
        use crate::accounts::Provider;
        let mut account = crate::account_selection::tests::row(Provider::Claude, "weekly", 1, 12.0, 34.0);
        for (observed, now, expected) in [
            ("2026-10-27T03:30:00Z", "2026-10-27T13:00:00Z", "as of 26 October 2026 · 11:30 PM ET"),
            ("2026-10-26T22:30:00Z", "2026-10-27T03:55:00Z", "as of 6:30 PM ET"),
        ] {
            account.usage_observed_at = Some(observed.parse().unwrap());
            let summary = summarize_account_at(&account, now.parse().unwrap()).unwrap();
            assert!(summary.contains(expected), "{observed} at {now}: {summary}");
        }
    }

    #[test]
    fn native_claude_snapshot_reaches_limits_and_picker_without_authentication() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".claude-fixture");
        fs::create_dir(&dir).unwrap();
        let reset = Utc::now() + chrono::Duration::hours(1);
        record_claude_statusline(&dir, serde_json::from_value(serde_json::json!({
            "five_hour": {"used_percentage": 12.0, "resets_at": reset.timestamp()},
            "seven_day": {"used_percentage": 34.0, "resets_at": reset.timestamp()},
            "accessToken": "must-not-be-cached"
        })).unwrap());
        let report = collect_claude("fixture", &dir);
        assert_eq!(report.windows[0].used_percent, Some(12.0));
        assert_eq!(report.windows[0].reset_at.as_deref(), Some(local_unix(reset.timestamp()).unwrap().as_str()));
        assert!(render_human(&LimitsReport { accounts: vec![report] }).contains("12%"));
        let picker = usage_summaries(crate::accounts::Provider::Claude,
            vec![("fixture".into(), dir.clone())], Duration::from_secs(1));
        assert!(picker["fixture"].contains("weekly 34%"));
        assert!(!account_usage(crate::accounts::Provider::Claude, &dir).authenticated);
        assert!(!fs::read_to_string(dir.join(CLAUDE_SNAPSHOT_FILE)).unwrap().contains("must-not-be-cached"));
        assert!(collect_claude("other", home.path()).windows.is_empty(), "profiles do not share usage");
        let mut snapshot: ClaudeSnapshot = serde_json::from_slice(&fs::read(dir.join(CLAUDE_SNAPSHOT_FILE)).unwrap()).unwrap();
        snapshot.observed_at = Utc::now() - chrono::Duration::hours(3);
        fs::write(dir.join(CLAUDE_SNAPSHOT_FILE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = collect_claude("fixture", &dir);
        assert_eq!(report.windows.get(1).and_then(|window| window.used_percent), Some(34.0), "unreset weekly observation survives idle sessions");
        assert_eq!(cached_account_usage(crate::accounts::Provider::Claude, &dir).unwrap().windows[1].used_percent, Some(34.0));
        let picker = usage_summaries(crate::accounts::Provider::Claude,
            vec![("fixture".into(), dir.clone())], Duration::from_secs(1));
        assert!(picker["fixture"].contains("as of ") && picker["fixture"].contains(" ET"), "{}", picker["fixture"]);
        let report = LimitsReport { accounts: vec![report] };
        let compact = compact_line(&report);
        assert!(compact.contains("34% as of ") && compact.contains(" ET"), "{compact}");
        let human = render_human(&report);
        assert!(human.contains("34%") && human.contains("as of ") && human.contains(" ET"), "{human}");
        snapshot.observed_at = Utc::now();
        for window in &mut snapshot.windows {
            window.reset_at = Some((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        }
        fs::write(dir.join(CLAUDE_SNAPSHOT_FILE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert!(collect_claude("fixture", &dir).windows.iter().all(|window| window.used_percent.is_none()));
        fs::write(dir.join(".claude.json"), r#"{"oauthAccount":{"accountUuid":"new-account"}}"#).unwrap();
        assert!(collect_claude("fixture", &dir).windows.is_empty(), "a re-login invalidates old capacity");
    }

    #[test]
    fn cas_e1f4_picker_summary_marks_a_used_up_window_with_its_reset() {
        let window = |name: &str, used: f64, reset: Option<&str>| LimitWindow {
            name: name.into(),
            used_percent: Some(used),
            reset_at: reset.map(str::to_string),
            spend_dollars: None,
        };
        let limit = |windows| AccountLimit {
            account: "a".into(),
            provider: "claude".into(),
            plan: None,
            state: "active".into(),
            source: "live".into(),
            observed_at: None,
            windows,
            detail: None,
        };
        let now = Utc::now();
        let reset = now + chrono::Duration::hours(6);
        let stamp = reset.to_rfc3339();
        let full = limit(vec![window("5h", 100., Some(&stamp)), window("weekly", 40., Some(&stamp))]);
        let summary = summarize(&full).unwrap();
        assert!(summary.contains(&format!("5h limit hit, {}", format_reset(Some(reset), now))), "{summary}");
        assert!(summary.contains("weekly 40%") && summary.matches("(in ").count() == 2, "{summary}");
        let report = LimitsReport { accounts: vec![full] };
        let human = render_human(&report);
        assert!(human.matches("(in ").count() == 2 && human.lines().all(|line| ratatui::text::Line::from(line).width() <= 80), "{human}");
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["accounts"][0]["windows"][0]["reset_at"], stamp);
        let mut compact_report = LimitsReport { accounts: Vec::new() };
        for (name, hours) in [("account-first", 1), ("account-next", 2), ("account-third", 3), ("account-last", 4)] {
            let stamp = (now + chrono::Duration::hours(hours)).to_rfc3339();
            let mut account = limit(vec![window("5h", 20., Some(&stamp)), window("weekly", 96., Some(&stamp))]);
            account.account = name.into(); compact_report.accounts.push(account);
        }
        let compact = compact_line(&compact_report);
        assert!(compact.starts_with("account-first 96% resets "), "{compact}");
        assert!(!compact.contains('\n') && ratatui::text::Line::from(compact.as_str()).width() <= 80, "{compact}");
        assert!(compact.ends_with("more") && compact.contains('+'), "{compact}");
        assert_eq!(summarize(&limit(Vec::new())), None);
    }

    #[test]
    fn parses_exa_cost_shape_without_secrets() {
        let v2: Value = serde_json::json!({"costDollars":{"total":0.007}});
        let legacy: Value = serde_json::json!({"costDollars":0.003});
        assert_eq!(parse_exa_cost(&v2), Some(0.007));
        assert_eq!(parse_exa_cost(&legacy), Some(0.003));
    }
}
