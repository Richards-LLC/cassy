//! Authentication CLI commands
//!
//! Provides login, logout, and whoami commands using CAS Cloud device flow.

use std::io;

use clap::{Parser, Subcommand};

use crate::cli::Cli;
use crate::cli::cloud::{
    LoginTeamSelection, print_backfill_notice, print_login_team_selection,
    select_cached_team_after_login,
};
use crate::cloud::{
    CloudConfig, FetchTeamsOutcome, clear_login_credentials, default_endpoint, fetch_and_cache_teams,
    is_acceptable_endpoint, maybe_apply_team_backfill, store_login_credentials,
};
use crate::ui::components::{
    Component, Formatter, Spinner, SpinnerMsg, clear_inline, render_inline_view, rerender_inline,
};
use crate::ui::theme::{ActiveTheme, Icons};

/// Authentication commands
#[derive(Subcommand, Clone)]
pub enum AuthCommands {
    /// Log in to CAS Cloud
    Login(LoginArgs),

    /// Log out and clear credentials
    Logout,

    /// Show current user information
    Whoami,
}

#[derive(Parser, Clone)]
pub struct LoginArgs {
    /// API token (skip device flow, use direct token)
    #[arg(long, env = "CAS_CLOUD_TOKEN")]
    pub token: Option<String>,

    /// Cloud API endpoint
    #[arg(
        long,
        env = "CAS_CLOUD_ENDPOINT",
        default_value = "https://petra-stella-cloud.vercel.app",
        value_parser = parse_endpoint,
    )]
    pub endpoint: String,

    /// Don't open browser automatically
    #[arg(long)]
    pub no_browser: bool,
}

impl Default for LoginArgs {
    fn default() -> Self {
        Self {
            token: None,
            endpoint: default_endpoint(),
            no_browser: false,
        }
    }
}

/// Validate an endpoint value: accept https://* or http://localhost variants only.
/// Rejects empty strings, file:// URLs, and arbitrary http:// hosts.
fn parse_endpoint(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        return Err("endpoint must not be empty".into());
    }
    if is_acceptable_endpoint(s) {
        Ok(s.to_string())
    } else {
        Err(format!(
            "endpoint must be https:// or http://localhost (got {s:?})"
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DEVICE-FLOW PURE HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the URL that opens the device-authorization page in a browser.
///
/// The `/device/code` response may already carry the user code — either as
/// RFC 8628's `verification_uri_complete`, or (as CAS Cloud does today) baked
/// into `verification_uri` itself. Appending `?code=` unconditionally produced
/// `…/device?code=FEUE-NMWQ?code=FEUE-NMWQ`, and the page then read the whole
/// blob as the code and failed (cas-046d).
///
/// Rules, in order:
///  1. an explicit `verification_uri_complete` is used verbatim;
///  2. a `verification_uri` that already has a `code` query parameter is left
///     alone;
///  3. otherwise the code is appended with the correct separator and
///     percent-encoded.
pub(crate) fn build_verification_url(
    verification_uri: &str,
    verification_uri_complete: Option<&str>,
    user_code: &str,
) -> String {
    if let Some(complete) = verification_uri_complete
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return complete.to_string();
    }

    let base = verification_uri.trim();
    if user_code.is_empty() || has_code_param(base) {
        return base.to_string();
    }

    let separator = if base.contains('?') { '&' } else { '?' };
    format!(
        "{base}{separator}code={}",
        urlencoding::encode(user_code.trim())
    )
}

/// True when `url`'s query string already contains a `code` parameter.
fn has_code_param(url: &str) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').any(|pair| {
        let name = pair.split_once('=').map(|(name, _)| name).unwrap_or(pair);
        name.eq_ignore_ascii_case("code")
    })
}

/// What the CLI should do after one `/device/token` poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollDecision {
    /// Nobody has approved the code yet — keep waiting at the current cadence.
    Pending,
    /// The server asked us to slow down: RFC 8628 `slow_down`, or HTTP 429.
    SlowDown { retry_after: Option<u64> },
    /// Transient failure (5xx, timeout, dropped connection) — retry, backed off.
    Transient,
    /// The user rejected the request.
    Denied,
    /// The device code is no longer valid.
    Expired,
    /// Unrecoverable: stop and report the HTTP status.
    Fatal { http_status: u16 },
}

/// Map one poll result to the next action.
///
/// `body_status` is the `status` (or `error`) field of the response body;
/// `retry_after` is the parsed `Retry-After` header, when the server sent one.
///
/// The rate-limit case is the point of this function: a 429 during ordinary
/// login polling used to abort the whole login with "Server error (429)"
/// (cas-046d / Ben #7). It is a throttle, not a failure.
pub(crate) fn classify_poll_status(
    http_status: u16,
    body_status: &str,
    retry_after: Option<u64>,
) -> PollDecision {
    match body_status {
        "authorization_pending" | "pending" => return PollDecision::Pending,
        "slow_down" => return PollDecision::SlowDown { retry_after },
        "access_denied" => return PollDecision::Denied,
        "expired_token" | "expired" => return PollDecision::Expired,
        _ => {}
    }

    match http_status {
        429 => PollDecision::SlowDown { retry_after },
        200 | 202 => PollDecision::Pending,
        408 | 425 | 500..=599 => PollDecision::Transient,
        other => PollDecision::Fatal { http_status: other },
    }
}

/// Longest gap the client will wait between polls, however hard the server
/// throttles. Keeps a slow-down storm from silently parking the login.
const MAX_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Next polling interval after a throttle or transient failure.
///
/// `Retry-After` (delta-seconds) wins when the server sent one; otherwise the
/// interval grows by 5 seconds, per RFC 8628 §3.5. The result never shrinks
/// below the current interval and never exceeds [`MAX_POLL_INTERVAL`].
pub(crate) fn backoff_interval(
    current: std::time::Duration,
    retry_after: Option<u64>,
) -> std::time::Duration {
    use std::time::Duration;

    let proposed = match retry_after {
        Some(secs) if secs > 0 => Duration::from_secs(secs),
        _ => current.saturating_add(Duration::from_secs(5)),
    };
    let floor = current.min(MAX_POLL_INTERVAL);
    proposed.max(floor).min(MAX_POLL_INTERVAL)
}

/// Parse a `Retry-After` header value expressed in delta-seconds.
///
/// The HTTP-date form is deliberately unsupported: it returns `None`, and the
/// caller falls back to the RFC 8628 `+5s` rule rather than guessing a clock
/// skew.
pub(crate) fn parse_retry_after(header: Option<&str>) -> Option<u64> {
    let value = header.map(str::trim).filter(|value| !value.is_empty())?;
    value.parse::<u64>().ok().filter(|secs| *secs > 0)
}

/// The status field of a device-token response, tolerating both the CAS Cloud
/// `status` spelling and RFC 8628's `error`.
fn body_status(body: &serde_json::Value) -> &str {
    body["status"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::time::Duration;

    #[test]
    fn verification_url_does_not_double_append_code() {
        // The exact shape Ben hit: the server's verification_uri already
        // carries the code, so appending another one broke the page.
        let url = build_verification_url(
            "https://petra-stella-cloud.vercel.app/device?code=FEUE-NMWQ",
            None,
            "FEUE-NMWQ",
        );
        assert_eq!(
            url, "https://petra-stella-cloud.vercel.app/device?code=FEUE-NMWQ",
            "a verification_uri that already carries the code must be used as-is"
        );
        assert_eq!(url.matches("code=").count(), 1, "exactly one code parameter");
    }

    #[test]
    fn verification_url_appends_code_when_absent() {
        assert_eq!(
            build_verification_url("https://cloud.example/device", None, "ABCD-EFGH"),
            "https://cloud.example/device?code=ABCD-EFGH"
        );
    }

    #[test]
    fn verification_url_uses_ampersand_with_existing_query() {
        let url = build_verification_url("https://cloud.example/device?next=/home", None, "AB-CD");
        assert_eq!(url, "https://cloud.example/device?next=/home&code=AB-CD");
        assert_eq!(url.matches("code=").count(), 1);
    }

    #[test]
    fn verification_url_prefers_uri_complete() {
        let url = build_verification_url(
            "https://cloud.example/device",
            Some("https://cloud.example/device?user_code=AB-CD&code=AB-CD"),
            "AB-CD",
        );
        assert_eq!(
            url, "https://cloud.example/device?user_code=AB-CD&code=AB-CD",
            "RFC 8628 verification_uri_complete wins verbatim"
        );
    }

    #[test]
    fn verification_url_percent_encodes_the_code() {
        let url = build_verification_url("https://cloud.example/device", None, "A B&C");
        assert_eq!(url, "https://cloud.example/device?code=A%20B%26C");
    }

    #[test]
    fn verification_url_ignores_a_code_prefixed_param() {
        // `codex=` must not be mistaken for `code=`.
        let url = build_verification_url("https://cloud.example/device?codex=1", None, "AB-CD");
        assert_eq!(url, "https://cloud.example/device?codex=1&code=AB-CD");
    }

    #[test]
    fn rate_limit_backs_off_instead_of_aborting_login() {
        assert_eq!(
            classify_poll_status(429, "", None),
            PollDecision::SlowDown { retry_after: None },
            "429 during ordinary polling is a throttle, not a login failure"
        );
        assert_eq!(
            classify_poll_status(429, "", Some(30)),
            PollDecision::SlowDown {
                retry_after: Some(30)
            }
        );
    }

    #[test]
    fn poll_classification_covers_the_device_flow_states() {
        assert_eq!(
            classify_poll_status(400, "authorization_pending", None),
            PollDecision::Pending
        );
        assert_eq!(
            classify_poll_status(400, "slow_down", None),
            PollDecision::SlowDown { retry_after: None }
        );
        assert_eq!(
            classify_poll_status(400, "access_denied", None),
            PollDecision::Denied
        );
        assert_eq!(
            classify_poll_status(400, "expired_token", None),
            PollDecision::Expired
        );
        assert_eq!(classify_poll_status(502, "", None), PollDecision::Transient);
        assert_eq!(
            classify_poll_status(403, "", None),
            PollDecision::Fatal { http_status: 403 }
        );
        assert_eq!(classify_poll_status(200, "", None), PollDecision::Pending);
    }

    #[test]
    fn backoff_grows_by_five_seconds_without_retry_after() {
        assert_eq!(
            backoff_interval(Duration::from_secs(5), None),
            Duration::from_secs(10)
        );
        assert_eq!(
            backoff_interval(Duration::from_secs(10), None),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn backoff_honours_retry_after_and_never_speeds_up() {
        assert_eq!(
            backoff_interval(Duration::from_secs(5), Some(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            backoff_interval(Duration::from_secs(20), Some(2)),
            Duration::from_secs(20),
            "a shorter Retry-After must not make the client poll faster"
        );
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(
            backoff_interval(Duration::from_secs(58), None),
            MAX_POLL_INTERVAL
        );
        assert_eq!(
            backoff_interval(Duration::from_secs(30), Some(3600)),
            MAX_POLL_INTERVAL
        );
    }

    #[test]
    fn retry_after_parses_delta_seconds_only() {
        assert_eq!(parse_retry_after(Some("30")), Some(30));
        assert_eq!(parse_retry_after(Some("  30 ")), Some(30));
        assert_eq!(parse_retry_after(Some("0")), None);
        assert_eq!(parse_retry_after(Some("")), None);
        assert_eq!(parse_retry_after(None), None);
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            None,
            "HTTP-date form falls back to the RFC 8628 +5s rule"
        );
    }

    #[test]
    fn body_status_accepts_both_spellings() {
        assert_eq!(
            body_status(&serde_json::json!({"status": "slow_down"})),
            "slow_down"
        );
        assert_eq!(
            body_status(&serde_json::json!({"error": "authorization_pending"})),
            "authorization_pending"
        );
        assert_eq!(body_status(&serde_json::json!({})), "");
    }

    #[test]
    fn login_args_default_uses_default_endpoint() {
        let mut g =
            TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "https://staging.example.com");
        let args = LoginArgs::default();
        assert_eq!(
            args.endpoint,
            "https://staging.example.com",
            "LoginArgs::default() must delegate to default_endpoint() so env var is honoured"
        );
    }

    #[test]
    fn parse_endpoint_rejects_http_attacker() {
        let result = parse_endpoint("http://attacker.com");
        assert!(
            result.is_err(),
            "http://attacker.com must be rejected by parse_endpoint"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("https://") || msg.contains("http://localhost"),
            "error message should describe allowed schemes, got: {msg}"
        );
    }

    #[test]
    fn parse_endpoint_accepts_https() {
        assert_eq!(
            parse_endpoint("https://petra-stella-cloud.vercel.app"),
            Ok("https://petra-stella-cloud.vercel.app".to_string())
        );
    }

    #[test]
    fn parse_endpoint_accepts_http_localhost() {
        assert_eq!(
            parse_endpoint("http://localhost:8080"),
            Ok("http://localhost:8080".to_string())
        );
    }

    #[test]
    fn parse_endpoint_rejects_empty() {
        assert!(parse_endpoint("").is_err());
        assert!(parse_endpoint("   ").is_err());
    }
}

/// Execute an auth subcommand
pub fn execute(cmd: &AuthCommands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        AuthCommands::Login(args) => execute_login(args, cli),
        AuthCommands::Logout => execute_logout(cli),
        AuthCommands::Whoami => execute_whoami(cli),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// LOGIN
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_login(args: &LoginArgs, cli: &Cli) -> anyhow::Result<()> {
    // If token provided directly, use direct token flow
    if let Some(token) = &args.token {
        return execute_login_with_token(token, &args.endpoint, cli);
    }

    execute_device_flow_login(args, cli)
}

fn execute_device_flow_login(args: &LoginArgs, cli: &Cli) -> anyhow::Result<()> {
    use std::time::Duration;

    use crate::cloud::CloudConfig;

    // Check if already logged in. `load_effective` so a machine-wide login is
    // recognised from any project — and from outside a project entirely.
    {
        let config = CloudConfig::load_effective();
        if config.is_logged_in() {
            if cli.json {
                let output = serde_json::json!({
                    "status": "already_logged_in",
                    "email": config.email,
                    "plan": config.plan,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let email = config.email.as_deref().unwrap_or("unknown");
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.write_raw(&format!("Already logged in as {email}."))?;
                fmt.newline()?;
                fmt.write_raw("Use ")?;
                fmt.write_accent("cas logout")?;
                fmt.write_raw(" to log out first.")?;
                fmt.newline()?;
            }
            return Ok(());
        }
    }

    // Show header
    if !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        print_login_header(&mut fmt)?;
    }

    // Step 1: Request device code
    let code_url = format!("{}/device/code", args.endpoint);
    let response = ureq::post(&code_url)
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "client_name": "CAS CLI"
        }));

    let device_response: serde_json::Value = match response {
        Ok(resp) => resp.into_json()?,
        Err(e) => {
            if cli.json {
                println!(r#"{{"status":"error","message":"Failed to connect to CAS Cloud"}}"#);
            } else {
                let mut err = io::stderr();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut err, theme);
                fmt.newline()?;
                fmt.write_raw("  ")?;
                fmt.error("Failed to connect to CAS Cloud")?;
                fmt.write_raw(&format!("    {e}"))?;
                fmt.newline()?;
            }
            return Ok(());
        }
    };

    let device_code = device_response["device_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid response from server"))?;
    let user_code = device_response["user_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid response from server"))?;
    let default_verification_uri = format!("{}/device", args.endpoint);
    let verification_uri = device_response["verification_uri"]
        .as_str()
        .unwrap_or(&default_verification_uri);
    let verification_uri_complete = device_response["verification_uri_complete"].as_str();
    let expires_in = device_response["expires_in"].as_u64().unwrap_or(900);
    let interval = device_response["interval"].as_u64().unwrap_or(5);

    // One code parameter, whether the server pre-baked it or not (cas-046d).
    let browser_url = build_verification_url(verification_uri, verification_uri_complete, user_code);

    if cli.json {
        println!(
            r#"{{"status":"pending","user_code":"{user_code}","verification_uri":"{browser_url}"}}"#
        );
    } else {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);

        // Display the code prominently — the same URL the browser will open.
        print_device_code(&mut fmt, user_code, &browser_url)?;

        // Open browser if not disabled
        if !args.no_browser {
            if open_browser(&browser_url).is_ok() {
                fmt.write_raw("  ")?;
                fmt.write_accent(&format!("{} ", Icons::ARROW_RIGHT))?;
                fmt.write_raw("Opening browser...")?;
                fmt.newline()?;
                fmt.newline()?;
            } else {
                fmt.write_raw("  ")?;
                fmt.write_accent(&format!("{} ", Icons::ARROW_RIGHT))?;
                fmt.write_raw("Please open the URL above in your browser")?;
                fmt.newline()?;
                fmt.newline()?;
            }
        } else {
            fmt.write_raw("  ")?;
            fmt.write_accent(&format!("{} ", Icons::ARROW_RIGHT))?;
            fmt.write_raw("Open the URL above in your browser")?;
            fmt.newline()?;
            fmt.newline()?;
        }

        print_token_fallback_hint(&mut fmt)?;
    }

    // Step 2: Poll for authorization.
    //
    // Cadence is server-driven (cas-046d / Ben #7): the interval starts at the
    // server's advertised value and only ever grows — on `slow_down`, on HTTP
    // 429 (honouring `Retry-After`), and on transient 5xx/network errors. The
    // loop is bounded by the code's own expiry rather than a precomputed
    // attempt count, so backing off shortens the number of requests instead of
    // extending the wait past `expires_in`.
    let token_url = format!("{}/device/token", args.endpoint);
    let mut poll_interval = Duration::from_secs(interval.clamp(1, MAX_POLL_INTERVAL.as_secs()));
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    let mut consecutive_transient: u32 = 0;
    let mut last_transport_error: Option<String> = None;
    /// Consecutive transport/5xx failures tolerated before giving up.
    const MAX_CONSECUTIVE_TRANSIENT: u32 = 5;
    let theme = ActiveTheme::default();

    let mut spinner = Spinner::new("Waiting for authorization...");
    let mut prev_lines = if !cli.json {
        render_inline_view(&spinner, &theme)?
    } else {
        0
    };

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = poll_interval.min(remaining);

        // Animate spinner during sleep interval
        if !cli.json {
            let tick_interval = Duration::from_millis(80);
            let wait_started = std::time::Instant::now();
            while wait_started.elapsed() < wait {
                spinner.update(SpinnerMsg::Tick);
                prev_lines = rerender_inline(&spinner, prev_lines, &theme)?;
                std::thread::sleep(tick_interval);
            }
        } else {
            std::thread::sleep(wait);
        }

        let poll_response = ureq::post(&token_url)
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({
                "device_code": device_code
            }));

        let decision = match poll_response {
            Ok(resp) => {
                let body: serde_json::Value = resp.into_json()?;
                let status = body_status(&body).to_string();

                if status == "authorized" {
                    if !cli.json {
                        clear_inline(prev_lines)?;
                    }

                    let access_token = body["access_token"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("No access token in response"))?;
                    let email = body["user"]["email"].as_str();
                    let plan = body["user"]["plan"].as_str();

                    // Credentials are user-level: one login serves every
                    // project on the machine (cas-046d).
                    store_login_credentials(&args.endpoint, access_token, email, plan)?;

                    // Best-effort: fetch team membership from /api/me and
                    // cache into ~/.cas/cloud.json so T3's resolution chain
                    // works offline immediately after login.
                    let membership_outcome = fetch_and_cache_teams(&args.endpoint, access_token);
                    report_membership_outcome(&membership_outcome);

                    if matches!(
                        membership_outcome,
                        FetchTeamsOutcome::Updated { .. } | FetchTeamsOutcome::Empty
                    ) {
                        apply_login_team_selection(cli);
                    }

                    // T6: first-run backfill — auto-promote to team scope on first
                    // login when the user has exactly one team (or the server already
                    // set a default).  Best-effort; errors in the write are ignored.
                    let backfill_outcome = maybe_apply_team_backfill();
                    print_backfill_notice(cli, &backfill_outcome);

                    if cli.json {
                        println!(r#"{{"status":"ok","email":"{}"}}"#, email.unwrap_or(""));
                    } else {
                        let mut out = io::stdout();
                        let mut fmt = Formatter::stdout(&mut out, theme.clone());
                        print_login_success(&mut fmt, email)?;
                    }

                    return Ok(());
                }

                classify_poll_status(200, &status, None)
            }
            Err(ureq::Error::Status(202, _)) => PollDecision::Pending,
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = parse_retry_after(resp.header("Retry-After"));
                let body: serde_json::Value = resp.into_json().unwrap_or_default();
                classify_poll_status(code, body_status(&body), retry_after)
            }
            Err(error) => {
                last_transport_error = Some(error.to_string());
                PollDecision::Transient
            }
        };

        let remaining_secs = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();

        match decision {
            PollDecision::Pending => {
                consecutive_transient = 0;
                if !cli.json {
                    spinner.update(SpinnerMsg::SetMessage(format!(
                        "Waiting for authorization... ({remaining_secs}s remaining)"
                    )));
                }
            }
            PollDecision::SlowDown { retry_after } => {
                consecutive_transient = 0;
                poll_interval = backoff_interval(poll_interval, retry_after);
                if !cli.json {
                    spinner.update(SpinnerMsg::SetMessage(format!(
                        "Waiting for authorization... (server busy; retrying every {}s, {remaining_secs}s remaining)",
                        poll_interval.as_secs()
                    )));
                }
            }
            PollDecision::Transient => {
                consecutive_transient += 1;
                if consecutive_transient >= MAX_CONSECUTIVE_TRANSIENT {
                    if !cli.json {
                        clear_inline(prev_lines)?;
                    }
                    let detail = last_transport_error
                        .clone()
                        .unwrap_or_else(|| "the server is not responding".to_string());
                    if cli.json {
                        println!(r#"{{"status":"error","message":"Connection lost"}}"#);
                    } else {
                        let mut err = io::stderr();
                        let mut fmt = Formatter::stdout(&mut err, theme.clone());
                        fmt.newline()?;
                        fmt.write_raw("  ")?;
                        fmt.error(&format!("Connection lost: {detail}"))?;
                    }
                    return Ok(());
                }
                poll_interval = backoff_interval(poll_interval, None);
            }
            PollDecision::Denied => {
                if !cli.json {
                    clear_inline(prev_lines)?;
                }
                if cli.json {
                    println!(r#"{{"status":"denied","message":"Authorization denied"}}"#);
                } else {
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme.clone());
                    fmt.newline()?;
                    fmt.write_raw("  ")?;
                    fmt.error("Authorization denied")?;
                }
                return Ok(());
            }
            PollDecision::Expired => {
                if !cli.json {
                    clear_inline(prev_lines)?;
                }
                if cli.json {
                    println!(r#"{{"status":"expired","message":"Code expired"}}"#);
                } else {
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme.clone());
                    fmt.newline()?;
                    fmt.write_raw("  ")?;
                    fmt.error("Code expired. Please try again.")?;
                }
                return Ok(());
            }
            PollDecision::Fatal { http_status } => {
                if !cli.json {
                    clear_inline(prev_lines)?;
                }
                if cli.json {
                    println!(r#"{{"status":"error","code":{http_status}}}"#);
                } else {
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme.clone());
                    fmt.newline()?;
                    fmt.write_raw("  ")?;
                    fmt.error(&format!("Server error ({http_status})"))?;
                    fmt.newline()?;
                    print_token_fallback_hint(&mut fmt)?;
                }
                return Ok(());
            }
        }
    }

    if !cli.json {
        clear_inline(prev_lines)?;
    }

    if cli.json {
        println!(r#"{{"status":"timeout"}}"#);
    } else {
        let mut err = io::stderr();
        let mut fmt = Formatter::stdout(&mut err, theme);
        fmt.newline()?;
        fmt.write_raw("  ")?;
        fmt.error("Authorization timed out. Please try again.")?;
    }

    Ok(())
}

fn execute_login_with_token(token: &str, endpoint: &str, cli: &Cli) -> anyhow::Result<()> {
    if token.is_empty() {
        anyhow::bail!("Token cannot be empty");
    }

    // Verify token
    let status_url = format!("{endpoint}/api/sync/status");
    let response = ureq::get(&status_url)
        .set("Authorization", &format!("Bearer {token}"))
        .call();

    match response {
        Ok(resp) => {
            if resp.status() != 200 {
                anyhow::bail!("Invalid token or server error: {}", resp.status());
            }
        }
        Err(ureq::Error::Status(401, _)) => {
            anyhow::bail!("Invalid API token");
        }
        Err(e) => {
            anyhow::bail!("Failed to connect to CAS Cloud: {e}");
        }
    }

    // Credentials live at user level (`~/.cas/cloud.json`), so this works from
    // any directory — including outside a CAS project — and logs the whole
    // machine in once (cas-046d / Ben #3, #4).
    store_login_credentials(endpoint, token, None, None)?;

    // Best-effort: fetch team membership from /api/me and cache into
    // ~/.cas/cloud.json so T3's resolution chain works immediately.
    let membership_outcome = fetch_and_cache_teams(endpoint, token);
    match &membership_outcome {
        FetchTeamsOutcome::Updated { team_count } => {
            tracing::debug!(
                team_count,
                "fetched and cached team membership from /api/me"
            );
        }
        FetchTeamsOutcome::Empty => {
            tracing::debug!("logged in but /api/me returned zero team memberships");
        }
        FetchTeamsOutcome::AuthFailed | FetchTeamsOutcome::NetworkError(_) => {
            // Token was just verified, so a 401 or network error here is
            // a transient anomaly.  Swallow it silently; the next sync
            // will retry via the lazy-refresh path.
            tracing::warn!("could not fetch team membership from /api/me during token login (non-fatal)");
        }
    }

    if matches!(
        membership_outcome,
        FetchTeamsOutcome::Updated { .. } | FetchTeamsOutcome::Empty
    ) {
        apply_login_team_selection(cli);
    }

    // T6: first-run backfill — auto-promote to team scope on first login when
    // the user has exactly one team (or the server already set a default).
    let backfill_outcome = maybe_apply_team_backfill();
    print_backfill_notice(cli, &backfill_outcome);

    if cli.json {
        println!(r#"{{"status":"ok","message":"Logged in successfully"}}"#);
    } else {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.write_raw("  ")?;
        fmt.success("Logged in to CAS Cloud")?;
    }

    Ok(())
}

/// Report the best-effort `/api/me` membership refresh that follows a login.
fn report_membership_outcome(outcome: &FetchTeamsOutcome) {
    match outcome {
        FetchTeamsOutcome::Updated { team_count } => {
            tracing::debug!(
                team_count,
                "fetched and cached team membership from /api/me"
            );
        }
        FetchTeamsOutcome::Empty => {
            tracing::debug!("logged in but /api/me returned zero team memberships");
        }
        FetchTeamsOutcome::AuthFailed => {
            eprintln!(
                "warning: could not fetch team membership (/api/me returned 401). \
                 Run `cas login` again to refresh."
            );
        }
        FetchTeamsOutcome::NetworkError(msg) => {
            eprintln!(
                "warning: could not fetch team membership: {msg}. \
                 Team auto-scope will work after the next `cas cloud sync`."
            );
        }
    }
}

/// Point at the token path, which does not depend on the device-approval page.
///
/// The hosted `/device` page currently rejects an otherwise valid code with
/// "Missing or invalid Authorization header" even for a signed-in session
/// (cas-046d, server-side; see `docs/reports/2026-08-18-cloud-device-login-server-defect.md`).
/// Until that is fixed, browser approval can fail through no fault of the CLI,
/// so every device-flow screen names the working alternative.
fn print_token_fallback_hint(fmt: &mut Formatter) -> io::Result<()> {
    fmt.write_muted("  If the approval page reports an authorization error, use a token instead:")?;
    fmt.newline()?;
    fmt.write_raw("    ")?;
    fmt.write_accent("cas login --token <API-TOKEN>")?;
    fmt.newline()?;
    fmt.write_muted("  It works from any directory and logs in every project on this machine.")?;
    fmt.newline()?;
    fmt.newline()
}

/// Use the cached-membership resolver behind `cas cloud team set` to make a
/// sole team membership active for the project that just logged in. Failures
/// remain non-fatal, matching the surrounding best-effort membership refresh.
fn apply_login_team_selection(cli: &Cli) {
    let user_config = match crate::cloud::user_level_cloud_json_path()
        .and_then(|path| CloudConfig::load_from(&path).ok())
    {
        Some(config) => config,
        None => return,
    };
    let mut project_config = match CloudConfig::load() {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "could not load project cloud config after login");
            return;
        }
    };

    let outcome = select_cached_team_after_login(&mut project_config, &user_config);
    if matches!(outcome, LoginTeamSelection::Activated(_))
        && let Err(error) = project_config.save()
    {
        tracing::warn!(%error, "could not save active team selected after login");
        return;
    }
    print_login_team_selection(cli, &outcome);
}

// ═══════════════════════════════════════════════════════════════════════════════
// LOGOUT
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_logout(cli: &Cli) -> anyhow::Result<()> {
    {
        use crate::cloud::CloudConfig;

        // `load_effective` + `clear_login_credentials`: logging out is a
        // machine-wide act, and it must work from outside a project too.
        let config = CloudConfig::load_effective();

        if !config.is_logged_in() {
            if cli.json {
                let output = serde_json::json!({
                    "status": "not_logged_in",
                    "message": "Not logged in."
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.write_raw("Not logged in.")?;
                fmt.newline()?;
            }
            return Ok(());
        }

        let email = config.email.clone().unwrap_or_else(|| "user".to_string());
        clear_login_credentials()?;

        if cli.json {
            let output = serde_json::json!({
                "status": "logged_out",
                "message": "Logged out successfully."
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success(&format!("Logged out successfully. Goodbye, {email}!"))?;
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// WHOAMI
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_whoami(cli: &Cli) -> anyhow::Result<()> {
    {
        use crate::cloud::CloudConfig;

        // Machine-wide credentials, so `cas whoami` answers the same question
        // in every directory (cas-046d).
        let config = CloudConfig::load_effective();

        if config.is_logged_in() {
            if cli.json {
                let output = serde_json::json!({
                    "logged_in": true,
                    "email": config.email,
                    "plan": config.plan,
                    "endpoint": config.endpoint,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                if let Some(email) = &config.email {
                    fmt.write_raw(&format!("Logged in as: {email}"))?;
                    fmt.newline()?;
                }
                if let Some(plan) = &config.plan {
                    fmt.write_muted("  Plan: ")?;
                    fmt.write_raw(plan)?;
                    fmt.newline()?;
                }
                fmt.write_muted("  Endpoint: ")?;
                fmt.write_raw(&config.endpoint)?;
                fmt.newline()?;
            }
            Ok(())
        } else {
            if cli.json {
                let output = serde_json::json!({
                    "logged_in": false,
                    "message": "Not logged in. Run 'cas login' to authenticate."
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.write_raw("Not logged in. Run ")?;
                fmt.write_accent("cas login")?;
                fmt.write_raw(" to authenticate.")?;
                fmt.newline()?;
            }
            anyhow::bail!("not logged in")
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// UI HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn print_login_header(fmt: &mut Formatter) -> io::Result<()> {
    let muted_color = fmt.theme().palette.text_muted;
    let accent_color = fmt.theme().palette.accent;

    fmt.newline()?;
    fmt.write_colored(
        "  \u{256D}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256E}",
        muted_color,
    )?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("                                                      ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}  ", muted_color)?;
    fmt.write_bold_colored(
        "\u{2588}\u{2580}\u{2580} \u{2584}\u{2580}\u{2588} \u{2588}\u{2580}",
        accent_color,
    )?;
    fmt.write_raw("     Cloud                                  ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}  ", muted_color)?;
    fmt.write_bold_colored(
        "\u{2588}\u{2584}\u{2584} \u{2588}\u{2580}\u{2588} \u{2584}\u{2588}",
        accent_color,
    )?;
    fmt.write_raw("                                            ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("                                                      ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored(
        "  \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256F}",
        muted_color,
    )?;
    fmt.newline()?;
    fmt.newline()
}

fn print_device_code(
    fmt: &mut Formatter,
    user_code: &str,
    verification_uri: &str,
) -> io::Result<()> {
    let muted_color = fmt.theme().palette.text_muted;
    let accent_color = fmt.theme().palette.accent;

    fmt.write_colored(
        "  \u{250C}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}",
        muted_color,
    )?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("                                                     ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("   Your login code:   ")?;
    fmt.write_bold_colored(user_code, accent_color)?;
    fmt.write_raw("               ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("                                                     ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw(&format!("   {verification_uri}"))?;
    // Pad to fill the box width
    let uri_len = verification_uri.len() + 3;
    let padding = 53_usize.saturating_sub(uri_len);
    fmt.write_raw(&" ".repeat(padding))?;
    fmt.write_raw("  ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored("  \u{2502}", muted_color)?;
    fmt.write_raw("                                                     ")?;
    fmt.write_colored("\u{2502}", muted_color)?;
    fmt.newline()?;
    fmt.write_colored(
        "  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}",
        muted_color,
    )?;
    fmt.newline()?;
    fmt.newline()
}

fn print_login_success(fmt: &mut Formatter, email: Option<&str>) -> io::Result<()> {
    fmt.newline()?;
    fmt.write_raw("  ")?;
    fmt.success("Successfully logged in!")?;
    fmt.newline()?;

    if let Some(email) = email {
        fmt.write_muted("  Email:  ")?;
        fmt.write_primary(email)?;
        fmt.newline()?;
    }

    fmt.newline()?;
    fmt.write_muted("  Quick start:")?;
    fmt.newline()?;
    fmt.write_raw("    ")?;
    fmt.write_accent("cas cloud push")?;
    fmt.write_raw(" Push local data to cloud")?;
    fmt.newline()?;
    fmt.write_raw("    ")?;
    fmt.write_accent("cas cloud pull")?;
    fmt.write_raw(" Pull cloud data locally")?;
    fmt.newline()?;
    fmt.write_raw("    ")?;
    fmt.write_accent("cas cloud sync")?;
    fmt.write_raw(" Full bidirectional sync")?;
    fmt.newline()?;
    fmt.newline()
}

fn open_browser(url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()?;
    }
    Ok(())
}
