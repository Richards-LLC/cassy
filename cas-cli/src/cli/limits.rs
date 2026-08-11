//! Machine-local provider capacity snapshots for worker routing.
//!
//! This deliberately reads credentials only long enough to make the Claude
//! usage request. Tokens are never included in errors or output.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Local, TimeZone, Utc};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::cli::Cli;

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

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
    windows: Vec<LimitWindow>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct LimitWindow {
    name: String,
    used_percent: Option<f64>,
    reset_at: Option<String>,
    spend_dollars: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<ClaudeOauth>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
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
    accounts.push(collect_claude("claude-main", &home.join(".claude")));
    accounts.push(collect_claude("claude-alt", &home.join(".claude-alt")));
    accounts.push(collect_codex(&home.join(".codex")));
    accounts.push(collect_exa());
    LimitsReport { accounts }
}

fn collect_claude(account: &str, config_dir: &Path) -> AccountLimit {
    let credentials_path = config_dir.join(".credentials.json");
    let credentials: ClaudeCredentials = match fs::read_to_string(&credentials_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(value) => value,
        None => return unavailable(account, "claude", "absent", "credential file not found"),
    };
    let Some(oauth) = credentials.oauth else {
        return unavailable(
            account,
            "claude",
            "auth-needed",
            "OAuth credentials not available",
        );
    };
    let plan = oauth.subscription_type;
    let response = ureq::get(CLAUDE_USAGE_URL)
        .set("Authorization", &format!("Bearer {}", oauth.access_token))
        .set("Accept", "application/json")
        .set("anthropic-version", "2023-06-01")
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(std::time::Duration::from_secs(8))
        .call();
    match response {
        Ok(response) => match response.into_json::<Value>().ok().map(parse_claude_usage) {
            Some(windows) if !windows.is_empty() => AccountLimit {
                account: account.to_string(),
                provider: "claude".to_string(),
                plan,
                state: "active".to_string(),
                source: "live".to_string(),
                windows,
                detail: None,
            },
            _ => unavailable_with_plan(
                account,
                "claude",
                plan,
                "unavailable",
                "usage response had no recognized windows",
            ),
        },
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            unavailable_with_plan(
                account,
                "claude",
                plan,
                "auth-needed",
                "OAuth token rejected; run claude login",
            )
        }
        Err(_) => unavailable_with_plan(
            account,
            "claude",
            plan,
            "unavailable",
            "usage endpoint unavailable",
        ),
    }
}

fn collect_codex(root: &Path) -> AccountLimit {
    let Some((plan, windows)) = latest_codex_limits(root) else {
        return unavailable("codex", "codex", "absent", "no rollout rate_limits found");
    };
    AccountLimit {
        account: "codex".to_string(),
        provider: "codex".to_string(),
        plan,
        state: "active".to_string(),
        source: "live transcript".to_string(),
        windows,
        detail: None,
    }
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
        windows: Vec::new(),
        detail: Some(detail.to_string()),
    }
}

fn parse_claude_usage(value: Value) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    for (key, label) in [
        ("five_hour", "5h"),
        ("seven_day", "weekly"),
        ("seven_day_opus", "weekly opus"),
        ("seven_day_sonnet", "weekly sonnet"),
    ] {
        let Some(bucket) = value.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        windows.push(LimitWindow {
            name: label.to_string(),
            used_percent: bucket.get("utilization").and_then(Value::as_f64),
            reset_at: bucket
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(local_rfc3339),
            spend_dollars: None,
        });
    }
    if let Some(extra) = value.get("extra_usage").filter(|v| !v.is_null()) {
        windows.push(LimitWindow {
            name: "extra usage".to_string(),
            used_percent: extra.get("utilization").and_then(Value::as_f64),
            reset_at: None,
            spend_dollars: extra.get("used_credits").and_then(Value::as_f64),
        });
    }
    windows
}

fn latest_codex_limits(root: &Path) -> Option<(Option<String>, Vec<LimitWindow>)> {
    let sessions = root.join("sessions");
    let newest = WalkDir::new(sessions)
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
        .max_by_key(|(modified, _)| *modified)?
        .1;
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
        .map(|time| time.format("%Y-%m-%d %H:%M %Z").to_string())
}

fn local_rfc3339(timestamp: &str) -> Option<String> {
    timestamp.parse::<DateTime<Utc>>().ok().map(|time| {
        time.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M %Z")
            .to_string()
    })
}

fn compact_line(report: &LimitsReport) -> String {
    report
        .accounts
        .iter()
        .map(|account| {
            let windows = account
                .windows
                .iter()
                .map(|window| format!("{}:{:.0}%", window.name, window.used_percent.unwrap_or(0.0)))
                .collect::<Vec<_>>()
                .join(",");
            if windows.is_empty() {
                format!("{}:{}", account.account, account.state)
            } else {
                format!("{} {}", account.account, windows)
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn print_human(report: &LimitsReport) {
    println!("CAS provider limits");
    for account in &report.accounts {
        let plan = account
            .plan
            .as_deref()
            .map(|plan| format!(" ({plan})"))
            .unwrap_or_default();
        println!(
            "\n{}{} — {} [{}]",
            account.account, plan, account.state, account.source
        );
        for window in &account.windows {
            let used = window
                .used_percent
                .map(|value| format!("{value:.1}% used"))
                .unwrap_or_else(|| "usage unavailable".to_string());
            let reset = window
                .reset_at
                .as_deref()
                .map(|value| format!(", resets {value}"))
                .unwrap_or_default();
            let spend = window
                .spend_dollars
                .map(|value| format!(", ${value:.2} spent"))
                .unwrap_or_default();
            println!("  {}: {}{}{}", window.name, used, spend, reset);
        }
        if let Some(detail) = &account.detail {
            println!("  {detail}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_primary_and_secondary_windows() {
        let value: Value = serde_json::json!({"primary":{"used_percent":26.0,"window_minutes":10080,"resets_at":1787011690},"secondary":{"used_percent":7.0,"window_minutes":300,"resets_at":1786651690},"plan_type":"pro"});
        let windows = parse_codex_limits(&value);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].name, "weekly");
        assert_eq!(windows[1].name, "5h");
        assert_eq!(windows[0].used_percent, Some(26.0));
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
    fn parses_claude_windows_and_optional_model_caps() {
        let value: Value = serde_json::json!({"five_hour":{"utilization":6.0,"resets_at":"2026-04-08T18:59:59Z"},"seven_day":null,"seven_day_opus":{"utilization":12.0,"resets_at":"2026-04-14T17:59:59Z"},"extra_usage":{"utilization":12.5,"used_credits":12.5}});
        let windows = parse_claude_usage(value);
        assert_eq!(
            windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            vec!["5h", "weekly opus", "extra usage"]
        );
        assert_eq!(windows[2].spend_dollars, Some(12.5));
    }

    #[test]
    fn parses_exa_cost_shape_without_secrets() {
        let v2: Value = serde_json::json!({"costDollars":{"total":0.007}});
        let legacy: Value = serde_json::json!({"costDollars":0.003});
        assert_eq!(parse_exa_cost(&v2), Some(0.007));
        assert_eq!(parse_exa_cost(&legacy), Some(0.003));
    }
}
