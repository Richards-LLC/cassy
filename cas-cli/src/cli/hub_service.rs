//! User-level persistence for the Commander hub.
//!
//! This module deliberately supervises the existing `cas hub serve` entry
//! point. The hub itself retains ownership of `process.json`, `hub.lock`,
//! identity, auth state, and Tailscale Serve receipts; a unit/plist contains
//! only an absolute executable path and non-secret listener arguments.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{Cli, hub::HubServiceCommands};
use crate::hub::{
    DEFAULT_HUB_PORT, HubLockHolder, HubLockOwner, HubProcessRecord, HubRuntimePaths,
};

use crate::hub::observation::{LAUNCHD_LABEL, LAUNCHD_TEST_LABEL_ENV, launchd_label};
const LAUNCHD_TEST_PORT_ENV: &str = "CAS_HUB_SERVICE_PORT";
const LAUNCHD_CLI_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
const SYSTEMD_UNIT: &str = "cas-hub.service";
const SYSTEMCTL_PATH_ENV: &str = "CAS_HUB_SYSTEMCTL";
const MANAGER_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const MANAGER_TIMEOUT_WARNING: &str =
    "service manager unknown (timed out). Retry: cas hub status";
pub(super) const MANAGER_UNAVAILABLE_WARNING: &str =
    "service manager unknown (unavailable). Retry: cas hub status";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagerProbe {
    Observed(bool),
    TimedOut,
    Unavailable,
}

impl ManagerProbe {
    fn observed(self) -> Option<bool> {
        match self {
            Self::Observed(active) => Some(active),
            Self::TimedOut | Self::Unavailable => None,
        }
    }

    fn warning(self) -> Option<&'static str> {
        match self {
            Self::Observed(_) => None,
            Self::TimedOut => Some(MANAGER_TIMEOUT_WARNING),
            Self::Unavailable => Some(MANAGER_UNAVAILABLE_WARNING),
        }
    }

    fn require_observed(self) -> Result<bool> {
        self.observed().ok_or_else(|| {
            anyhow::anyhow!(
                "{}; refusing to change hub service ownership",
                self.warning().unwrap_or(MANAGER_UNAVAILABLE_WARNING)
            )
        })
    }
}
/// Signals that systemd classes as a clean exit, and so never restarts under
/// `Restart=on-failure`, but that are never a deliberate stop of the hub.
/// SIGTERM is left out on purpose: `systemctl stop` and `cas hub stop` both
/// deliver it and both mean "stay stopped". A hub killed by SIGPIPE on
/// soundwave (2026-09-26) sat dead for hours because of this gap (cas-621ec).
const SYSTEMD_FORCE_RESTART_SIGNALS: &str = "SIGHUP SIGINT SIGPIPE";
/// Open-file limit for the supervised hub. launchd's default soft limit of
/// 256 let Commander bursts exhaust it (EMFILE on accept, cas-6cc6).
const HUB_FILE_LIMIT: u64 = 8192;
// Both findings below point at `cas hub restart`: it stops a detached hub if
// one is running and (re)starts the installed service.
pub(crate) const INACTIVE_DETACHED_HUB_WARNING: &str = "service installed but inactive, detached hub running without restart supervision. Fix: run `cas hub restart` to hand the hub back to the service";
pub(crate) const INACTIVE_SERVICE_NO_HUB_ERROR: &str = "service installed but inactive, and no hub is running. Fix: run `cas hub restart` to start the hub under the service";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServicePlatform {
    Launchd,
    Systemd,
    ManualLinux,
    Unsupported,
}

#[derive(Debug, Serialize)]
struct ServiceReport {
    platform: &'static str,
    supervision: &'static str,
    installed: bool,
    enabled: Option<bool>,
    active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manager_warning: Option<&'static str>,
    unit_path: Option<String>,
    log_path: Option<String>,
    hub_running: bool,
    hub_state: &'static str,
    launch_readiness: Vec<LaunchReadiness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct LaunchReadiness {
    cli: &'static str,
    profile: String,
    ready: bool,
    detail: String,
    #[serde(skip)]
    summary: String,
    #[serde(skip)]
    profile_path: Option<PathBuf>,
    #[serde(skip)]
    repair: Option<String>,
    #[serde(skip)]
    cause: Option<String>,
}

impl ServiceReport {
    fn after_start(mut self) -> Self {
        if !self.hub_running {
            self.hub_state = "starting";
        }
        self
    }
}

fn launch_readiness() -> Vec<LaunchReadiness> {
    use cas_mux::SupervisorCli;
    crate::hub::launch_env::readiness()
        .into_iter()
        .map(|(cli, result)| {
            let provider = match cli {
                "claude" => SupervisorCli::Claude,
                "codex" => SupervisorCli::Codex,
                _ => SupervisorCli::Grok,
            };
            let profile = crate::hub::launch_env::default_profile_name(provider)
                .unwrap_or_else(|_| "unknown".into());
            match result {
                Ok(path) => LaunchReadiness {
                    cli,
                    profile,
                    ready: true,
                    detail: path.display().to_string(),
                    summary: String::new(),
                    profile_path: None,
                    repair: None,
                    cause: None,
                },
                Err(error) => {
                    use crate::hub::launch_env::LaunchError;
                    let detail = error.to_string();
                    let (summary, profile_path, repair, cause) = match error {
                        LaunchError::MissingBinary { cli } => (
                            "CLI missing".to_owned(),
                            None,
                            Some(install_cli_command(cli).to_owned()),
                            None,
                        ),
                        LaunchError::ProfileMissing { cli, path } => (
                            "profile missing".to_owned(),
                            Some(path),
                            Some(format!("cas {cli} login {profile}")),
                            None,
                        ),
                        LaunchError::NotLoggedIn { cli, profile } => (
                            "profile not logged in".to_owned(),
                            None,
                            Some(format!("cas {cli} login {profile}")),
                            None,
                        ),
                        LaunchError::ProbeFailed { detail, .. } =>
                            ("launch check failed".to_owned(), None, None, Some(detail)),
                    };
                    LaunchReadiness {
                        cli,
                        profile,
                        ready: false,
                        detail,
                        summary,
                        profile_path,
                        repair,
                        cause,
                    }
                }
            }
        })
        .collect()
}

fn install_cli_command(cli: &str) -> &'static str {
    match cli {
        "claude" => "npm install -g @anthropic-ai/claude-code",
        "codex" => "npm install -g @openai/codex",
        "grok" => "curl -fsSL https://x.ai/cli/install.sh | bash",
        _ => "Install the CLI and add it to PATH",
    }
}

fn readiness_path(path: &Path, full: bool, width: usize) -> (String, bool) {
    let display = if full {
        path.display().to_string()
    } else {
        crate::cli::update::findings::home_relative(path)
    };
    let available = width.saturating_sub("    path: ".len()).max(1);
    if full || display_cells(&display) <= available {
        return (display, false);
    }

    let ellipsis = if crate::ui::components::formatter::locale_is_utf8() {
        "…"
    } else {
        "..."
    };
    let suffix = trailing_path_cells(&display, available.saturating_sub(display_cells(ellipsis) + 1));
    (format!("{ellipsis}/{suffix}"), true)
}

fn display_cells(value: &str) -> usize {
    value
        .chars()
        .map(|character| if character.is_ascii() { 1 } else { 2 })
        .sum()
}

fn fit_cause(cause: &str, width: usize) -> String {
    let mut result = String::new();
    for character in cause.chars() {
        if display_cells(&result) + display_cells(&character.to_string()) > width.saturating_sub(1) {
            result.push('…');
            return result;
        }
        result.push(character);
    }
    result
}

fn trailing_path_cells(value: &str, limit: usize) -> String {
    let mut suffix = Vec::new();
    let mut cells = 0;
    for character in value.chars().rev() {
        let width = if character.is_ascii() { 1 } else { 2 };
        if cells + width > limit {
            break;
        }
        cells += width;
        suffix.push(character);
    }
    suffix.reverse();
    suffix.into_iter().collect::<String>().trim_start_matches('/').to_owned()
}

fn capture_launch_profiles(cli: &Cli) -> Result<()> {
    use cas_mux::SupervisorCli;
    let home = home_dir()?;
    let root = crate::store::known_repos::host_cas_dir();
    let mut config = crate::config::Config::load(&root)?;
    let profiles = config
        .hub
        .get_or_insert_with(Default::default)
        .launch_profiles
        .get_or_insert_with(Default::default);
    let mut recorded = Vec::new();
    for (cli, key, slot) in [
        (
            SupervisorCli::Claude,
            "CLAUDE_CONFIG_DIR",
            &mut profiles.claude,
        ),
        (SupervisorCli::Codex, "CODEX_HOME", &mut profiles.codex),
    ] {
        if slot.is_some() {
            continue;
        }
        if let Some(dir) = std::env::var_os(key) {
            if let Some(name) = crate::hub::launch_env::captured_profile(
                slot.as_deref(),
                cli,
                &home,
                Path::new(&dir),
            ) {
                recorded.push(format!("Hub launch profile recorded: {key} → {name}"));
                *slot = Some(name);
            }
        }
    }
    if !recorded.is_empty() {
        config.save(&root)?;
        if !cli.json {
            for line in recorded {
                eprintln!("{line}");
            }
        }
    }
    Ok(())
}

fn capture_launch_profiles_warning(result: Result<()>) -> Option<String> {
    result
        .err()
        .map(|error| format!("Hub launch profile not recorded: {error:#}"))
}

/// Profile capture never writes to stdout: `cas update --json` reserves it for JSON.
fn capture_launch_profiles_best_effort(cli: &Cli) {
    if let Some(warning) = capture_launch_profiles_warning(capture_launch_profiles(cli)) {
        tracing::warn!("{warning}");
        eprintln!("{warning}");
    }
}

pub(super) fn manage_service(
    command: &HubServiceCommands,
    cli: &Cli,
    tailscale_serve: bool,
    tailscale_port: u16,
) -> Result<()> {
    let platform = native_platform();
    match command {
        HubServiceCommands::Install(args) => {
            install(platform, cli, tailscale_serve, tailscale_port, args.dry_run)
        }
        HubServiceCommands::Status => status(platform, cli, false),
        HubServiceCommands::Uninstall => uninstall(platform, cli),
    }
}

fn native_platform() -> ServicePlatform {
    #[cfg(target_os = "macos")]
    {
        ServicePlatform::Launchd
    }
    #[cfg(target_os = "linux")]
    {
        match probe_manager("systemctl", ["--user", "--version"]) {
            // A timeout is not evidence that systemd is absent. Diagnostic
            // callers must retain the unknown outcome of the actual probe.
            ManagerProbe::Observed(true) | ManagerProbe::TimedOut => ServicePlatform::Systemd,
            ManagerProbe::Observed(false) | ManagerProbe::Unavailable => {
                ServicePlatform::ManualLinux
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        ServicePlatform::Unsupported
    }
}

fn install(
    platform: ServicePlatform,
    cli: &Cli,
    tailscale_serve: bool,
    tailscale_port: u16,
    dry_run: bool,
) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    if !dry_run {
        capture_launch_profiles_best_effort(cli);
    }
    match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let binary = service_binary(dry_run)?;
            let definition =
                launchd_plist(&binary, &paths.log_path(), tailscale_serve, tailscale_port);
            if dry_run {
                return print_dry_run(
                    cli,
                    platform,
                    &path,
                    &definition,
                    &launchd_preview_actions(&path),
                );
            }
            crate::hub::ensure_private_dir(paths.root())?;
            write_service_file(&path, &definition)?;
            let domain = launchd_domain()?;
            // bootstrap is idempotent only after the previous service has been
            // removed from the bootstrap namespace.
            let _ = Command::new("launchctl")
                .args(launchd_bootout_args(&domain, &path))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            run_manager_vec("launchctl", &launchd_bootstrap_args(&domain, &path))?;
            run_manager_vec("launchctl", &launchd_kickstart_args(&domain))?;
            status(platform, cli, true)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            let binary = service_binary(dry_run)?;
            let definition = systemd_unit(&binary, tailscale_serve, tailscale_port);
            if dry_run {
                return print_dry_run(
                    cli,
                    platform,
                    &path,
                    &definition,
                    &systemd_preview_actions(),
                );
            }
            crate::hub::ensure_private_dir(paths.root())?;
            write_service_file(&path, &definition)?;
            // A user service only survives logout/reboot when lingering is
            // enabled. Do this before activation so a partial install never
            // advertises reboot persistence that it does not have.
            let user = current_user()?;
            run_manager("loginctl", ["enable-linger", &user], None)?;
            run_manager("systemctl", ["--user", "daemon-reload"], None)?;
            run_manager(
                "systemctl",
                ["--user", "enable", "--now", SYSTEMD_UNIT],
                None,
            )?;
            status(platform, cli, true)
        }
        ServicePlatform::ManualLinux => print_report(
            cli,
            report(
                ServicePlatform::ManualLinux,
                false,
                None,
                None,
                Some(manual_linux_instructions()),
            )?,
        ),
        ServicePlatform::Unsupported => {
            anyhow::bail!("hub service management is supported on macOS and Linux only")
        }
    }
}

/// Restart a hub that is owned by a user-level service manager without racing
/// its KeepAlive/restart policy. Returns `true` when the manager handled the
/// restart, leaving normal stop/start lifecycle code for manually launched
/// hubs.
pub(super) fn restart_supervised(
    cli: &Cli,
    tailscale_serve: bool,
    tailscale_port: u16,
) -> Result<bool> {
    match native_platform() {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            if !path.is_file() {
                return Ok(false);
            }
            let domain = launchd_domain()?;
            let active = probe_manager(
                "launchctl",
                ["print", &format!("{domain}/{}", launchd_label())],
            )
            .require_observed()?;
            capture_launch_profiles_best_effort(cli);
            let definition = fs::read_to_string(&path)?;
            let rewritten =
                rewrite_launchd_publication_flags(&definition, tailscale_serve, tailscale_port)?;
            let paths = HubRuntimePaths::default_for_user()?;
            stop_detached_hub_if_present(cli, &paths, active)?;
            let previous_pid = paths.read_process_record().ok().map(|record| record.pid);
            // update may already have rewritten the file. Always reload the
            // job so launchd cannot keep cached ProgramArguments from before it.
            if active {
                run_manager_vec("launchctl", &launchd_bootout_args(&domain, &path))?;
            }
            if rewritten != definition {
                write_service_file(&path, &rewritten)?;
            }
            run_manager_vec("launchctl", &launchd_bootstrap_args(&domain, &path))?;
            run_manager_vec("launchctl", &launchd_kickstart_args(&domain))?;
            wait_for_supervised_hub(previous_pid, tailscale_serve)?;
            Ok(true)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            if !path.is_file() {
                return Ok(false);
            }
            let active = probe_manager(
                "systemctl",
                ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
            )
            .require_observed()?;
            capture_launch_profiles_best_effort(cli);
            repair_systemd_publication_flags(&path, tailscale_serve, tailscale_port)?;
            refresh_systemd_unit(&path)?;
            let paths = HubRuntimePaths::default_for_user()?;
            stop_detached_hub_if_present(cli, &paths, active)?;
            let previous_pid = paths.read_process_record().ok().map(|record| record.pid);
            run_manager("systemctl", ["--user", "restart", SYSTEMD_UNIT], None)?;
            wait_for_supervised_hub(previous_pid, tailscale_serve)?;
            Ok(true)
        }
        ServicePlatform::ManualLinux => {
            if systemd_path()?.is_file() {
                anyhow::bail!(
                    "cas hub service is installed but the systemd user manager is unavailable; refusing to launch a detached hub"
                );
            }
            Ok(false)
        }
        ServicePlatform::Unsupported => Ok(false),
    }
}

fn stop_detached_hub_if_present(
    cli: &Cli,
    paths: &HubRuntimePaths,
    manager_active: bool,
) -> Result<()> {
    let holders = paths.lock_holders();
    if holders.is_empty() {
        if let Ok(record) = paths.read_process_record()
            && (!manager_active || record.launched_by.as_deref() != Some("service"))
            && super::hub::record_is_live(&record)
        {
            super::hub::stop_for_service(cli, false)?;
        }
        if !manager_active {
            ensure!(
                std::net::TcpListener::bind(("127.0.0.1", service_port())).is_ok(),
                "hub service port {} is occupied after detached hub takeover; refusing to bootstrap a KeepAlive service. Run `cas hub status --json`, then `cas hub stop --force` and retry `cas hub restart`",
                service_port()
            );
        }
        return Ok(());
    }
    let record = paths.read_process_record().ok();
    let record_live = record.as_ref().is_some_and(super::hub::record_is_live);
    let decision = if holders.len() == 1 {
        detached_hub_takeover(record.as_ref(), &holders[0], manager_active)
    } else {
        DetachedTakeover::Refuse
    };
    match decision {
        DetachedTakeover::Stop => {
            super::hub::stop_for_service(cli, !record_live)?;
            ensure!(
                paths.lock_holders().is_empty(),
                "detached hub still holds the hub lock; refusing to start a supervised hub"
            );
        }
        DetachedTakeover::Refuse => anyhow::bail!(
            "cannot transfer hub ownership to the service; lock holders: {}. Run `cas hub status --json` to inspect them, then `cas hub stop --force` and retry `cas hub restart`",
            describe_holders(&holders)
        ),
        DetachedTakeover::None => {}
    }
    Ok(())
}

fn rewrite_launchd_publication_flags(
    definition: &str,
    enabled: bool,
    tailscale_port: u16,
) -> Result<String> {
    let key = "<key>ProgramArguments</key>";
    let key_start = definition
        .find(key)
        .context("launchd plist has no ProgramArguments")?;
    let array_start = definition[key_start + key.len()..]
        .find("<array>")
        .map(|offset| key_start + key.len() + offset + "<array>".len())
        .context("launchd plist has no ProgramArguments array")?;
    let array_end = definition[array_start..]
        .find("</array>")
        .map(|offset| array_start + offset)
        .context("launchd plist has an unclosed ProgramArguments array")?;
    let mut args = Vec::new();
    let mut remaining = definition[array_start..array_end].trim();
    while !remaining.is_empty() {
        let content = remaining
            .strip_prefix("<string>")
            .context("launchd ProgramArguments contains a non-string element")?;
        let end = content
            .find("</string>")
            .context("unclosed launchd argument")?;
        args.push(quick_xml::escape::unescape(&content[..end])?.into_owned());
        remaining = content[end + "</string>".len()..].trim_start();
    }
    ensure!(
        args.first()
            .is_some_and(|binary| Path::new(binary).is_absolute()),
        "launchd ProgramArguments must start with an absolute binary path"
    );
    let mut rewritten_args = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--tailscale-serve" || arg == "--no-tailscale-serve" {
            continue;
        }
        if arg == "--tailscale-serve-port" {
            ensure!(
                iter.next().is_some(),
                "launchd plist has a missing Tailscale port"
            );
            continue;
        }
        rewritten_args.push(arg);
    }
    if enabled {
        rewritten_args.extend([
            "--tailscale-serve".to_owned(),
            "--tailscale-serve-port".to_owned(),
            tailscale_port.to_string(),
        ]);
    } else {
        rewritten_args.push("--no-tailscale-serve".to_owned());
    }
    let array = rewritten_args
        .iter()
        .map(|arg| format!("\n    <string>{}</string>", xml_escape(arg)))
        .collect::<String>();
    let rewritten = format!(
        "{}{}\n  {}",
        &definition[..array_start],
        array,
        &definition[array_end..]
    );
    ensure_launchd_file_limits(&ensure_launchd_cli_environment(&rewritten)?)
}

fn launchd_tailscale_environment() -> [(&'static str, Option<std::ffi::OsString>); 3] {
    [
        ("TAILSCALE_BE_CLI", Some("1".into())),
        ("TAILSCALE", std::env::var_os("TAILSCALE").filter(|value| !value.is_empty())),
        ("TAILSCALE_SOCKET", std::env::var_os("TAILSCALE_SOCKET")),
    ]
}

fn ensure_launchd_cli_environment(definition: &str) -> Result<String> {
    if let Some(key_start) = definition.find("<key>EnvironmentVariables</key>") {
        let after_key = key_start + "<key>EnvironmentVariables</key>".len();
        let value_start = definition.len() - definition[after_key..].trim_start().len();
        if definition[value_start..].starts_with("<dict/>") {
            return ensure_launchd_cli_environment(&format!(
                "{}<dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n  </dict>{}",
                &definition[..value_start],
                &definition[value_start + "<dict/>".len()..]
            ));
        }
        ensure!(
            definition[value_start..].starts_with("<dict>"),
            "launchd EnvironmentVariables has no dict"
        );
        let dict_start = value_start + "<dict>".len();
        let dict_end = definition[dict_start..]
            .find("</dict>")
            .map(|offset| dict_start + offset)
            .context("launchd EnvironmentVariables has an unclosed dict")?;
        let has_path = definition[dict_start..dict_end].contains("<key>PATH</key>");
        let has_term = definition[dict_start..dict_end].contains("<key>TERM</key>");
        let insert_at = definition[..dict_end]
            .rfind('\n')
            .map_or(dict_end, |newline| newline + 1);
        let mut additions = String::new();
        if !has_path {
            additions.push_str(&format!(
                "    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n"
            ));
        }
        if !has_term {
            additions.push_str("    <key>TERM</key>\n    <string>dumb</string>\n");
        }
        for (key, value) in launchd_tailscale_environment() {
            let Some(value) = value else { continue };
            let escaped = xml_escape(&value.to_string_lossy());
            let key_tag = format!("<key>{key}</key>");
            if let Some(offset) = definition[dict_start..dict_end].find(&key_tag) {
                let after_key = dict_start + offset + key_tag.len();
                let string_start = definition.len() - definition[after_key..].trim_start().len();
                let value_start = string_start + "<string>".len();
                ensure!(definition[string_start..].starts_with("<string>"),
                    "launchd {key} must be a string");
                let value_end = definition[value_start..dict_end].find("</string>")
                    .map(|offset| value_start + offset).context("unclosed launchd environment string")?;
                if definition[value_start..value_end] != escaped {
                    let mut updated = definition.to_owned();
                    updated.replace_range(value_start..value_end, &escaped);
                    return ensure_launchd_cli_environment(&updated);
                }
            } else {
                additions.push_str(&format!("    <key>{key}</key>\n    <string>{escaped}</string>\n"));
            }
        }
        if additions.is_empty() {
            return Ok(definition.to_owned());
        }
        return Ok(format!(
            "{}{}{}",
            &definition[..insert_at],
            additions,
            &definition[insert_at..]
        ));
    }
    ensure!(
        definition.contains("  <key>StandardOutPath</key>"),
        "launchd plist has no StandardOutPath after ProgramArguments"
    );
    ensure_launchd_cli_environment(&definition.replacen(
        "  <key>StandardOutPath</key>",
        &format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n  </dict>\n  <key>StandardOutPath</key>"
        ),
        1,
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum DetachedTakeover {
    None,
    Stop,
    Refuse,
}

fn detached_hub_takeover(
    record: Option<&HubProcessRecord>,
    holder: &HubLockHolder,
    manager_active: bool,
) -> DetachedTakeover {
    let matching_record = record.filter(|record| record.pid == holder.pid);
    let recognized_command = holder.command.as_deref().is_some_and(|command| {
        command.contains("cas hub serve") || command.contains("/cas hub serve")
    });
    if matching_record.is_some_and(|record| record.launched_by.as_deref() == Some("service"))
        && manager_active
    {
        return DetachedTakeover::None;
    }
    if matching_record.is_none() && !recognized_command {
        return DetachedTakeover::Refuse;
    }
    DetachedTakeover::Stop
}

fn describe_holders(holders: &[HubLockHolder]) -> String {
    holders
        .iter()
        .map(|holder| {
            format!(
                "pid {} (phase {}, age {}, command {})",
                holder.pid,
                holder.phase.as_deref().unwrap_or("unknown"),
                holder.age_label(),
                holder.command.as_deref().unwrap_or("unknown")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Preserve quoted tokens verbatim: decoding/re-escaping would also change
/// systemd specifiers, environment expansions and quoted executable paths.
fn systemd_argument_tokens(command: &str) -> Result<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (offset, byte) in command.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte.is_ascii_whitespace() && quote.is_none() {
            if let Some(start) = start.take() {
                tokens.push(&command[start..offset]);
            }
            continue;
        }
        start.get_or_insert(offset);
        if byte == b'\\' {
            escaped = true;
        } else if Some(byte) == quote {
            quote = None;
        } else if quote.is_none() && matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        }
    }
    ensure!(
        quote.is_none() && !escaped,
        "hub service ExecStart has an incomplete quote or escape"
    );
    if let Some(start) = start {
        tokens.push(&command[start..]);
    }
    Ok(tokens)
}

/// Amend only publication arguments; preserve the executable, listener,
/// environment and operator unit customizations, including quoted values.
fn rewrite_systemd_publication_flags(
    definition: &str,
    enabled: bool,
    tailscale_port: u16,
) -> Result<String> {
    let mut found = false;
    let mut lines = Vec::new();
    for line in definition.lines() {
        let Some(command) = line.strip_prefix("ExecStart=") else {
            lines.push(line.to_owned());
            continue;
        };
        let mut args = Vec::new();
        let tokens = systemd_argument_tokens(command)?;
        let mut iter = tokens.into_iter();
        while let Some(arg) = iter.next() {
            if matches!(arg, "--tailscale-serve" | "--no-tailscale-serve") {
                continue;
            }
            if arg == "--tailscale-serve-port" {
                ensure!(
                    iter.next().is_some(),
                    "systemd unit has a missing Tailscale port"
                );
                continue;
            }
            args.push(arg.to_owned());
        }
        ensure!(!args.is_empty(), "systemd ExecStart is empty");
        if enabled {
            args.extend([
                "--tailscale-serve".to_owned(),
                "--tailscale-serve-port".to_owned(),
                tailscale_port.to_string(),
            ]);
        } else {
            args.push("--no-tailscale-serve".to_owned());
        }
        lines.push(format!("ExecStart={}", args.join(" ")));
        found = true;
    }
    ensure!(found, "installed hub service has no ExecStart");
    let mut rewritten = lines.join("\n");
    if definition.ends_with('\n') {
        rewritten.push('\n');
    }
    Ok(rewritten)
}

fn repair_systemd_publication_flags(
    path: &Path,
    enabled: bool,
    tailscale_port: u16,
) -> Result<bool> {
    let definition = fs::read_to_string(path)?;
    let rewritten = rewrite_systemd_publication_flags(&definition, enabled, tailscale_port)?;
    if rewritten == definition {
        return Ok(false);
    }
    write_service_file(path, &rewritten)?;
    run_manager("systemctl", ["--user", "daemon-reload"], None)?;
    Ok(true)
}

fn service_https_port(definition: &str) -> u16 {
    let normalized = definition
        .replace("<string>", " ")
        .replace("</string>", " ");
    let mut args = normalized.split_whitespace();
    while let Some(arg) = args.next() {
        if arg == "--tailscale-serve-port" {
            return args
                .next()
                .and_then(|port| port.parse().ok())
                .unwrap_or(443);
        }
    }
    443
}

/// Bring an installed unit's restart policy up to the current template
/// (cas-621ec) without touching anything else in it: binary, ports,
/// publication flags and any operator edits survive. It also adds the hub's
/// `LimitNOFILE` when absent (cas-6cc6). Returns `None` when the unit already
/// carries both, or has no `[Service]` section to amend.
fn upgrade_systemd_restart_policy(definition: &str) -> Option<String> {
    let desired = format!("RestartForceExitStatus={SYSTEMD_FORCE_RESTART_SIGNALS}");
    let has_policy = definition.lines().any(|line| line.trim() == desired);
    // An operator's own LimitNOFILE is kept; only its absence is stale (cas-6cc6).
    let has_limit = definition
        .lines()
        .any(|line| line.trim_start().starts_with("LimitNOFILE="));
    if has_policy && has_limit {
        return None;
    }
    let mut lines = Vec::new();
    let mut inserted = has_policy;
    for line in definition.lines() {
        // Replace, rather than duplicate, a stale force-restart list.
        if !has_policy && line.trim_start().starts_with("RestartForceExitStatus=") {
            continue;
        }
        lines.push(line.to_owned());
        if !inserted && line.trim_start().starts_with("Restart=") {
            lines.push(desired.clone());
            inserted = true;
        }
    }
    if !inserted {
        let service = lines.iter().position(|line| line.trim() == "[Service]")?;
        lines.insert(service + 1, desired.clone());
        lines.insert(service + 1, "Restart=on-failure".to_owned());
    }
    if !has_limit {
        // Before the log lines, as in the template, so operator lines after
        // RestartSec stay adjacent.
        let at = lines
            .iter()
            .position(|line| line.trim_start().starts_with("StandardOutput="))
            .or_else(|| {
                lines
                    .iter()
                    .position(|line| line.trim() == desired)
                    .map(|index| index + 1)
            })?;
        lines.insert(at, format!("LimitNOFILE={HUB_FILE_LIMIT}"));
    }
    let mut upgraded = lines.join("\n");
    if definition.ends_with('\n') {
        upgraded.push('\n');
    }
    Some(upgraded)
}

/// Rewrite an installed systemd unit whose restart policy predates the
/// current template. `daemon-reload` applies the new policy to the running
/// service without restarting it. Returns whether the unit changed.
fn refresh_systemd_unit(path: &Path) -> Result<bool> {
    let Some(upgraded) = upgrade_systemd_restart_policy(&fs::read_to_string(path)?) else {
        return Ok(false);
    };
    write_service_file(path, &upgraded)?;
    run_manager("systemctl", ["--user", "daemon-reload"], None)?;
    Ok(true)
}

/// Refresh publication policy even when the old process was loopback-only.
/// Return true when a manager reload/restart is needed before early verification.
pub(crate) fn refresh_installed_service(tailscale_serve: bool) -> Result<bool> {
    let platform = native_platform();
    let path = match platform {
        ServicePlatform::Launchd => launchd_path()?,
        ServicePlatform::Systemd => systemd_path()?,
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => return Ok(false),
    };
    refresh_installed_service_at(platform, &path, tailscale_serve, &std::env::current_exe()?)
}

fn refresh_installed_service_at(
    platform: ServicePlatform,
    path: &Path,
    tailscale_serve: bool,
    installed_binary: &Path,
) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let definition = fs::read_to_string(path)?;
    require_installed_service_binary(platform, &definition, installed_binary)?;
    match platform {
        ServicePlatform::Systemd => {
            let port = service_https_port(&fs::read_to_string(path)?);
            let publication_changed =
                repair_systemd_publication_flags(path, tailscale_serve, port)?;
            Ok(refresh_systemd_unit(path)? || publication_changed)
        }
        ServicePlatform::Launchd => {
            let definition = fs::read_to_string(path)?;
            let rewritten = rewrite_launchd_publication_flags(
                &definition,
                tailscale_serve,
                service_https_port(&definition),
            )?;
            if rewritten == definition {
                return Ok(false);
            }
            write_service_file(path, &rewritten)?;
            Ok(true)
        }
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => Ok(false),
    }
}

/// A service restart reuses the manager's executable, not the updating CLI.
/// Refuse before rewriting or reloading anything when those executables differ.
fn require_installed_service_binary(
    platform: ServicePlatform,
    definition: &str,
    installed_binary: &Path,
) -> Result<()> {
    let configured = match platform {
        ServicePlatform::Launchd => {
            let argument = definition
                .split_once("<key>ProgramArguments</key>")
                .and_then(|(_, tail)| tail.split_once("<array>"))
                .and_then(|(_, tail)| tail.trim_start().strip_prefix("<string>"))
                .and_then(|tail| tail.split_once("</string>"))
                .map(|(binary, _)| binary)
                .context("launchd plist has no ProgramArguments[0]")?;
            PathBuf::from(quick_xml::escape::unescape(argument)?.as_ref())
        }
        ServicePlatform::Systemd => {
            let command = definition
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("ExecStart="))
                .context("systemd unit has no ExecStart")?;
            let args = shell_words::split(command).context("cannot parse systemd ExecStart")?;
            PathBuf::from(args.first().context("systemd ExecStart has no binary")?)
        }
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => return Ok(()),
    };
    // Linux can run this comparison in the pre-swap image. /proc/self/exe
    // then names the old inode as "<install path> (deleted)".
    let stable_binary = (platform == ServicePlatform::Systemd)
        .then(|| super::update::strip_deleted_suffix(installed_binary.to_owned()));
    let installed_binary = stable_binary.as_deref().unwrap_or(installed_binary);
    let resolved_configured = fs::canonicalize(&configured).unwrap_or_else(|_| configured.clone());
    let resolved_installed =
        fs::canonicalize(installed_binary).unwrap_or_else(|_| installed_binary.to_owned());
    ensure!(
        configured.is_absolute() && resolved_configured == resolved_installed,
        "hub service binary drift; refusing to restart\n  configured: {}\n  installed: {}\n  Fix: cas hub service install",
        configured.display(),
        installed_binary.display()
    );
    Ok(())
}

pub(super) fn installed_https_port() -> Result<Option<u16>> {
    let path = match native_platform() {
        ServicePlatform::Launchd => launchd_path()?,
        ServicePlatform::Systemd | ServicePlatform::ManualLinux => systemd_path()?,
        ServicePlatform::Unsupported => return Ok(None),
    };
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(service_https_port(&fs::read_to_string(path)?)))
}

pub(super) fn installed_service_exists() -> Result<bool> {
    match native_platform() {
        ServicePlatform::Launchd => Ok(launchd_path()?.is_file()),
        ServicePlatform::Systemd | ServicePlatform::ManualLinux => Ok(systemd_path()?.is_file()),
        ServicePlatform::Unsupported => Ok(false),
    }
}

/// One end-state budget shared by launchd unload, process exit and Serve teardown.
pub(super) fn stop_deadline() -> crate::bounded_process::Deadline {
    crate::bounded_process::Deadline::after(Duration::from_secs(15))
}

/// Unload before signaling a launchd-owned hub: KeepAlive must no longer
/// own a restart while the ordinary stop path waits for process teardown.
/// Keep the plist so an explicit restart can bootstrap it again.
pub(super) fn stop_supervised(deadline: crate::bounded_process::Deadline) -> Result<()> {
    if native_platform() != ServicePlatform::Launchd || !launchd_path()?.is_file() {
        return Ok(());
    }
    let target = format!("{}/{}", launchd_domain()?, launchd_label());
    if !probe_manager_with_timeout("launchctl", ["print", &target], deadline.remaining()).require_observed()? {
        return Ok(());
    }
    let mut command = manager_command("launchctl");
    command.args(["bootout", &target]);
    let output = crate::bounded_process::run_command(
        &mut command, deadline, deadline.remaining(),
    ).map_err(|_| anyhow::anyhow!("launchctl bootout did not complete; refusing to race hub supervision"))?;
    ensure!(output.status.success(), "launchctl refused to unload the Cassy hub service");
    wait_for_manager_unload(deadline, |remaining| {
        probe_manager_with_timeout("launchctl", ["print", &target], remaining)
    })
}

fn wait_for_manager_unload(
    deadline: crate::bounded_process::Deadline,
    mut probe: impl FnMut(Duration) -> ManagerProbe,
) -> Result<()> {
    loop {
        let remaining = deadline.remaining();
        ensure!(!remaining.is_zero(), "Cassy hub service is still loaded after launchctl bootout");
        if !probe(remaining).require_observed()? {
            return Ok(());
        }
        // A successful bootout can precede removal from launchd's namespace.
        std::thread::sleep(Duration::from_millis(25).min(deadline.remaining()));
    }
}

#[cfg(test)]
fn systemd_service_binary(path: &Path) -> Result<PathBuf> {
    let command = fs::read_to_string(path)?
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .and_then(|line| line.split_whitespace().next())
        .filter(|binary| !binary.is_empty())
        .map(PathBuf::from)
        .context("installed cas hub service has no ExecStart binary")?;
    ensure!(
        command.is_absolute(),
        "installed cas hub service ExecStart must use an absolute binary path"
    );
    Ok(command)
}

/// Return the operator-facing error used by `cas hub status` and `cas doctor`
/// when an installed service is inactive: bypassed by a detached hub, or with
/// no hub running at all. Both messages name the fix command.
pub(super) fn inactive_detached_warning(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
) -> Result<Option<&'static str>> {
    inactive_detached_warning_on(native_platform(), paths, record)
}

fn inactive_detached_warning_on(
    platform: ServicePlatform,
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
) -> Result<Option<&'static str>> {
    let (installed, active) = match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let domain = launchd_domain()?;
            (
                path.is_file(),
                probe_manager(
                    "launchctl",
                    ["print", &format!("{domain}/{}", launchd_label())],
                ),
            )
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            (
                path.is_file(),
                probe_manager(
                    "systemctl",
                    ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
                ),
            )
        }
        ServicePlatform::ManualLinux => {
            let installed = systemd_path()?.is_file();
            // Manual supervision without an installed systemd unit remains
            // supported. An installed unit needs an observable manager.
            if !installed {
                return Ok(None);
            }
            (installed, ManagerProbe::Unavailable)
        }
        ServicePlatform::Unsupported => return Ok(None),
    };
    // A detached hub needs no service manager when no service is installed.
    if !installed {
        return Ok(None);
    }
    // A hung diagnostic cannot establish either service health or inactivity.
    if let Some(warning) = active.warning() {
        return Ok(Some(warning));
    }
    let hub_live = match record {
        Some(record) => {
            super::hub::record_is_live(record)
                && paths
                    .read_lock_owner()
                    .is_some_and(|owner| lock_owner_is_active(&owner) && owner.pid == record.pid)
        }
        None => paths
            .lock_holders()
            .into_iter()
            .any(|holder| holder.phase.as_deref() != Some("stopping")),
    };
    Ok(inactive_detached_warning_for(
        installed,
        active.observed(),
        hub_live,
        record.and_then(|record| record.launched_by.as_deref()),
    ))
}

fn lock_owner_is_active(owner: &HubLockOwner) -> bool {
    owner.phase != "stopping"
}

pub(crate) fn doctor_warning() -> Result<Option<String>> {
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record().ok();
    Ok(inactive_detached_warning(&paths, record.as_ref())?.map(str::to_owned))
}

pub(crate) fn inactive_detached_warning_for(
    installed: bool,
    active: Option<bool>,
    hub_live: bool,
    launched_by: Option<&str>,
) -> Option<&'static str> {
    if !installed || active != Some(false) {
        return None;
    }
    if !hub_live {
        // cas-621ec: an installed service that is not running leaves nothing
        // to restart the hub on the next crash or reboot, so it is reported
        // even when no detached hub is covering for it.
        return Some(INACTIVE_SERVICE_NO_HUB_ERROR);
    }
    (launched_by != Some("service")).then_some(INACTIVE_DETACHED_HUB_WARNING)
}

#[cfg(test)]
fn service_file_requests_tailscale(path: &Path) -> Result<bool> {
    Ok(fs::read_to_string(path)?.contains("--tailscale-serve"))
}

fn wait_for_supervised_hub(previous_pid: Option<u32>, tailscale_serve: bool) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    // Publication can make six bounded CLI calls before the hub writes its
    // ready record. Match the detached Serve startup budget.
    let timeout = if tailscale_serve { 65 } else { 10 };
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        if let Ok(record) = paths.read_process_record()
            && previous_pid != Some(record.pid)
            && super::hub::record_is_live(&record)
            && paths
                .read_lock_owner()
                .is_some_and(|owner| owner.pid == record.pid && owner.phase == "running")
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "supervised cas hub did not become ready after {timeout}.0s; inspect `cas hub service status` and `{}`",
                paths.log_path().display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn status(platform: ServicePlatform, cli: &Cli, just_started: bool) -> Result<()> {
    let finish = |report: ServiceReport| {
        print_report(
            cli,
            if just_started {
                report.after_start()
            } else {
                report
            },
        )
    };
    match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let installed = path.exists();
            let active = Some(
                launchd_domain()
                    .map(|domain| {
                        probe_manager(
                            "launchctl",
                            ["print", &format!("{domain}/{}", launchd_label())],
                        )
                    })
                    .unwrap_or(ManagerProbe::Unavailable),
            );
            finish(report(platform, installed, active, Some(path), None)?)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            let installed = path.exists();
            let active = Some(probe_manager(
                "systemctl",
                ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
            ));
            finish(report(platform, installed, active, Some(path), None)?)
        }
        ServicePlatform::ManualLinux => print_report(
            cli,
            report(
                platform,
                false,
                None,
                None,
                Some(manual_linux_instructions()),
            )?,
        ),
        ServicePlatform::Unsupported => {
            anyhow::bail!("hub service management is supported on macOS and Linux only")
        }
    }
}

fn uninstall(platform: ServicePlatform, cli: &Cli) -> Result<()> {
    match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            if path.exists() {
                let domain = launchd_domain()?;
                // A stale/unloaded agent is already absent; do not turn that
                // benign state into a failed uninstall.
                let _ = Command::new("launchctl")
                    .args(["bootout", &domain])
                    .arg(&path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                fs::remove_file(&path).context("remove Cassy launchd agent")?;
            }
            print_report(cli, report(platform, false, Some(ManagerProbe::Observed(false)), Some(path), None)?)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            if path.exists() {
                let _ = manager_command("systemctl")
                    .args(["--user", "disable", "--now", SYSTEMD_UNIT])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                fs::remove_file(&path).context("remove Cassy systemd unit")?;
                run_manager("systemctl", ["--user", "daemon-reload"], None)?;
            }
            print_report(cli, report(platform, false, Some(ManagerProbe::Observed(false)), Some(path), None)?)
        }
        ServicePlatform::ManualLinux => print_report(
            cli,
            report(
                platform,
                false,
                None,
                None,
                Some(manual_linux_instructions()),
            )?,
        ),
        ServicePlatform::Unsupported => {
            anyhow::bail!("hub service management is supported on macOS and Linux only")
        }
    }
}

fn report(
    platform: ServicePlatform,
    installed: bool,
    active: Option<ManagerProbe>,
    path: Option<PathBuf>,
    instructions: Option<&'static str>,
) -> Result<ServiceReport> {
    let paths = HubRuntimePaths::default_for_user()?;
    use super::hub::HubDisplayState;
    let display_state = paths
        .read_process_record()
        .ok()
        .map(|record| super::hub::hub_display_state(&paths, &record))
        .filter(|state| *state != HubDisplayState::Exited)
        .or_else(|| paths.lock_holders().first().map(super::hub::lock_holder_display_state))
        .unwrap_or(HubDisplayState::Exited);
    let hub_running = display_state == HubDisplayState::Running;
    let hub_state = match display_state {
        HubDisplayState::Running => "running",
        HubDisplayState::Starting { .. } => "starting",
        HubDisplayState::Stopping { .. } => "stopping",
        HubDisplayState::Unresponsive { .. } => "unresponsive",
        HubDisplayState::Exited => "not_running",
    };
    let (platform_name, supervision) = match platform {
        ServicePlatform::Launchd => ("macos", "launchd"),
        ServicePlatform::Systemd => ("linux", "systemd-user"),
        ServicePlatform::ManualLinux => ("linux", "manual"),
        ServicePlatform::Unsupported => ("unsupported", "none"),
    };
    let enabled = match platform {
        ServicePlatform::Launchd => Some(ManagerProbe::Observed(installed)),
        ServicePlatform::Systemd => Some(probe_manager(
            "systemctl",
            ["--user", "is-enabled", "--quiet", SYSTEMD_UNIT],
        )),
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => None,
    };
    let manager_warning =
        if active == Some(ManagerProbe::TimedOut) || enabled == Some(ManagerProbe::TimedOut) {
            Some(MANAGER_TIMEOUT_WARNING)
        } else {
            active
                .and_then(ManagerProbe::warning)
                .or_else(|| enabled.and_then(ManagerProbe::warning))
        };
    Ok(ServiceReport {
        platform: platform_name,
        supervision,
        installed,
        enabled: enabled.and_then(ManagerProbe::observed),
        active: active.and_then(ManagerProbe::observed),
        manager_warning,
        unit_path: path.map(|path| path.display().to_string()),
        log_path: Some(paths.log_path().display().to_string()),
        hub_running,
        hub_state,
        launch_readiness: launch_readiness(),
        instructions,
    })
}

fn print_report(cli: &Cli, report: ServiceReport) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        let terminal_width = usize::from(crate::ui::components::formatter::terminal_width());
        let terminal_width = if terminal_width < 40 {
            80
        } else {
            terminal_width
        };
        let mut paths_shortened = false;
        let launch_issues = report
            .launch_readiness
            .iter()
            .filter(|readiness| !readiness.ready)
            .count();
        let launch_summary = match launch_issues {
            0 => "launches ready".to_owned(),
            1 => "1 launch issue".to_owned(),
            count => format!("{count} launch issues"),
        };
        match report.supervision {
            "manual" => println!(
                "Hub service: manual; hub {}; {launch_summary}",
                report.hub_state.replace('_', " ")
            ),
            manager => {
                let manager = if manager == "systemd-user" {
                    "systemd"
                } else {
                    manager
                };
                println!(
                    "Hub service ({manager}): {}; hub {}; {launch_summary}",
                    if report.installed {
                        "installed"
                    } else {
                        "not installed"
                    },
                    report.hub_state.replace('_', " ")
                );
            }
        }

        let first_repair = report
            .launch_readiness
            .iter()
            .position(|readiness| readiness.repair.is_some());
        if let Some(repair) = first_repair
            .and_then(|index| report.launch_readiness[index].repair.as_deref())
        {
            println!("  Fix: {repair}");
        } else if launch_issues == 0 {
            println!("  Launch readiness: healthy");
        }

        for (index, readiness) in report.launch_readiness.iter().enumerate() {
            if readiness.ready {
                println!("  {} launch: ready", readiness.cli);
                continue;
            }
            println!(
                "  {} launch: unavailable ({})",
                readiness.cli, readiness.summary
            );
            if let Some(cause) = &readiness.cause {
                let suffix = "; details: --json";
                let available = terminal_width.saturating_sub("    Cause: ".len() + suffix.len());
                let cause = cause.lines().next().unwrap_or(cause);
                let cause = fit_cause(cause, available);
                println!("    Cause: {cause}{suffix}");
            }
            if let Some(path) = &readiness.profile_path {
                let (path, shortened) = readiness_path(path, cli.full, terminal_width);
                paths_shortened |= shortened;
                println!("    path: {path}");
            }
            if Some(index) != first_repair
                && let Some(repair) = &readiness.repair
            {
                println!("    Fix: {repair}");
            }
        }
        if report.supervision == "manual" {
            for line in manual_linux_reboot_lines() {
                println!("{line}");
            }
        } else {
            println!(
                "  Supervision: {}; {}; {}",
                report.supervision,
                match report.enabled {
                    Some(true) => "enabled",
                    Some(false) => "disabled",
                    None => "enablement unknown",
                },
                match report.active {
                    Some(true) => "active",
                    Some(false) => "inactive",
                    None => "activity unknown",
                }
            );
        }
        if let Some(warning) = report.manager_warning {
            println!("  Warning: {warning}");
        }
        if let Some(log_path) = report.log_path {
            let (log_path, shortened) =
                readiness_path(Path::new(&log_path), cli.full, terminal_width);
            paths_shortened |= shortened;
            println!("  Logs: {log_path}");
        }
        if paths_shortened {
            println!("  Full paths: cas hub service status --full or --json");
        }
    }
    Ok(())
}

fn launchd_path() -> Result<PathBuf> {
    validate_launchd_test_label()?;
    Ok(home_dir()?
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", launchd_label())))
}

fn validate_launchd_test_label() -> Result<()> {
    if let Ok(label) = std::env::var(LAUNCHD_TEST_LABEL_ENV) {
        ensure!(
            !label.is_empty()
                && label != LAUNCHD_LABEL
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte)),
            "{LAUNCHD_TEST_LABEL_ENV} must be a distinct launchd label using letters, digits, dots, or hyphens"
        );
        ensure!(
            std::env::var(LAUNCHD_TEST_PORT_ENV)
                .ok()
                .and_then(|port| port.parse::<u16>().ok())
                .is_some_and(|port| port != 0 && port != DEFAULT_HUB_PORT),
            "{LAUNCHD_TEST_PORT_ENV} must be a non-default TCP port when using an isolated launchd label"
        );
    }
    Ok(())
}

fn service_port() -> u16 {
    if std::env::var_os(LAUNCHD_TEST_LABEL_ENV).is_some() {
        std::env::var(LAUNCHD_TEST_PORT_ENV)
            .ok()
            .and_then(|port| port.parse().ok())
            .filter(|port| *port != 0)
            .unwrap_or(DEFAULT_HUB_PORT)
    } else {
        DEFAULT_HUB_PORT
    }
}

fn systemd_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config/systemd/user").join(SYSTEMD_UNIT))
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("cannot determine home directory")
}

fn current_user() -> Result<String> {
    std::env::var("USER").context("cannot determine current user for systemd lingering")
}

fn launchd_domain() -> Result<String> {
    #[cfg(unix)]
    {
        Ok(format!("gui/{}", unsafe { libc::geteuid() }))
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("cannot determine launchd user domain")
    }
}

fn service_binary(dry_run: bool) -> Result<PathBuf> {
    let binary = std::env::current_exe().context("cannot resolve the running cas binary")?;
    ensure!(
        binary.is_absolute(),
        "Cassy service requires an absolute installed binary path"
    );
    if !dry_run {
        ensure!(
            !binary.components().any(|part| part.as_os_str() == ".cas")
                || !binary.to_string_lossy().contains("/.cas/worktrees/"),
            "refusing to install a hub service from a disposable Cassy worktree; install a released cas binary first"
        );
    }
    Ok(binary)
}

fn write_service_file(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("service definition has no parent directory")?;
    fs::create_dir_all(parent).context("create service definition directory")?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("create private service definition")?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path).context("install service definition")?;
    Ok(())
}

fn run_manager<const N: usize>(
    command: &str,
    args: [&str; N],
    trailing_path: Option<&Path>,
) -> Result<()> {
    let mut child = manager_command(command);
    child.args(args);
    child.stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(path) = trailing_path {
        child.arg(path);
    }
    let status = child.status().with_context(|| format!("run {command}"))?;
    ensure!(
        status.success(),
        "{command} refused the Cassy hub service operation"
    );
    Ok(())
}

fn run_manager_vec(command: &str, args: &[String]) -> Result<()> {
    let status = manager_command(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("run {command}"))?;
    ensure!(
        status.success(),
        "{command} refused the Cassy hub service operation"
    );
    Ok(())
}

/// Read-only manager diagnostics have the same finite child deadline as the
/// runtime receipt collector. Mutating manager operations retain their own
/// lifecycle semantics; an unknown diagnostic is never an inactive service.
fn probe_manager<const N: usize>(command: &str, args: [&str; N]) -> ManagerProbe {
    probe_manager_with_timeout(command, args, MANAGER_PROBE_TIMEOUT)
}

fn probe_manager_with_timeout<const N: usize>(command: &str, args: [&str; N], timeout: Duration) -> ManagerProbe {
    if timeout.is_zero() {
        return ManagerProbe::TimedOut;
    }
    let deadline = Instant::now() + MANAGER_PROBE_TIMEOUT.min(timeout);
    let Ok(mut child) = manager_command(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return ManagerProbe::Unavailable;
    };
    loop {
        let outcome = match child.try_wait() {
            Ok(Some(status)) => return ManagerProbe::Observed(status.success()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Ok(None) => ManagerProbe::TimedOut,
            Err(_) => ManagerProbe::Unavailable,
        };
        // Only the child we created is killed. Reaping happens off the
        // diagnostic deadline, including children stuck in kernel I/O.
        let _ = child.kill();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        return outcome;
    }
}

fn manager_command(command: &str) -> Command {
    match (command, std::env::var_os(SYSTEMCTL_PATH_ENV)) {
        ("systemctl", Some(path)) => Command::new(path),
        _ => Command::new(command),
    }
}

fn launchd_bootout_args(domain: &str, path: &Path) -> Vec<String> {
    vec!["bootout".into(), domain.into(), path.display().to_string()]
}

fn launchd_bootstrap_args(domain: &str, path: &Path) -> Vec<String> {
    vec![
        "bootstrap".into(),
        domain.into(),
        path.display().to_string(),
    ]
}

fn launchd_kickstart_args(domain: &str) -> Vec<String> {
    vec![
        "kickstart".into(),
        "-k".into(),
        format!("{domain}/{}", launchd_label()),
    ]
}

fn launchd_preview_actions(path: &Path) -> Vec<String> {
    vec![
        format!(
            "launchctl bootout gui/$UID {} (ignore if absent)",
            path.display()
        ),
        format!("launchctl bootstrap gui/$UID {}", path.display()),
        format!("launchctl kickstart -k gui/$UID/{}", launchd_label()),
    ]
}

fn systemd_preview_actions() -> Vec<String> {
    vec![
        "loginctl enable-linger $USER".into(),
        "systemctl --user daemon-reload".into(),
        format!("systemctl --user enable --now {SYSTEMD_UNIT}"),
    ]
}

fn human_dry_run(
    platform: ServicePlatform,
    path: &Path,
    definition: &str,
    actions: &[String],
    full: bool,
    utf8: bool,
) -> String {
    let (supervision, action_summary) = match platform {
        ServicePlatform::Launchd => (
            "launchd",
            "bootout (if present), bootstrap, kickstart via launchctl",
        ),
        ServicePlatform::Systemd => (
            "systemd-user",
            "enable lingering, reload, enable service via systemctl",
        ),
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => {
            ("none", "no manager steps")
        }
    };
    let dash = if utf8 { "—" } else { "-" };
    let header = format!(
        "Cassy hub service ({supervision}) dry run {dash} no files or manager state changed."
    );
    if !full {
        return format!(
            "{header}\nWould write: {}\nWould run: {action_summary}\nFull definition: cas hub service install --dry-run --full\n",
            service_file_display_path(platform, path)
        );
    }
    let actions = actions
        .iter()
        .map(|action| format!("  {action}\n"))
        .collect::<String>();
    format!(
        "{header}\nWould write: {}\nWould run:\n{actions}\nService definition:\n{definition}\n",
        path.display()
    )
}

fn service_file_display_path(platform: ServicePlatform, path: &Path) -> String {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "service file".into());
    match platform {
        ServicePlatform::Launchd => format!("~/Library/LaunchAgents/{file_name}"),
        ServicePlatform::Systemd => format!("~/.config/systemd/user/{file_name}"),
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => path.display().to_string(),
    }
}

fn print_dry_run(
    cli: &Cli,
    platform: ServicePlatform,
    path: &Path,
    definition: &str,
    actions: &[String],
) -> Result<()> {
    let (platform_name, supervision) = match platform {
        ServicePlatform::Launchd => ("macos", "launchd"),
        ServicePlatform::Systemd => ("linux", "systemd-user"),
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => ("unsupported", "none"),
    };
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "dry_run": true,
                "platform": platform_name,
                "supervision": supervision,
                "unit_path": path.display().to_string(),
                "log_path": HubRuntimePaths::default_for_user()?.log_path().display().to_string(),
                "actions": actions,
                "definition": definition,
            })
        );
    } else {
        print!(
            "{}",
            human_dry_run(
                platform,
                path,
                definition,
                actions,
                cli.full,
                crate::ui::components::locale_is_utf8(),
            )
        );
    }
    Ok(())
}

fn launchd_plist(
    binary: &Path,
    log_path: &Path,
    tailscale_serve: bool,
    tailscale_port: u16,
) -> String {
    let label = launchd_label();
    let isolated_home = if label != LAUNCHD_LABEL {
        format!(
            "    <key>HOME</key>\n    <string>{}</string>\n",
            xml_escape(&std::env::var("HOME").unwrap_or_default())
        )
    } else {
        String::new()
    };
    let args = service_args(binary, tailscale_serve, tailscale_port)
        .into_iter()
        .map(|arg| format!("    <string>{}</string>", xml_escape(&arg)))
        .collect::<Vec<_>>()
        .join("\n");
    let tailscale_override = launchd_tailscale_environment()
        .into_iter().filter_map(|(key, value)| value.map(|value| {
            format!(
                "    <key>{key}</key>\n    <string>{}</string>\n",
                xml_escape(&value.to_string_lossy())
            )
        })).collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
{file_limits}  <key>EnvironmentVariables</key>
  <dict>
{isolated_home}    <key>PATH</key>
    <string>{LAUNCHD_CLI_PATH}</string>
    <key>TERM</key>
    <string>dumb</string>
{tailscale_override}  </dict>
  <key>StandardOutPath</key>
  <string>{log_path}</string>
  <key>StandardErrorPath</key>
  <string>{log_path}</string>
</dict>
</plist>
"#,
        log_path = xml_escape(&log_path.display().to_string()),
        file_limits = ["SoftResourceLimits", "HardResourceLimits"]
            .map(|key| launchd_file_limit_block(key, HUB_FILE_LIMIT))
            .concat(),
    )
}

fn launchd_file_limit_block(key: &str, limit: u64) -> String {
    format!(
        "  <key>{key}</key>\n  <dict>\n    <key>NumberOfFiles</key>\n    <integer>{limit}</integer>\n  </dict>\n"
    )
}

/// Give an installed plist that predates cas-6cc6 the hub's file limits. An
/// existing `NumberOfFiles` (an operator's own value) is left alone.
fn ensure_launchd_file_limits(definition: &str) -> Result<String> {
    let mut definition = definition.to_owned();
    let mut limit = HUB_FILE_LIMIT;
    for key in ["SoftResourceLimits", "HardResourceLimits"] {
        let marker = format!("<key>{key}</key>");
        let Some(key_start) = definition.find(&marker) else {
            // Insert on its own line before EnvironmentVariables, whatever the
            // file's indentation (plutil writes tabs).
            let anchor = definition
                .find("<key>EnvironmentVariables</key>")
                .context("launchd plist has no EnvironmentVariables to place file limits before")?;
            let line_start = definition[..anchor]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            definition.insert_str(line_start, &launchd_file_limit_block(key, limit));
            continue;
        };
        let after_key = key_start + marker.len();
        let value_start = definition.len() - definition[after_key..].trim_start().len();
        if definition[value_start..].starts_with("<dict/>") {
            definition.replace_range(
                value_start..value_start + "<dict/>".len(),
                &format!(
                    "<dict>\n    <key>NumberOfFiles</key>\n    <integer>{limit}</integer>\n  </dict>"
                ),
            );
            continue;
        }
        ensure!(
            definition[value_start..].starts_with("<dict>"),
            "launchd {key} has no dict"
        );
        let dict_start = value_start + "<dict>".len();
        let dict_end = definition[dict_start..]
            .find("</dict>")
            .map(|offset| dict_start + offset)
            .with_context(|| format!("launchd {key} has an unclosed dict"))?;
        if let Some((_, value)) =
            definition[dict_start..dict_end].split_once("<key>NumberOfFiles</key>")
        {
            if key == "SoftResourceLimits" {
                let value = value
                    .trim_start()
                    .strip_prefix("<integer>")
                    .and_then(|value| value.split_once("</integer>"))
                    .context("launchd SoftResourceLimits NumberOfFiles has no integer")?
                    .0;
                // A missing hard limit must permit the operator's soft limit.
                limit = limit.max(
                    value
                        .trim()
                        .parse::<u64>()
                        .context("invalid launchd SoftResourceLimits NumberOfFiles")?,
                );
            }
        } else {
            // Only consider newlines inside this dict: an inline closing tag
            // must not place NumberOfFiles before the opening tag.
            let insert_at = definition[dict_start..dict_end]
                .rfind('\n')
                .map_or(dict_end, |newline| dict_start + newline + 1);
            let prefix = if insert_at == dict_end { "\n" } else { "" };
            definition.insert_str(
                insert_at,
                &format!("{prefix}    <key>NumberOfFiles</key>\n    <integer>{limit}</integer>\n"),
            );
        }
    }
    Ok(definition)
}

fn systemd_unit(binary: &Path, tailscale_serve: bool, tailscale_port: u16) -> String {
    let command = service_args(binary, tailscale_serve, tailscale_port)
        .into_iter()
        .map(|arg| systemd_escape(&arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=Cassy Commander hub\nAfter=network-online.target tailscaled.service\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={command}\nRestart=on-failure\nRestartForceExitStatus={SYSTEMD_FORCE_RESTART_SIGNALS}\nRestartSec=3\nLimitNOFILE={HUB_FILE_LIMIT}\nStandardOutput=append:%h/.cas/hub/hub.log\nStandardError=append:%h/.cas/hub/hub.log\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn service_args(binary: &Path, tailscale_serve: bool, tailscale_port: u16) -> Vec<String> {
    let mut args = vec![
        binary.display().to_string(),
        "hub".into(),
        "serve".into(),
        "--bind".into(),
        "127.0.0.1".into(),
        "--port".into(),
        service_port().to_string(),
        "--launched-by".into(),
        "service".into(),
    ];
    if tailscale_serve {
        args.extend([
            "--tailscale-serve".into(),
            "--tailscale-serve-port".into(),
            tailscale_port.to_string(),
        ]);
    } else {
        args.push("--no-tailscale-serve".into());
    }
    args
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd_escape(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".into();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-=:".contains(&byte))
    {
        value.into()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn manual_linux_instructions() -> &'static str {
    "systemd --user is unavailable; run `cas hub start --tailscale-serve` from your distribution's rc script after networking and Tailscale, and use `cas hub status` to verify it. Cassy cannot supervise reboot startup on this host."
}

fn manual_linux_reboot_lines() -> [&'static str; 3] {
    [
        "  Reboot: rc script after networking and Tailscale",
        "  Run: cas hub start --tailscale-serve",
        "  Verify: cas hub status",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_unload_after_five_seconds_uses_whole_stop_budget() {
        let unloaded_at = Instant::now() + Duration::from_millis(5_050);
        wait_for_manager_unload(stop_deadline(), |_| {
            ManagerProbe::Observed(Instant::now() < unloaded_at)
        }).expect("a launchd job disappearing after five seconds must fit the fifteen-second Stop budget");
    }

    #[test]
    fn successful_bootout_waits_for_loaded_job_to_disappear() {
        let mut calls = 0;
        wait_for_manager_unload(crate::bounded_process::Deadline::after(Duration::from_secs(1)), |_| {
            calls += 1;
            ManagerProbe::Observed(calls == 1)
        }).unwrap();
        assert_eq!(calls, 2);

        let error = wait_for_manager_unload(crate::bounded_process::Deadline::after(Duration::from_millis(10)), |_| {
            ManagerProbe::Observed(true)
        }).unwrap_err();
        assert!(error.to_string().contains("still loaded after launchctl bootout"));
        let error = wait_for_manager_unload(crate::bounded_process::Deadline::after(Duration::from_secs(1)), |_| {
            ManagerProbe::TimedOut
        }).unwrap_err();
        assert!(error.to_string().contains("unknown (timed out)"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unknown_manager_refuses_restart_before_service_mutation_cas_8ee8() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let manager = fixture.path().join("systemctl");
        let log = fixture.path().join("manager.log");
        crate::test_paths::warm_stub(
            &manager,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CAS_MANAGER_LOG"
if [ "$2" = "is-active" ]; then exec /usr/bin/tail -f /dev/null; fi
exit 0
"#,
        );
        env.set(SYSTEMCTL_PATH_ENV, &manager);
        env.set("CAS_MANAGER_LOG", &log);
        let unit = systemd_path().unwrap();
        write_service_file(&unit, LEGACY_SYSTEMD_UNIT).unwrap();
        let cli = Cli {
            json: false,
            full: false,
            verbose: false,
            command: None,
        };
        let started = Instant::now();
        let error = restart_supervised(&cli, true, 443).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("unknown (timed out)"));
        assert!(
            error
                .to_string()
                .contains("refusing to change hub service ownership")
        );
        assert_eq!(fs::read_to_string(&unit).unwrap(), LEGACY_SYSTEMD_UNIT);
        let calls = fs::read_to_string(&log).unwrap();
        assert!(calls.lines().any(|line| line.contains("is-active")));
        assert!(
            !calls
                .lines()
                .any(|line| line.contains("daemon-reload") || line.contains("restart"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn service_report_keeps_timed_out_enablement_unknown_cas_8ee8() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let manager = fixture.path().join("systemctl");
        crate::test_paths::warm_stub(&manager, "#!/bin/sh\nexec /usr/bin/tail -f /dev/null\n");
        env.set(SYSTEMCTL_PATH_ENV, &manager);
        let report = report(
            ServicePlatform::Systemd,
            true,
            Some(ManagerProbe::Observed(true)),
            Some(systemd_path().unwrap()),
            None,
        )
        .unwrap();
        assert_eq!(report.active, Some(true));
        assert_eq!(report.enabled, None);
        assert_eq!(report.manager_warning, Some(MANAGER_TIMEOUT_WARNING));
        let json = serde_json::to_value(report).unwrap();
        assert!(json["enabled"].is_null());
        assert!(
            json["manager_warning"]
                .as_str()
                .unwrap()
                .contains("timed out")
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn unavailable_manager_preserves_manual_mode_and_installed_ownership_cas_8ee8() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let manager = fixture.path().join("systemctl");
        env.set(SYSTEMCTL_PATH_ENV, &manager);
        let paths = HubRuntimePaths::default_for_user().unwrap();
        // Exercise an unavailable manager even when the platform was
        // resolved before it disappeared, rather than only manual mode.
        assert_eq!(
            inactive_detached_warning_on(ServicePlatform::Systemd, &paths, None).unwrap(),
            None,
        );
        assert_eq!(inactive_detached_warning(&paths, None).unwrap(), None);
        write_service_file(&systemd_path().unwrap(), LEGACY_SYSTEMD_UNIT).unwrap();
        assert_eq!(
            inactive_detached_warning_on(ServicePlatform::Systemd, &paths, None).unwrap(),
            Some(MANAGER_UNAVAILABLE_WARNING),
        );
        assert_eq!(inactive_detached_warning(&paths, None).unwrap(), Some(MANAGER_UNAVAILABLE_WARNING));
        let cli = Cli { json: false, full: false, verbose: false, command: None };
        assert!(restart_supervised(&cli, false, 443).unwrap_err().to_string().contains("refusing to launch a detached hub"));
    }

    #[test]
    fn profile_capture_failure_is_reported_without_failing_service_lifecycle() {
        let warning =
            capture_launch_profiles_warning(Err(anyhow::anyhow!("malformed host config")));
        assert_eq!(
            warning.as_deref(),
            Some("Hub launch profile not recorded: malformed host config")
        );
        assert!(capture_launch_profiles_warning(Ok(())).is_none());
    }

    #[test]
    fn service_install_dry_run_has_compact_summary_and_full_escape_cas_0051() {
        let path = Path::new("/Users/test/Library/LaunchAgents/dev.cas.commander-hub.plist");
        let definition = launchd_plist(
            Path::new("/Users/test/.local/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            true,
            443,
        );
        let actions = launchd_preview_actions(path);

        let summary = human_dry_run(
            ServicePlatform::Launchd,
            path,
            &definition,
            &actions,
            false,
            false,
        );
        assert!(
            !summary.contains(&definition),
            "the default human preview should not dump the plist"
        );
        assert!(
            summary.contains("Would write: ~/Library/LaunchAgents/dev.cas.commander-hub.plist")
        );
        assert!(
            summary.contains("Would run: bootout (if present), bootstrap, kickstart via launchctl")
        );
        assert!(summary.contains("Full definition: cas hub service install --dry-run --full"));
        assert!(summary.starts_with(
            "Cassy hub service (launchd) dry run - no files or manager state changed."
        ));
        assert!(summary.lines().all(|line| line.len() <= 80), "{summary}");

        let full = human_dry_run(
            ServicePlatform::Launchd,
            path,
            &definition,
            &actions,
            true,
            false,
        );
        assert!(full.contains(&definition));
        assert!(full.contains(path.to_str().unwrap()));
    }

    #[test]
    fn update_refuses_stale_launchd_binary_cas_94fb() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("hub.plist");
        let definition = include_str!("../../tests/fixtures/hub-service-launchd.plist");
        fs::write(&path, definition).unwrap();
        let error = refresh_installed_service_at(
            ServicePlatform::Launchd,
            &path,
            true,
            Path::new("/Users/test/.local/bin/cas"),
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("/opt/cas/bin/cas"), "{message}");
        assert!(message.contains("/Users/test/.local/bin/cas"), "{message}");
        assert!(message.contains("cas hub service install"), "{message}");
        assert_eq!(fs::read_to_string(&path).unwrap(), definition);
        assert!(
            !refresh_installed_service_at(
                ServicePlatform::Launchd,
                &path,
                true,
                Path::new("/opt/cas/bin/cas"),
            )
            .unwrap()
        );
    }

    #[test]
    fn update_refuses_stale_systemd_binary_cas_94fb() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("hub.service");
        let definition = include_str!("../../tests/fixtures/hub-service-systemd.service");
        fs::write(&path, definition).unwrap();
        let error = refresh_installed_service_at(
            ServicePlatform::Systemd,
            &path,
            true,
            Path::new("/home/test/.local/bin/cas"),
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("/opt/cas/bin/cas"), "{message}");
        assert!(message.contains("/home/test/.local/bin/cas"), "{message}");
        assert!(message.contains("cas hub service install"), "{message}");
        assert_eq!(fs::read_to_string(&path).unwrap(), definition);
        assert!(
            !refresh_installed_service_at(
                ServicePlatform::Systemd,
                &path,
                true,
                Path::new("/opt/cas/bin/cas"),
            )
            .unwrap()
        );
    }

    #[test]
    fn systemd_update_accepts_deleted_process_path_cas_94fb() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("hub.service");
        let definition = include_str!("../../tests/fixtures/hub-service-systemd.service");
        fs::write(&path, definition).unwrap();
        assert!(
            !refresh_installed_service_at(
                ServicePlatform::Systemd,
                &path,
                true,
                Path::new("/opt/cas/bin/cas (deleted)"),
            )
            .expect("a renamed process image still identifies the installed cas")
        );
        let error = refresh_installed_service_at(
            ServicePlatform::Systemd,
            &path,
            true,
            Path::new("/different/cas (deleted)"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cas hub service install"));
        assert_eq!(fs::read_to_string(&path).unwrap(), definition);
    }

    #[test]
    fn service_activation_reports_starting_until_ready_cas_94fb() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let activation = report(
            ServicePlatform::Launchd,
            true,
            Some(ManagerProbe::Observed(true)),
            None,
            None,
        )
        .unwrap()
        .after_start();
        assert!(!activation.hub_running);
        assert_eq!(activation.hub_state, "starting");
        let json = serde_json::to_value(activation).unwrap();
        assert_eq!(json["hub_state"], "starting");
        assert_eq!(json["hub_running"], false);
        let paths = HubRuntimePaths::default_for_user().unwrap();
        let mut lock = paths.acquire_instance_lock().unwrap();
        let starting = report(ServicePlatform::Launchd, true, Some(ManagerProbe::Observed(true)), None, None).unwrap();
        assert_eq!(starting.hub_state, "starting", "plain status before a process record exists");
        let process: HubProcessRecord = serde_json::from_value(serde_json::json!({
            "pid": std::process::id(), "bind": "127.0.0.1", "port": 0,
            "version": env!("CARGO_PKG_VERSION"), "started_at": chrono::Utc::now().to_rfc3339(),
        })).unwrap();
        paths.write_process_record(&process).unwrap();
        let starting = report(ServicePlatform::Launchd, true, Some(ManagerProbe::Observed(true)), None, None).unwrap();
        assert_eq!(starting.hub_state, "starting", "plain status with a startup record");
        assert!(!starting.hub_running);
        lock.set_phase("stopping").unwrap();
        let stopping = report(ServicePlatform::Launchd, true, Some(ManagerProbe::Observed(true)), None, None).unwrap();
        assert_eq!(stopping.hub_state, "stopping");
        drop(lock);
        // The fixture process stays alive; remove metadata as though that
        // process had exited so the lock-owner fallback cannot still see it.
        fs::remove_file(paths.lock_path()).unwrap();
        paths.remove_process_record().unwrap();
        let stopped = report(ServicePlatform::Launchd, true, Some(ManagerProbe::Observed(false)), None, None).unwrap();
        assert_eq!(stopped.hub_state, "not_running");
    }

    #[test]
    fn launchd_plist_is_a_secret_free_golden_with_tailscale_round_trip() {
        let plist = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            true,
            8443,
        );
        assert_eq!(
            plist,
            include_str!("../../tests/fixtures/hub-service-launchd.plist")
        );
        assert!(plist.contains("<string>--tailscale-serve</string>"));
        assert!(plist.contains("<string>8443</string>"));
        assert!(plist.contains("<string>127.0.0.1</string>"));
        assert!(plist.contains("<key>StandardOutPath</key>"));
        assert!(plist.contains("<key>StandardErrorPath</key>"));
        assert!(plist.contains("/Users/test/.cas/hub/hub.log"));
        assert!(!plist.to_ascii_lowercase().contains("token"));
        assert!(!plist.contains("auth.json"));
    }

    #[test]
    fn launchd_publication_rewrite_round_trips_and_preserves_other_keys() {
        let original = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        let enabled = rewrite_launchd_publication_flags(&original, true, 8443).unwrap();
        assert!(enabled.contains("<string>--tailscale-serve</string>"));
        assert!(enabled.contains("<string>8443</string>"));
        assert_eq!(
            rewrite_launchd_publication_flags(&enabled, true, 9443)
                .unwrap()
                .matches("<string>--tailscale-serve</string>")
                .count(),
            1
        );
        let disabled = rewrite_launchd_publication_flags(&enabled, false, 443).unwrap();
        assert!(!disabled.contains("--tailscale-serve"));
        assert!(!disabled.contains("8443"));
        assert!(disabled.contains("<key>KeepAlive</key>"));
        assert!(disabled.contains("/Users/test/.cas/hub/hub.log"));
        assert_eq!(
            rewrite_launchd_publication_flags(&disabled, true, 8443).unwrap(),
            enabled
        );
        let custom = original
            .replace("<integer>8192</integer>", "<integer>16384</integer>")
            .replace(
                &launchd_file_limit_block("HardResourceLimits", HUB_FILE_LIMIT)
                    .replace("8192", "16384"),
                "",
            );
        let repaired = rewrite_launchd_publication_flags(&custom, true, 8443).unwrap();
        for key in ["SoftResourceLimits", "HardResourceLimits"] {
            let dict = repaired
                .split(&format!("<key>{key}</key>"))
                .nth(1)
                .unwrap()
                .split("<dict>")
                .nth(1)
                .unwrap()
                .split("</dict>")
                .next()
                .unwrap();
            assert!(dict.contains("<integer>16384</integer>"), "{key}: {dict}");
        }
        assert_eq!(
            rewrite_launchd_publication_flags(&repaired, true, 8443).unwrap(),
            repaired
        );
    }

    #[test]
    fn launchd_publication_rewrite_rejects_malformed_arguments() {
        let plist = "<key>ProgramArguments</key><array><integer>3</integer></array>";
        assert!(rewrite_launchd_publication_flags(plist, true, 443).is_err());
    }

    #[test]
    fn launchd_publication_repair_adds_cli_path_to_older_plist() {
        let old = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        let old = old.replace(
            &format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n    <key>TAILSCALE_BE_CLI</key>\n    <string>1</string>\n  </dict>\n"
            ),
            "",
        );
        // A plist installed before cas-6cc6 has no file limits either.
        let old = ["SoftResourceLimits", "HardResourceLimits"]
            .into_iter()
            .fold(old, |plist, limits| {
                plist.replace(
                    &format!(
                        "  <key>{limits}</key>\n  <dict>\n    <key>NumberOfFiles</key>\n    <integer>8192</integer>\n  </dict>\n"
                    ),
                    "",
                )
            });
        assert!(!old.contains("NumberOfFiles"));
        let repaired = rewrite_launchd_publication_flags(&old, true, 8443).unwrap();
        assert!(repaired.contains(LAUNCHD_CLI_PATH));
        assert!(repaired.contains("<key>TERM</key>\n    <string>dumb</string>"));
        assert!(repaired.contains("<string>--tailscale-serve</string>"));
        assert_eq!(
            repaired.matches("<key>NumberOfFiles</key>").count(),
            2,
            "{repaired}"
        );
        for empty in ["<dict/>", "<dict></dict>", "<dict> </dict>"] {
            let existing = ["SoftResourceLimits", "HardResourceLimits"]
                .into_iter()
                .fold(repaired.clone(), |plist, key| {
                    plist.replace(
                        &launchd_file_limit_block(key, HUB_FILE_LIMIT),
                        &format!("  <key>{key}</key>\n  {empty}\n"),
                    )
                });
            let fixed = rewrite_launchd_publication_flags(&existing, true, 8443).unwrap();
            for key in ["SoftResourceLimits", "HardResourceLimits"] {
                let dict = fixed
                    .split(&format!("<key>{key}</key>"))
                    .nth(1)
                    .unwrap()
                    .split("<dict>")
                    .nth(1)
                    .unwrap()
                    .split("</dict>")
                    .next()
                    .unwrap();
                assert!(
                    dict.contains("<key>NumberOfFiles</key>"),
                    "{empty}: {key}: {dict}"
                );
                assert!(dict.contains("<integer>8192</integer>"), "{dict}");
            }
            assert_eq!(
                rewrite_launchd_publication_flags(&fixed, true, 8443).unwrap(),
                fixed
            );
        }
        assert_eq!(
            rewrite_launchd_publication_flags(&repaired, true, 8443).unwrap(),
            repaired,
            "a current plist must not count as stale"
        );
        // plutil and PlistBuddy save tab-indented plists, as on prowl.
        let tabbed = ["SoftResourceLimits", "HardResourceLimits"]
            .into_iter()
            .fold(
                launchd_plist(
                    Path::new("/opt/cas/bin/cas"),
                    Path::new("/Users/test/.cas/hub/hub.log"),
                    true,
                    8443,
                ),
                |plist, limits| {
                    plist.replace(&launchd_file_limit_block(limits, HUB_FILE_LIMIT), "")
                },
            )
            .replace("\n    ", "\n\t\t")
            .replace("\n  ", "\n\t");
        assert!(!tabbed.contains("NumberOfFiles") && !tabbed.contains("\n  <key>"));
        let repaired = rewrite_launchd_publication_flags(&tabbed, true, 8443).unwrap();
        assert_eq!(
            repaired.matches("<key>NumberOfFiles</key>").count(),
            2,
            "{repaired}"
        );
    }

    #[test]
    fn launchd_publication_repair_adds_missing_path_inside_existing_environment() {
        let original = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        let existing = original.replace(
            &format!("    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>"),
            "    <key>HOME</key>\n    <string>/Users/test</string>",
        );
        let repaired = rewrite_launchd_publication_flags(&existing, true, 8443).unwrap();
        assert_eq!(
            repaired.matches("<key>EnvironmentVariables</key>").count(),
            1
        );
        assert!(repaired.contains("<key>HOME</key>\n    <string>/Users/test</string>"));
        assert!(repaired.contains(&format!(
            "<key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>"
        )));
    }

    #[test]
    fn launchd_publication_repair_preserves_operator_path() {
        let original = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        let custom_path = "/custom/bin:/usr/bin:/bin";
        let existing = original.replace(LAUNCHD_CLI_PATH, custom_path);
        let repaired = rewrite_launchd_publication_flags(&existing, true, 8443).unwrap();
        assert!(repaired.contains(&format!("<string>{custom_path}</string>")));
        assert!(!repaired.contains(LAUNCHD_CLI_PATH));
        assert_eq!(repaired.matches("<key>PATH</key>").count(), 1);
    }

    #[test]
    fn launchd_publication_repair_expands_empty_environment_dict() {
        let original = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        let existing = original.replace(
            &format!(
                "<dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n    <key>TAILSCALE_BE_CLI</key>\n    <string>1</string>\n  </dict>"
            ),
            "<dict/>",
        );
        let repaired = rewrite_launchd_publication_flags(&existing, true, 8443).unwrap();
        assert!(repaired.contains(&format!(
            "<key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>"
        )));
        assert!(!repaired.contains("<dict/>"));
    }

    #[test]
    fn launchd_restart_repairs_legacy_environment_without_publication_change() {
        let current = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            true,
            8443,
        );
        let legacy = current.replace("    <key>TERM</key>\n    <string>dumb</string>\n", "")
            .replace("    <key>TAILSCALE_BE_CLI</key>\n    <string>1</string>\n", "");
        assert_eq!(ensure_launchd_cli_environment(&legacy).unwrap(), current);
        let legacy = current.replace(
            &format!("  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n    <key>TAILSCALE_BE_CLI</key>\n    <string>1</string>\n  </dict>\n"),
            "",
        );
        assert_eq!(ensure_launchd_cli_environment(&legacy).unwrap(), current);
    }

    #[test]
    fn service_publication_rewrites_preserve_custom_settings_and_opt_out() {
        let old = LEGACY_SYSTEMD_UNIT
            .replace(
                "--tailscale-serve --tailscale-serve-port 8443",
                "--no-tailscale-serve",
            )
            .replace(
                "RestartSec=3",
                "RestartSec=7\nEnvironment=OPERATOR_SETTING=kept",
            );
        let old = old.replace("/opt/cas/bin/cas", "\"/opt/cas  dir/bin/cas\"");
        let enabled = rewrite_systemd_publication_flags(&old, true, 8443).unwrap();
        assert!(
            enabled.contains("\"/opt/cas  dir/bin/cas\""),
            "quoted executable must survive byte-for-byte"
        );
        assert!(enabled.contains("--tailscale-serve --tailscale-serve-port 8443"));
        assert!(enabled.contains("RestartSec=7\nEnvironment=OPERATOR_SETTING=kept"));
        let disabled = rewrite_systemd_publication_flags(&enabled, false, 443).unwrap();
        assert!(disabled.contains("--no-tailscale-serve"));
        assert!(!disabled.contains("--tailscale-serve "));
        assert_eq!(
            rewrite_systemd_publication_flags(&disabled, false, 443).unwrap(),
            disabled
        );
        let plist = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/tmp/hub.log"),
            false,
            443,
        );
        assert!(plist.contains("--no-tailscale-serve"));
        let published = rewrite_launchd_publication_flags(&plist, true, 8443).unwrap();
        assert!(!published.contains("--no-tailscale-serve"));
        assert_eq!(service_https_port(&published), 8443);
        let loopback = rewrite_launchd_publication_flags(&published, false, 443).unwrap();
        assert!(loopback.contains("--no-tailscale-serve"));
        assert!(!loopback.contains("<string>--tailscale-serve</string>"));
    }

    #[test]
    fn detached_takeover_handles_inactive_service_and_old_wedged_holders() {
        let owner = HubLockHolder {
            pid: 42,
            age: None,
            phase: Some("running".into()),
            command: Some("/opt/cas/bin/cas hub serve --port 4173".into()),
        };
        let mut record: HubProcessRecord = serde_json::from_value(serde_json::json!({
            "pid":42,"bind":"127.0.0.1","port":4173,"version":"3.27.0",
            "started_at":"2026-09-23T21:00:00Z","launched_by":"update"
        }))
        .unwrap();
        assert_eq!(
            detached_hub_takeover(None, &owner, false),
            DetachedTakeover::Stop
        );
        assert_eq!(
            detached_hub_takeover(Some(&record), &owner, false),
            DetachedTakeover::Stop
        );
        assert_eq!(
            detached_hub_takeover(Some(&record), &owner, false),
            DetachedTakeover::Stop
        );
        let old_wedged = HubLockHolder {
            phase: None,
            ..owner.clone()
        };
        assert_eq!(
            detached_hub_takeover(Some(&record), &old_wedged, false),
            DetachedTakeover::Stop
        );
        record.launched_by = Some("service".into());
        assert_eq!(
            detached_hub_takeover(Some(&record), &owner, true),
            DetachedTakeover::None
        );
        assert_eq!(
            detached_hub_takeover(Some(&record), &owner, false),
            DetachedTakeover::Stop
        );
        record.pid = 43;
        assert_eq!(
            detached_hub_takeover(Some(&record), &owner, false),
            DetachedTakeover::Stop
        );
        let unknown = HubLockHolder {
            command: None,
            ..owner
        };
        assert_eq!(
            detached_hub_takeover(Some(&record), &unknown, false),
            DetachedTakeover::Refuse
        );
        record.pid = 42;
        assert_eq!(
            detached_hub_takeover(Some(&record), &unknown, false),
            DetachedTakeover::Stop
        );
        assert!(
            describe_holders(&[unknown])
                .contains("pid 42 (phase running, age unknown age, command unknown)")
        );
    }

    #[test]
    fn systemd_unit_is_a_secret_free_golden_with_loopback_only_bind() {
        let unit = systemd_unit(Path::new("/opt/cas/bin/cas"), true, 8443);
        assert_eq!(
            unit,
            include_str!("../../tests/fixtures/hub-service-systemd.service")
        );
        assert!(unit.contains("ExecStart=/opt/cas/bin/cas hub serve --bind 127.0.0.1 --port 4173 --launched-by service --tailscale-serve --tailscale-serve-port 8443"));
        assert!(!unit.to_ascii_lowercase().contains("token"));
        assert!(!unit.contains("credentials"));
        assert!(unit.contains("StandardOutput=append:%h/.cas/hub/hub.log"));
        assert!(unit.contains("StandardError=append:%h/.cas/hub/hub.log"));
    }

    /// The unit as generated before cas-621ec, with `Restart=on-failure` only.
    const LEGACY_SYSTEMD_UNIT: &str = "[Unit]\nDescription=Cassy Commander hub\nAfter=network-online.target tailscaled.service\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart=/opt/cas/bin/cas hub serve --bind 127.0.0.1 --port 4173 --launched-by service --tailscale-serve --tailscale-serve-port 8443\nRestart=on-failure\nRestartSec=3\nStandardOutput=append:%h/.cas/hub/hub.log\nStandardError=append:%h/.cas/hub/hub.log\n\n[Install]\nWantedBy=default.target\n";

    fn service_lines(unit: &str) -> Vec<&str> {
        unit.lines()
            .skip_while(|line| *line != "[Service]")
            .take_while(|line| !line.is_empty())
            .collect()
    }

    #[test]
    fn systemd_unit_restarts_after_a_signal_death_but_not_a_deliberate_stop() {
        let unit = systemd_unit(Path::new("/opt/cas/bin/cas"), false, 443);
        let service = service_lines(&unit);
        // systemd treats SIGHUP/SIGINT/SIGTERM/SIGPIPE deaths as clean, so
        // Restart=on-failure alone left a SIGPIPE-killed hub dead.
        assert!(service.contains(&"Restart=on-failure"), "{unit}");
        let forced = service
            .iter()
            .find_map(|line| line.strip_prefix("RestartForceExitStatus="))
            .expect("unit forces restart after clean-classed signals");
        let forced: Vec<&str> = forced.split_whitespace().collect();
        for signal in ["SIGPIPE", "SIGHUP", "SIGINT"] {
            assert!(forced.contains(&signal), "{signal} missing from {forced:?}");
        }
        // `systemctl stop` and `cas hub stop` send SIGTERM and mean "stay
        // stopped"; forcing a restart on it would undo an explicit stop.
        assert!(!forced.contains(&"SIGTERM"), "{forced:?}");
        assert!(service.contains(&"RestartSec=3"), "{unit}");
    }

    #[test]
    fn legacy_systemd_unit_upgrades_to_the_current_template() {
        let upgraded =
            upgrade_systemd_restart_policy(LEGACY_SYSTEMD_UNIT).expect("legacy unit needs upgrade");
        assert_eq!(
            upgraded,
            systemd_unit(Path::new("/opt/cas/bin/cas"), true, 8443)
        );
        assert_eq!(
            upgrade_systemd_restart_policy(&upgraded),
            None,
            "upgrade is idempotent"
        );
        // A unit from before cas-6cc6 already has the restart policy but no
        // file limit, so it is stale too.
        let current = systemd_unit(Path::new("/opt/cas/bin/cas"), true, 8443);
        let pre_limit = current.replace("LimitNOFILE=8192\n", "");
        assert_eq!(
            upgrade_systemd_restart_policy(&pre_limit).as_deref(),
            Some(current.as_str())
        );
        assert_eq!(
            upgrade_systemd_restart_policy(&systemd_unit(
                Path::new("/opt/cas/bin/cas"),
                false,
                443
            )),
            None,
            "a current unit is left alone"
        );
    }

    #[test]
    fn systemd_restart_upgrade_preserves_operator_edits_and_replaces_stale_lists() {
        let edited = LEGACY_SYSTEMD_UNIT.replace(
            "RestartSec=3\n",
            "RestartSec=10\nEnvironment=TAILSCALE=/opt/ts/bin/tailscale\n",
        );
        let upgraded = upgrade_systemd_restart_policy(&edited).unwrap();
        assert!(upgraded.contains("RestartSec=10\nEnvironment=TAILSCALE=/opt/ts/bin/tailscale\n"));
        assert!(upgraded.contains(
            "Restart=on-failure\nRestartForceExitStatus=SIGHUP SIGINT SIGPIPE\nRestartSec=10\n"
        ));

        let stale = LEGACY_SYSTEMD_UNIT.replace(
            "RestartSec=3\n",
            "RestartSec=3\nRestartForceExitStatus=SIGHUP\n",
        );
        let upgraded = upgrade_systemd_restart_policy(&stale).unwrap();
        assert_eq!(
            upgraded.matches("RestartForceExitStatus=").count(),
            1,
            "{upgraded}"
        );
        assert!(upgraded.contains("RestartForceExitStatus=SIGHUP SIGINT SIGPIPE\n"));

        let no_restart_line = LEGACY_SYSTEMD_UNIT.replace("Restart=on-failure\n", "");
        let upgraded = upgrade_systemd_restart_policy(&no_restart_line).unwrap();
        assert!(upgraded.contains(
            "[Service]\nRestart=on-failure\nRestartForceExitStatus=SIGHUP SIGINT SIGPIPE\nType=simple\n"
        ));

        assert_eq!(
            upgrade_systemd_restart_policy("[Unit]\nDescription=not a service\n"),
            None
        );
    }

    #[test]
    fn launchd_plist_keeps_the_hub_alive_after_any_exit() {
        let plist = launchd_plist(
            Path::new("/opt/cas/bin/cas"),
            Path::new("/Users/test/.cas/hub/hub.log"),
            false,
            443,
        );
        // KeepAlive=true relaunches on every exit, signal deaths included; it
        // is launchd's equivalent of the systemd force-restart list.
        assert!(plist.contains("<key>KeepAlive</key>\n  <true/>"), "{plist}");
        // launchd's default soft limit of 256 files let the hub hit EMFILE
        // under Commander bursts (cas-6cc6).
        for limits in ["SoftResourceLimits", "HardResourceLimits"] {
            assert!(
                plist.contains(&format!(
                    "<key>{limits}</key>\n  <dict>\n    <key>NumberOfFiles</key>\n    <integer>8192</integer>\n  </dict>"
                )),
                "{plist}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn installed_legacy_unit_is_rewritten_and_reloaded_once() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let systemctl = fixture.path().join("systemctl");
        crate::test_paths::warm_stub(
            &systemctl,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CAS_SYSTEMCTL_LOG"
exit 0
"#,
        );
        env.set(SYSTEMCTL_PATH_ENV, &systemctl);
        let log = fixture.path().join("systemctl.log");
        env.set("CAS_SYSTEMCTL_LOG", &log);
        let unit = fixture.path().join("cas-hub.service");
        write_service_file(&unit, LEGACY_SYSTEMD_UNIT).unwrap();

        assert!(
            refresh_systemd_unit(&unit).unwrap(),
            "legacy unit is rewritten"
        );
        assert_eq!(
            fs::read_to_string(&unit).unwrap(),
            systemd_unit(Path::new("/opt/cas/bin/cas"), true, 8443)
        );
        assert!(
            !refresh_systemd_unit(&unit).unwrap(),
            "current unit is left alone"
        );
        let reloads = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|line| *line == "--user daemon-reload")
            .count();
        assert_eq!(reloads, 1);
    }

    #[test]
    fn manual_linux_status_has_an_honest_reboot_fallback() {
        let instructions = manual_linux_instructions();
        assert!(instructions.contains("systemd --user is unavailable"));
        assert!(instructions.contains("rc script"));
        assert!(instructions.contains("cannot supervise reboot startup"));
        let human = manual_linux_reboot_lines().join("\n");
        assert!(human.contains("rc script after networking and Tailscale"));
        assert!(human.contains("cas hub start --tailscale-serve"));
        assert!(human.contains("cas hub status"));
    }

    #[test]
    fn service_definition_detection_distinguishes_serve_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let loopback = temp.path().join("loopback.service");
        let published = temp.path().join("published.service");
        fs::write(&loopback, "ExecStart=/opt/cas/bin/cas hub serve\n").unwrap();
        fs::write(
            &published,
            "ExecStart=/opt/cas/bin/cas hub serve --tailscale-serve\n",
        )
        .unwrap();

        assert!(!service_file_requests_tailscale(&loopback).unwrap());
        assert!(service_file_requests_tailscale(&published).unwrap());
    }

    #[test]
    fn systemd_service_binary_reads_the_absolute_exec_start_path() {
        let temp = tempfile::tempdir().unwrap();
        let unit = temp.path().join("cas-hub.service");
        fs::write(
            &unit,
            "[Service]\nExecStart=/opt/cas/bin/cas hub serve --bind 127.0.0.1\n",
        )
        .unwrap();

        assert_eq!(
            systemd_service_binary(&unit).unwrap(),
            PathBuf::from("/opt/cas/bin/cas")
        );
    }

    #[test]
    fn service_arguments_keep_tailscale_optional_and_loopback_fixed() {
        assert_eq!(
            service_args(Path::new("/opt/cas/bin/cas"), false, 443),
            vec![
                "/opt/cas/bin/cas",
                "hub",
                "serve",
                "--bind",
                "127.0.0.1",
                "--port",
                "4173",
                "--launched-by",
                "service",
                "--no-tailscale-serve"
            ]
        );
    }

    #[test]
    fn service_arguments_with_default_host_policy_request_tailscale_serve() {
        let default_policy = crate::config::Config::default()
            .get("hub.tailscale_serve")
            .expect("registered host publication key")
            .parse::<bool>()
            .expect("boolean host publication policy");
        // pin: An unset host publication policy must emit the Serve-on form;
        // explicit off must emit --no-tailscale-serve so the child stays off.
        assert_eq!(
            service_args(Path::new("/opt/cas/bin/cas"), default_policy, 443),
            vec![
                "/opt/cas/bin/cas", "hub", "serve", "--bind", "127.0.0.1",
                "--port", "4173", "--launched-by", "service",
                "--tailscale-serve", "--tailscale-serve-port", "443"
            ]
        );
    }

    #[test]
    fn inactive_detached_warning_only_flags_a_bypassed_service() {
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), true, Some("update")),
            Some(INACTIVE_DETACHED_HUB_WARNING)
        );
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), true, Some("cli")),
            Some(INACTIVE_DETACHED_HUB_WARNING)
        );
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), true, Some("service")),
            None
        );
        assert_eq!(
            inactive_detached_warning_for(true, Some(true), true, Some("update")),
            None
        );
        assert_eq!(
            inactive_detached_warning_for(false, Some(false), true, Some("update")),
            None
        );
        // cas-621ec: an inactive service with no hub at all is the state a
        // SIGPIPE-killed service left behind; it is reported, not skipped.
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), false, Some("update")),
            Some(INACTIVE_SERVICE_NO_HUB_ERROR)
        );
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), false, None),
            Some(INACTIVE_SERVICE_NO_HUB_ERROR)
        );
        assert_eq!(
            inactive_detached_warning_for(false, Some(false), false, None),
            None
        );
        assert_eq!(
            inactive_detached_warning_for(true, Some(true), false, None),
            None
        );
        assert_eq!(inactive_detached_warning_for(true, None, false, None), None);
    }

    #[test]
    fn inactive_service_findings_name_the_exact_fix_command() {
        for finding in [INACTIVE_DETACHED_HUB_WARNING, INACTIVE_SERVICE_NO_HUB_ERROR] {
            assert!(
                finding.starts_with("service installed but inactive"),
                "{finding}"
            );
            assert!(finding.contains("Fix: run `cas hub restart`"), "{finding}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn installed_systemd_unit_owns_restart_and_receives_the_new_record() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let bin = fixture.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let systemctl = bin.join("systemctl");
        crate::test_paths::warm_stub(
            &systemctl,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CAS_SYSTEMCTL_LOG"
if [ "$1" = "--user" ] && [ "$2" = "--version" ]; then
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "is-active" ]; then
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "restart" ] && [ "$3" = "cas-hub.service" ]; then
  cp "$CAS_NEW_RECORD" "$CAS_HUB_ROOT/process.json"
  cp "$CAS_NEW_LOCK" "$CAS_HUB_ROOT/hub.lock"
  exit 0
fi
exit 1
"#,
        );
        env.set(SYSTEMCTL_PATH_ENV, &systemctl);

        let log = fixture.path().join("systemctl.log");
        env.set("CAS_SYSTEMCTL_LOG", &log);

        let hub_root = env.home().join(".cas/hub");
        crate::hub::ensure_private_dir(&hub_root).unwrap();
        let service_path = env.home().join(".config/systemd/user/cas-hub.service");
        write_service_file(
            &service_path,
            &systemd_unit(Path::new("/opt/cas/bin/cas"), false, 443),
        )
        .unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accept with a deadline: when restart fails before probing health,
        // the test must fail, not hang on join.
        listener.set_nonblocking(true).unwrap();
        let health = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(error) => panic!("health listener failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let body = br#"{"schema_version":1,"ready":true}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                std::str::from_utf8(body).unwrap()
            )
            .unwrap();
        });
        let mut service_process = Command::new("sleep").arg("30").spawn().unwrap();
        let new_record = HubProcessRecord {
            pid: service_process.id(),
            sid: None,
            pgid: None,
            bind: "127.0.0.1".into(),
            port,
            version: env!("CARGO_PKG_VERSION").into(),
            started_at: "2026-09-21T15:30:00Z".into(),
            cgroup: None,
            launched_by: Some("service".into()),
            launched_at: Some("2026-09-21T15:30:00Z".into()),
            public_url: None,
            tailscale_serve_port: None,
            tailscale_cli: None,
            tailscale_serve_target: None,
            transport_warning: None,
            nofile_limit: None,
        };
        let new_record_path = fixture.path().join("new-process.json");
        fs::write(&new_record_path, serde_json::to_vec(&new_record).unwrap()).unwrap();
        let new_lock_path = fixture.path().join("new-hub.lock");
        fs::write(
            &new_lock_path,
            serde_json::json!({
                "pid": service_process.id(),
                "acquired_at": "2026-09-21T15:30:00Z",
                "phase": "running"
            })
            .to_string(),
        )
        .unwrap();
        env.set("CAS_HUB_ROOT", &hub_root);
        env.set("CAS_NEW_RECORD", &new_record_path);
        env.set("CAS_NEW_LOCK", &new_lock_path);

        // cas-24bd: this test was SIGTERMed on any host running a real hub.
        // The stub reported the unit inactive, so restart took the detached-
        // hub takeover path; the old record (this test's own PID, on
        // DEFAULT_HUB_PORT) read as live because the real hub answered its
        // health probe, and the takeover signalled the test. The stubbed unit
        // is now active, and the old hub is a disposable child…
        let old_hub = Command::new("sleep").arg("30").spawn().unwrap();
        let old_hub_pid = old_hub.id();
        let old_hub_reaper = std::thread::spawn(move || {
            let mut old_hub = old_hub;
            let _ = old_hub.wait();
        });
        // …on a port nothing answers: DEFAULT_HUB_PORT is the real hub on a
        // factory host, whose health reply made this record read as live.
        let dead_port = TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let old_record = HubProcessRecord {
            pid: old_hub_pid,
            sid: None,
            pgid: None,
            bind: "127.0.0.1".into(),
            port: dead_port,
            version: "3.26.0".into(),
            started_at: "2026-09-21T15:29:00Z".into(),
            cgroup: None,
            launched_by: Some("service".into()),
            launched_at: Some("2026-09-21T15:29:00Z".into()),
            public_url: None,
            tailscale_serve_port: None,
            tailscale_cli: None,
            tailscale_serve_target: None,
            transport_warning: None,
            nofile_limit: None,
        };
        fs::write(
            hub_root.join("process.json"),
            serde_json::to_vec(&old_record).unwrap(),
        )
        .unwrap();

        let cli = Cli {
            json: false,
            full: false,
            verbose: false,
            command: None,
        };
        let handled = restart_supervised(&cli, false, DEFAULT_HUB_PORT);
        service_process.kill().unwrap();
        let _ = service_process.wait();
        // Whether or not restart stopped it, the old hub must not outlive the test.
        let _ = Command::new("kill").arg(old_hub_pid.to_string()).status();
        old_hub_reaper.join().unwrap();
        health.join().unwrap();

        assert!(handled.unwrap(), "installed service must handle restart");
        let manager_log = fs::read_to_string(log).unwrap();
        assert!(manager_log.lines().any(|line| line == "--user --version"));
        assert!(
            manager_log
                .lines()
                .any(|line| line == "--user restart cas-hub.service"),
            "manager log: {manager_log}"
        );
        assert!(
            !manager_log.contains("hub serve"),
            "manager log: {manager_log}"
        );
        assert_eq!(
            HubRuntimePaths::new(&hub_root)
                .read_process_record()
                .unwrap()
                .launched_by
                .as_deref(),
            Some("service")
        );
    }

    #[cfg(unix)]
    #[test]
    fn definition_install_and_uninstall_preserve_private_hub_state() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let hub = home.join(".cas/hub");
        crate::hub::ensure_private_dir(&hub).unwrap();
        let identity = hub.join("identity.json");
        let auth = hub.join("auth.json");
        fs::write(&identity, "identity-kept").unwrap();
        fs::write(&auth, "auth-kept").unwrap();
        fs::set_permissions(&identity, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();

        let definition = home.join("Library/LaunchAgents/cas-test.plist");
        write_service_file(
            &definition,
            &launchd_plist(
                Path::new("/opt/cas/bin/cas"),
                Path::new("/Users/test/.cas/hub/hub.log"),
                true,
                443,
            ),
        )
        .unwrap();
        // A repeat install replaces only its own definition, including a
        // changed Serve choice, and never touches the hub state directory.
        write_service_file(
            &definition,
            &launchd_plist(
                Path::new("/opt/cas/bin/cas"),
                Path::new("/Users/test/.cas/hub/hub.log"),
                false,
                443,
            ),
        )
        .unwrap();
        fs::remove_file(&definition).unwrap();

        assert_eq!(fs::read_to_string(&identity).unwrap(), "identity-kept");
        assert_eq!(fs::read_to_string(&auth).unwrap(), "auth-kept");
        assert_eq!(fs::metadata(&hub).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&auth).unwrap().mode() & 0o777, 0o600);
        assert!(!definition.exists());
    }

    #[test]
    fn launchd_bootstrap_uses_the_user_gui_domain_and_definition_path() {
        let path = Path::new("/Users/test/Library/LaunchAgents/dev.cas.commander-hub.plist");
        assert_eq!(
            launchd_bootstrap_args("gui/501", path),
            vec![
                "bootstrap",
                "gui/501",
                "/Users/test/Library/LaunchAgents/dev.cas.commander-hub.plist"
            ]
        );
        assert_eq!(
            launchd_kickstart_args("gui/501"),
            vec!["kickstart", "-k", "gui/501/dev.cas.commander-hub"]
        );
    }

    #[test]
    fn dry_run_preview_is_explicit_and_contains_no_manager_side_effect() {
        let actions = systemd_preview_actions();
        assert_eq!(actions[0], "loginctl enable-linger $USER");
        assert!(actions[2].contains("enable --now cas-hub.service"));
        assert!(
            launchd_preview_actions(Path::new("/tmp/cas-hub.plist"))
                .iter()
                .any(|action| action.contains("bootstrap gui/$UID"))
        );
    }
}
