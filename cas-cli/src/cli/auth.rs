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
    FetchTeamsOutcome, default_endpoint, fetch_and_cache_teams, is_acceptable_endpoint,
    maybe_apply_team_backfill,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

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

    // Check if already logged in
    if let Ok(config) = CloudConfig::load() {
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
    let expires_in = device_response["expires_in"].as_u64().unwrap_or(900);
    let interval = device_response["interval"].as_u64().unwrap_or(5);

    if cli.json {
        println!(
            r#"{{"status":"pending","user_code":"{user_code}","verification_uri":"{verification_uri}"}}"#
        );
    } else {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);

        // Display the code prominently
        print_device_code(&mut fmt, user_code, verification_uri)?;

        // Open browser if not disabled
        if !args.no_browser {
            let url_with_code = format!("{verification_uri}?code={user_code}");
            if open_browser(&url_with_code).is_ok() {
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
    }

    // Step 2: Poll for authorization
    let token_url = format!("{}/device/token", args.endpoint);
    let poll_interval = Duration::from_secs(interval);
    let max_attempts = (expires_in / interval) as usize;
    let theme = ActiveTheme::default();

    let mut spinner = Spinner::new("Waiting for authorization...");
    let mut prev_lines = if !cli.json {
        render_inline_view(&spinner, &theme)?
    } else {
        0
    };

    for attempt in 0..max_attempts {
        // Animate spinner during sleep interval
        if !cli.json {
            let tick_interval = Duration::from_millis(80);
            let num_ticks = poll_interval.as_millis() / tick_interval.as_millis();
            for _ in 0..num_ticks {
                spinner.update(SpinnerMsg::Tick);
                prev_lines = rerender_inline(&spinner, prev_lines, &theme)?;
                std::thread::sleep(tick_interval);
            }
        } else {
            std::thread::sleep(poll_interval);
        }

        let poll_response = ureq::post(&token_url)
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({
                "device_code": device_code
            }));

        match poll_response {
            Ok(resp) => {
                let body: serde_json::Value = resp.into_json()?;
                let status = body["status"].as_str().unwrap_or("");

                match status {
                    "authorized" => {
                        if !cli.json {
                            clear_inline(prev_lines)?;
                        }

                        let access_token = body["access_token"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("No access token in response"))?;
                        let email = body["user"]["email"].as_str();
                        let plan = body["user"]["plan"].as_str();

                        // Save config
                        {
                            let mut config = CloudConfig::load().unwrap_or_default();
                            config.endpoint = args.endpoint.clone();
                            config.token = Some(access_token.to_string());
                            config.email = email.map(String::from);
                            config.plan = plan.map(String::from);
                            config.save()?;
                        }

                        // Best-effort: fetch team membership from /api/me and
                        // cache into ~/.cas/cloud.json so T3's resolution chain
                        // works offline immediately after login.
                        let membership_outcome =
                            fetch_and_cache_teams(&args.endpoint, access_token);
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
                            FetchTeamsOutcome::AuthFailed => {
                                eprintln!(
                                    "warning: could not fetch team membership (/api/me returned 401). \
                                     Run `cas cloud login` again to refresh."
                                );
                            }
                            FetchTeamsOutcome::NetworkError(msg) => {
                                eprintln!(
                                    "warning: could not fetch team membership: {msg}. \
                                     Team auto-scope will work after the next `cas cloud sync`."
                                );
                            }
                        }

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
                    "authorization_pending" => {
                        if !cli.json {
                            let remaining = expires_in - (attempt as u64 * interval);
                            spinner.update(SpinnerMsg::SetMessage(format!(
                                "Waiting for authorization... ({remaining}s remaining)"
                            )));
                        }
                    }
                    "access_denied" => {
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
                    "expired_token" => {
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
                    _ => {}
                }
            }
            Err(ureq::Error::Status(202, _)) => {
                // Still pending, continue
            }
            Err(ureq::Error::Status(code, resp)) => {
                if !cli.json {
                    clear_inline(prev_lines)?;
                }
                let body: serde_json::Value = resp.into_json().unwrap_or_default();
                let status = body["status"].as_str().unwrap_or("");

                match status {
                    "authorization_pending" => continue,
                    "access_denied" => {
                        if cli.json {
                            println!(r#"{{"status":"denied"}}"#);
                        } else {
                            let mut err = io::stderr();
                            let mut fmt = Formatter::stdout(&mut err, theme.clone());
                            fmt.newline()?;
                            fmt.write_raw("  ")?;
                            fmt.error("Authorization denied")?;
                        }
                        return Ok(());
                    }
                    "expired_token" => {
                        if cli.json {
                            println!(r#"{{"status":"expired"}}"#);
                        } else {
                            let mut err = io::stderr();
                            let mut fmt = Formatter::stdout(&mut err, theme.clone());
                            fmt.newline()?;
                            fmt.write_raw("  ")?;
                            fmt.error("Code expired")?;
                        }
                        return Ok(());
                    }
                    _ => {
                        if cli.json {
                            println!(r#"{{"status":"error","code":{code}}}"#);
                        } else {
                            let mut err = io::stderr();
                            let mut fmt = Formatter::stdout(&mut err, theme.clone());
                            fmt.newline()?;
                            fmt.write_raw("  ")?;
                            fmt.error(&format!("Server error ({code})"))?;
                        }
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                if !cli.json {
                    clear_inline(prev_lines)?;
                }
                if cli.json {
                    println!(r#"{{"status":"error","message":"Connection lost"}}"#);
                } else {
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme.clone());
                    fmt.newline()?;
                    fmt.write_raw("  ")?;
                    fmt.error(&format!("Connection lost: {e}"))?;
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

    // Save config
    {
        use crate::cloud::CloudConfig;

        let mut config = CloudConfig::load().unwrap_or_default();
        config.endpoint = endpoint.to_string();
        config.token = Some(token.to_string());
        config.save()?;
    }

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

        let mut config = CloudConfig::load().unwrap_or_default();

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
        config.logout();
        config.save()?;

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

        let config = CloudConfig::load().unwrap_or_default();

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
