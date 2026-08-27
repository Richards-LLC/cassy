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

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{Cli, hub::HubServiceCommands};
use crate::hub::{DEFAULT_HUB_PORT, HubRuntimePaths};

const LAUNCHD_LABEL: &str = "dev.cas.commander-hub";
const SYSTEMD_UNIT: &str = "cas-hub.service";

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

fn status(platform: ServicePlatform, cli: &Cli) -> Result<()> {
    match platform {
        ServicePlatform::Launchd => {
            let path = launchd_path()?;
            let installed = path.exists();
            let active = launchd_domain().ok().map(|domain| {
                command_succeeds("launchctl", ["print", &format!("{domain}/{LAUNCHD_LABEL}")])
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
                let _ = Command::new("systemctl")
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
    Ok(home_dir()?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
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
    let mut child = Command::new(command);
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
    let status = Command::new(command)
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
    Command::new(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
        format!("{domain}/{LAUNCHD_LABEL}"),
    ]
}

fn launchd_preview_actions(path: &Path) -> Vec<String> {
    vec![
        format!(
            "launchctl bootout gui/$UID {} (ignore if absent)",
            path.display()
        ),
        format!("launchctl bootstrap gui/$UID {}", path.display()),
        format!("launchctl kickstart -k gui/$UID/{LAUNCHD_LABEL}"),
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
    let args = service_args(binary, tailscale_serve, tailscale_port)
        .into_iter()
        .map(|arg| format!("    <string>{}</string>", xml_escape(&arg)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
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
        DEFAULT_HUB_PORT.to_string(),
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
    fn systemd_unit_is_a_secret_free_golden_with_loopback_only_bind() {
        let unit = systemd_unit(Path::new("/opt/cas/bin/cas"), true, 8443);
        assert_eq!(
            unit,
            include_str!("../../tests/fixtures/hub-service-systemd.service")
        );
        assert!(unit.contains("ExecStart=/opt/cas/bin/cas hub serve --bind 127.0.0.1 --port 4173 --tailscale-serve --tailscale-serve-port 8443"));
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
                "4173"
            ]
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
