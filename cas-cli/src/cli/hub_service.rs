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

const LAUNCHD_LABEL: &str = "dev.cas.commander-hub";
const LAUNCHD_TEST_LABEL_ENV: &str = "CAS_HUB_LAUNCHD_LABEL";
const LAUNCHD_TEST_PORT_ENV: &str = "CAS_HUB_SERVICE_PORT";
const LAUNCHD_CLI_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
const SYSTEMD_UNIT: &str = "cas-hub.service";
const SYSTEMCTL_PATH_ENV: &str = "CAS_HUB_SYSTEMCTL";
pub(crate) const INACTIVE_DETACHED_HUB_WARNING: &str =
    "service installed but inactive, detached hub running";
const LAUNCHD_TAILSCALE_REFUSAL: &str = "`cas hub service install --tailscale-serve` is not supported for launchd: Tailscale Serve needs the interactive user's GUI namespace, while launchd starts in its bootstrap namespace. Install the loopback-only service with `cas hub service install`, or run `cas hub service uninstall && cas hub start --tailscale-serve` from an interactive shell when Commander pairing needs a public URL.";

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
    unit_path: Option<String>,
    log_path: Option<String>,
    hub_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'static str>,
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
        HubServiceCommands::Status => status(platform, cli),
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
        if command_succeeds("systemctl", ["--user", "--version"]) {
            ServicePlatform::Systemd
        } else {
            ServicePlatform::ManualLinux
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
    match platform {
        ServicePlatform::Launchd => {
            if tailscale_serve {
                anyhow::bail!("{LAUNCHD_TAILSCALE_REFUSAL}");
            }
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
            status(platform, cli)
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
            status(platform, cli)
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
            let active = command_succeeds(
                "launchctl",
                ["print", &format!("{domain}/{}", launchd_label())],
            );
            let definition = fs::read_to_string(&path)?;
            let service_tailscale = definition.contains("--tailscale-serve");
            let rewritten = if service_publication_repair_needed(tailscale_serve, service_tailscale)
            {
                rewrite_launchd_publication_flags(
                    &definition,
                    true,
                    tailscale_port,
                )?
            } else {
                ensure_launchd_cli_environment(&definition)?
            };
            let rewritten = (rewritten != definition).then_some(rewritten);
            let paths = HubRuntimePaths::default_for_user()?;
            stop_detached_hub_if_present(cli, &paths, active)?;
            let previous_pid = paths.read_process_record().ok().map(|record| record.pid);
            if let Some(rewritten) = rewritten {
                if active {
                    run_manager_vec("launchctl", &launchd_bootout_args(&domain, &path))?;
                }
                write_service_file(&path, &rewritten)?;
                run_manager_vec("launchctl", &launchd_bootstrap_args(&domain, &path))?;
            } else if !active {
                run_manager_vec("launchctl", &launchd_bootstrap_args(&domain, &path))?;
            }
            run_manager_vec("launchctl", &launchd_kickstart_args(&domain))?;
            wait_for_supervised_hub(previous_pid, tailscale_serve || service_tailscale)?;
            Ok(true)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            if !path.is_file() {
                return Ok(false);
            }
            let service_tailscale = service_file_requests_tailscale(&path)?;
            if service_publication_repair_needed(tailscale_serve, service_tailscale) {
                repair_systemd_publication_flags(&path, tailscale_port)?;
            }
            let paths = HubRuntimePaths::default_for_user()?;
            let active = command_succeeds(
                "systemctl",
                ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
            );
            stop_detached_hub_if_present(cli, &paths, active)?;
            let previous_pid = paths.read_process_record().ok().map(|record| record.pid);
            run_manager("systemctl", ["--user", "restart", SYSTEMD_UNIT], None)?;
            wait_for_supervised_hub(previous_pid, tailscale_serve || service_tailscale)?;
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
        if arg == "--tailscale-serve" {
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
    ensure_launchd_cli_environment(&rewritten)
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
        let tailscale_override = std::env::var_os("TAILSCALE").filter(|value| !value.is_empty());
        let has_tailscale = definition[dict_start..dict_end].contains("<key>TAILSCALE</key>");
        if has_path && has_term && (tailscale_override.is_none() || has_tailscale) {
            return Ok(definition.to_owned());
        }
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
        if !has_tailscale && let Some(executable) = tailscale_override {
            additions.push_str(&format!(
                "    <key>TAILSCALE</key>\n    <string>{}</string>\n",
                xml_escape(&executable.to_string_lossy())
            ));
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

fn service_publication_repair_needed(requested: bool, configured: bool) -> bool {
    // The CLI boolean is additive: bare `restart` does not mean "turn Serve off".
    // Lifecycle also recovers intent from the owned receipt. Systemd repairs
    // only the false -> true case, so launchd must preserve an existing route
    // when the request is false as well. Reinstalling a loopback-only service
    // is the explicit flag-off operation today.
    requested && !configured
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

fn repair_systemd_publication_flags(path: &Path, tailscale_port: u16) -> Result<()> {
    let binary = systemd_service_binary(path)?;
    write_service_file(path, &systemd_unit(&binary, true, tailscale_port))?;
    run_manager("systemctl", ["--user", "daemon-reload"], None)
}

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

/// Return the operator-facing warning used by `cas hub status` and `cas doctor`
/// when an installed service has been bypassed by a detached hub.
pub(super) fn inactive_detached_warning(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
) -> Result<Option<&'static str>> {
    let (installed, active) = match native_platform() {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let domain = launchd_domain()?;
            (
                path.is_file(),
                Some(command_succeeds(
                    "launchctl",
                    ["print", &format!("{domain}/{}", launchd_label())],
                )),
            )
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            (
                path.is_file(),
                Some(command_succeeds(
                    "systemctl",
                    ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
                )),
            )
        }
        ServicePlatform::ManualLinux | ServicePlatform::Unsupported => (false, None),
    };
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
        active,
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
    (installed && active == Some(false) && hub_live && launched_by != Some("service"))
        .then_some(INACTIVE_DETACHED_HUB_WARNING)
}

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

fn status(platform: ServicePlatform, cli: &Cli) -> Result<()> {
    match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let installed = path.exists();
            let active = launchd_domain().ok().map(|domain| {
                command_succeeds(
                    "launchctl",
                    ["print", &format!("{domain}/{}", launchd_label())],
                )
            });
            print_report(cli, report(platform, installed, active, Some(path), None)?)
        }
        ServicePlatform::Systemd => {
            let path = systemd_path()?;
            let installed = path.exists();
            let active = Some(command_succeeds(
                "systemctl",
                ["--user", "is-active", "--quiet", SYSTEMD_UNIT],
            ));
            print_report(cli, report(platform, installed, active, Some(path), None)?)
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
            print_report(cli, report(platform, false, Some(false), Some(path), None)?)
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
            print_report(cli, report(platform, false, Some(false), Some(path), None)?)
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
    active: Option<bool>,
    path: Option<PathBuf>,
    instructions: Option<&'static str>,
) -> Result<ServiceReport> {
    let paths = HubRuntimePaths::default_for_user()?;
    let hub_running = paths
        .read_process_record()
        .ok()
        .is_some_and(|record| super::hub::record_is_live(&record));
    let (platform_name, supervision) = match platform {
        ServicePlatform::Launchd => ("macos", "launchd"),
        ServicePlatform::Systemd => ("linux", "systemd-user"),
        ServicePlatform::ManualLinux => ("linux", "manual"),
        ServicePlatform::Unsupported => ("unsupported", "none"),
    };
    Ok(ServiceReport {
        platform: platform_name,
        supervision,
        installed,
        enabled: match platform {
            ServicePlatform::Launchd => Some(installed),
            ServicePlatform::Systemd => Some(command_succeeds(
                "systemctl",
                ["--user", "is-enabled", "--quiet", SYSTEMD_UNIT],
            )),
            ServicePlatform::ManualLinux | ServicePlatform::Unsupported => None,
        },
        active,
        unit_path: path.map(|path| path.display().to_string()),
        log_path: Some(paths.log_path().display().to_string()),
        hub_running,
        instructions,
    })
}

fn print_report(cli: &Cli, report: ServiceReport) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        match report.supervision {
            "manual" => {
                println!(
                    "Cassy hub supervision is manual: {}",
                    report.instructions.unwrap_or_default()
                );
            }
            manager => {
                println!(
                    "Cassy hub service ({manager}) is {}{}{}; hub is {}",
                    if report.installed {
                        "installed"
                    } else {
                        "not installed"
                    },
                    match report.enabled {
                        Some(true) => ", enabled",
                        Some(false) => ", disabled",
                        None => "",
                    },
                    match report.active {
                        Some(true) => " and active",
                        Some(false) => " and inactive",
                        None => "",
                    },
                    if report.hub_running {
                        "running"
                    } else {
                        "not running"
                    },
                );
                if let Some(log_path) = report.log_path {
                    println!("  Logs: {log_path}");
                }
            }
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

fn launchd_label() -> String {
    std::env::var(LAUNCHD_TEST_LABEL_ENV).unwrap_or_else(|_| LAUNCHD_LABEL.to_owned())
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

fn command_succeeds<const N: usize>(command: &str, args: [&str; N]) -> bool {
    manager_command(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
        println!("Cassy hub service ({supervision}) dry run — no files or manager state changed.");
        println!("Would write: {}", path.display());
        println!("Would run:");
        for action in actions {
            println!("  {action}");
        }
        println!("\nService definition:\n{definition}");
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
    let tailscale_override = std::env::var_os("TAILSCALE")
        .filter(|value| !value.is_empty())
        .map(|value| {
            format!(
                "    <key>TAILSCALE</key>\n    <string>{}</string>\n",
                xml_escape(&value.to_string_lossy())
            )
        })
        .unwrap_or_default();
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
  <key>EnvironmentVariables</key>
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
        log_path = xml_escape(&log_path.display().to_string())
    )
}

fn systemd_unit(binary: &Path, tailscale_serve: bool, tailscale_port: u16) -> String {
    let command = service_args(binary, tailscale_serve, tailscale_port)
        .into_iter()
        .map(|arg| systemd_escape(&arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=Cassy Commander hub\nAfter=network-online.target tailscaled.service\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={command}\nRestart=on-failure\nRestartSec=3\nStandardOutput=append:%h/.cas/hub/hub.log\nStandardError=append:%h/.cas/hub/hub.log\n\n[Install]\nWantedBy=default.target\n"
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

#[cfg(test)]
mod tests {
    use super::*;

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
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n  </dict>\n"
            ),
            "",
        );
        let repaired = rewrite_launchd_publication_flags(&old, true, 8443).unwrap();
        assert!(repaired.contains(LAUNCHD_CLI_PATH));
        assert!(repaired.contains("<key>TERM</key>\n    <string>dumb</string>"));
        assert!(repaired.contains("<string>--tailscale-serve</string>"));
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
                "<dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n  </dict>"
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
        let legacy = current.replace("    <key>TERM</key>\n    <string>dumb</string>\n", "");
        assert_eq!(ensure_launchd_cli_environment(&legacy).unwrap(), current);
        let legacy = current.replace(
            &format!("  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{LAUNCHD_CLI_PATH}</string>\n    <key>TERM</key>\n    <string>dumb</string>\n  </dict>\n"),
            "",
        );
        assert_eq!(ensure_launchd_cli_environment(&legacy).unwrap(), current);
    }

    #[test]
    fn service_restart_publication_policy_matches_systemd_additive_repair() {
        assert!(!service_publication_repair_needed(false, false));
        assert!(!service_publication_repair_needed(false, true));
        assert!(service_publication_repair_needed(true, false));
        assert!(!service_publication_repair_needed(true, true));
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

    #[test]
    fn manual_linux_status_has_an_honest_reboot_fallback() {
        let instructions = manual_linux_instructions();
        assert!(instructions.contains("systemd --user is unavailable"));
        assert!(instructions.contains("rc script"));
        assert!(instructions.contains("cannot supervise reboot startup"));
    }

    #[test]
    fn launchd_tailscale_refusal_names_the_interactive_pairing_recovery() {
        assert!(LAUNCHD_TAILSCALE_REFUSAL.contains("bootstrap namespace"));
        assert!(LAUNCHD_TAILSCALE_REFUSAL.contains("cas hub service install`"));
        assert!(
            LAUNCHD_TAILSCALE_REFUSAL
                .contains("cas hub service uninstall && cas hub start --tailscale-serve")
        );
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
                "service"
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
        assert_eq!(
            inactive_detached_warning_for(true, Some(false), false, Some("update")),
            None
        );
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
        let health = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
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

        let old_record = HubProcessRecord {
            pid: std::process::id(),
            sid: None,
            pgid: None,
            bind: "127.0.0.1".into(),
            port: DEFAULT_HUB_PORT,
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
