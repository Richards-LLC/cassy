//! Read-only local observations. Remote presence is held for the cloud contract.
//!
//! A local receipt does not enable monitoring, resume jobs or prove a reboot.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{HubProcessRecord, HubRuntimePaths, MachineIdentityStore};

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const MANAGER_OUTPUT_LIMIT: u64 = 4096;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObservationState {
    Healthy,
    Failed,
    Unknown,
    Disabled,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Observation {
    pub state: ObservationState,
    pub reason: &'static str,
    pub source: &'static str,
}

impl Observation {
    pub(crate) fn new(state: ObservationState, reason: &'static str, source: &'static str) -> Self {
        Self {
            state,
            reason,
            source,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FactoryObservation {
    pub health: Observation,
    pub registered: usize,
    pub daemon_processes_present: usize,
    pub stale_records: usize,
    pub jobs_resumed: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BootPrerequisites {
    pub platform: &'static str,
    pub service_installed: bool,
    pub service_enabled: Option<bool>,
    pub service_active: Option<bool>,
    pub linger_enabled: Option<bool>,
    pub gui_login_required: bool,
    pub gui_session_available: Option<bool>,
    pub condition: &'static str,
    pub reboot_verified: bool,
    pub next_step: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeReceipt {
    pub schema_version: u32,
    pub binary_version: &'static str,
    pub probe_window_started_at: String,
    pub observed_at: String,
    pub collection_duration_ms: u64,
    pub observer: &'static str,
    pub hub_id: Option<String>,
    pub current_os_boot_id: Option<String>,
    pub recorded_hub_instance_id: Option<String>,
    pub hub: Observation,
    pub serve_publication: Observation,
    pub external_reachability: Observation,
    pub factory: FactoryObservation,
    pub boot_prerequisites: BootPrerequisites,
    pub independent_monitoring: Observation,
}

/// This is a diagnostic collection, never an enrollment or a service operation.
/// The caller supplies its already-completed current health/publication probes.
/// Record identity is explicitly historical when those probes fail.
pub(crate) fn collect_runtime_receipt(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
    hub: Observation,
    serve_publication: Observation,
    probe_window_started_at: chrono::DateTime<chrono::Utc>,
) -> RuntimeReceipt {
    let started = Instant::now();
    let identity = MachineIdentityStore::new(paths.root()).load().ok();
    let home = paths.root().parent().and_then(Path::parent);
    let native_boot = native_boot_identity();
    let current_os_boot_id = identity
        .as_ref()
        .zip(native_boot.as_ref())
        .map(|(identity, boot)| scoped_identity("os-boot", &identity.id, boot));
    let recorded_hub_instance_id = identity
        .as_ref()
        .zip(record)
        .filter(|(_, record)| !record.started_at.trim().is_empty())
        .map(|(identity, record)| {
            scoped_identity(
                "hub-instance",
                &identity.id,
                &format!("{}:{}", record.pid, record.started_at),
            )
        });
    let sessions = home
        .ok_or_else(|| std::io::Error::other("home unavailable"))
        .and_then(|home| {
            crate::ui::factory::SessionManager::for_home_read_only(home).list_sessions_read_only()
        });
    let factory = match sessions {
        Ok(sessions) => factory_observation(
            sessions.len(),
            sessions.iter().filter(|s| s.is_running).count(),
        ),
        Err(_) => FactoryObservation {
            health: Observation::new(
                ObservationState::Unknown,
                "session_records_unavailable",
                "session_metadata",
            ),
            registered: 0,
            daemon_processes_present: 0,
            stale_records: 0,
            jobs_resumed: None,
        },
    };
    let boot_prerequisites = boot_prerequisites(home);
    RuntimeReceipt {
        schema_version: 1,
        binary_version: env!("CARGO_PKG_VERSION"),
        probe_window_started_at: probe_window_started_at.to_rfc3339(),
        observed_at: chrono::Utc::now().to_rfc3339(),
        collection_duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        observer: "local_cli",
        hub_id: identity.map(|id| id.id),
        current_os_boot_id,
        recorded_hub_instance_id,
        hub,
        serve_publication,
        external_reachability: Observation::new(
            ObservationState::Unknown,
            "independent_probe_required",
            "not_observed",
        ),
        factory,
        boot_prerequisites,
        independent_monitoring: Observation::new(
            ObservationState::Unknown,
            "cloud_snapshot_required",
            "operator_presence_v1",
        ),
    }
}

fn scoped_identity(kind: &str, hub: &str, native: &str) -> String {
    let mut digest = Sha256::new();
    // JSON avoids ambiguous concatenation of identifiers or native boot fields.
    digest.update(
        serde_json::to_vec(&("cas-runtime-receipt-v1", kind, hub, native))
            .expect("string tuple serializes"),
    );
    format!("{:x}", digest.finalize())
}

/// cas-d8e7's host-only policy defaults on; no project config can change it.
pub(crate) fn requested_publication(paths: &HubRuntimePaths) -> Option<bool> {
    let config = paths.root().parent()?.join("config.toml");
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(true),
        Err(_) => return None,
    };
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    match parsed.get("hub").and_then(|hub| hub.get("tailscale_serve")) {
        Some(value) => value.as_bool(),
        None => Some(true),
    }
}

fn factory_observation(registered: usize, present: usize) -> FactoryObservation {
    let (state, reason) = if registered == 0 {
        (ObservationState::Unknown, "no_registered_factories")
    } else if present == 0 {
        (
            ObservationState::Failed,
            "registered_factory_daemons_absent",
        )
    } else {
        // A process or metadata file cannot prove a supervisor is responsive.
        (ObservationState::Unknown, "supervisor_health_unverified")
    };
    FactoryObservation {
        health: Observation::new(state, reason, "session_metadata_and_process_presence"),
        registered,
        daemon_processes_present: present,
        stale_records: registered.saturating_sub(present),
        jobs_resumed: None,
    }
}

fn native_boot_identity() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        uuid::Uuid::parse_str(raw.trim())
            .ok()
            .map(|id| id.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let output = bounded_output("/usr/sbin/sysctl", &["-n", "kern.boottime"], PROBE_TIMEOUT)?;
        if !output.success {
            return None;
        }
        parse_macos_boot_time(&output.stdout)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_boot_time(raw: &str) -> Option<String> {
    // kern.boottime prints a struct followed by a locale-dependent date. Only
    // the kernel seconds/microseconds identify the boot; ignore the date text.
    let fields = raw.split_once('{')?.1.split_once('}')?.0;
    let mut seconds = None;
    let mut micros = None;
    for item in fields.split(',') {
        let (key, value) = item.split_once('=')?;
        match key.trim() {
            "sec" => seconds = value.trim().parse::<u64>().ok(),
            "usec" => micros = value.trim().parse::<u32>().ok(),
            _ => return None,
        }
    }
    let (seconds, micros) = (seconds?, micros?);
    (seconds > 0 && micros < 1_000_000).then(|| format!("{seconds}:{micros}"))
}

struct CommandOutput {
    success: bool,
    stdout: String,
}

/// Bound the child and stdout reader separately; a hung manager or inherited
/// stdout pipe cannot pin status. Only the exact diagnostic child is killed.
fn bounded_output(program: &str, args: &[&str], timeout: Duration) -> Option<CommandOutput> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MANAGER_OUTPUT_LIMIT)
            .read_to_end(&mut bytes)
            .ok()
            .filter(|_| bytes.len() < MANAGER_OUTPUT_LIMIT as usize)
            .and_then(|_| String::from_utf8(bytes).ok());
        let _ = send.send(result);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                // Waiting for a killed process can still block in kernel I/O.
                // Reap it off the diagnostic caller's finite deadline.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return None;
            }
        }
    };
    let stdout = receive
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()??;
    Some(CommandOutput {
        success: status.success(),
        stdout,
    })
}

fn boot_prerequisites(home: Option<&Path>) -> BootPrerequisites {
    let mut facts = BootPrerequisites {
        platform: std::env::consts::OS,
        service_installed: false,
        service_enabled: None,
        service_active: None,
        linger_enabled: None,
        gui_login_required: cfg!(target_os = "macos"),
        gui_session_available: None,
        condition: "unsupported_platform",
        reboot_verified: false,
        next_step: "Use a supported service manager; local status cannot prove reboot recovery.",
    };
    let Some(home) = home else {
        facts.condition = "home_unavailable";
        facts.next_step = "Inspect the runtime under the service owner's home.";
        return facts;
    };
    if dirs::home_dir().as_deref() != Some(home) {
        facts.condition = "foreign_home_manager_unobserved";
        facts.next_step = "Run status as the service owner to inspect its manager.";
        return facts;
    }
    #[cfg(target_os = "linux")]
    {
        facts.service_installed = home.join(".config/systemd/user/cas-hub.service").is_file();
        let manager = std::env::var("CAS_HUB_SYSTEMCTL").unwrap_or_else(|_| "systemctl".into());
        if let Some(output) = bounded_output(
            &manager,
            &[
                "--user",
                "show",
                "cas-hub.service",
                "--property=UnitFileState",
                "--property=ActiveState",
            ],
            PROBE_TIMEOUT,
        )
        .filter(|o| o.success)
        {
            let properties = output
                .stdout
                .lines()
                .filter_map(|line| line.split_once('='))
                .collect::<std::collections::HashMap<_, _>>();
            facts.service_enabled = properties
                .get("UnitFileState")
                .filter(|v| !v.is_empty())
                .map(|v| *v == "enabled");
            facts.service_active = properties
                .get("ActiveState")
                .filter(|v| !v.is_empty())
                .map(|v| *v == "active");
        }
        // SAFETY: geteuid only reads this process's user identity.
        let uid = unsafe { libc::geteuid() }.to_string();
        facts.linger_enabled = bounded_output(
            "loginctl",
            &["show-user", &uid, "--property=Linger", "--value"],
            PROBE_TIMEOUT,
        )
        .filter(|o| o.success)
        .and_then(|o| match o.stdout.trim() {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        });
        facts.condition = linux_boot_condition(
            facts.service_installed,
            facts.service_enabled,
            facts.linger_enabled,
        );
    }
    #[cfg(target_os = "macos")]
    {
        facts.service_installed = home
            .join("Library/LaunchAgents/dev.cas.commander-hub.plist")
            .is_file();
        // SAFETY: geteuid only reads this process's user identity.
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        facts.gui_session_available =
            bounded_output("/bin/launchctl", &["print", &domain], PROBE_TIMEOUT).map(|o| o.success);
        let target = format!("{domain}/dev.cas.commander-hub");
        facts.service_active = bounded_output("/bin/launchctl", &["print", &target], PROBE_TIMEOUT)
            .filter(|o| o.success)
            .and_then(|o| {
                o.stdout.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("state = ")
                        .map(|state| state == "running")
                })
            });
        facts.condition = if !facts.service_installed {
            "service_not_installed"
        } else if facts.gui_session_available == Some(false) {
            "gui_login_required"
        } else if facts.gui_session_available == Some(true) {
            "gui_login_recovery_not_reboot_verified"
        } else {
            "service_manager_observation_unavailable"
        };
    }
    facts.next_step = match facts.condition {
        "service_not_installed" => "Install the hub service before relying on automatic recovery.",
        "service_not_boot_enabled" => "Enable the installed user service for boot recovery.",
        "user_linger_required" => "Enable user linger before relying on recovery without login.",
        "gui_login_required" => {
            "Log in to the macOS GUI; this LaunchAgent cannot recover before login."
        }
        "gui_login_recovery_not_reboot_verified" => {
            "Verify RunAtLoad, KeepAlive and hub health after GUI login; pre-login recovery is unsupported."
        }
        "configured_not_reboot_verified" => {
            "Verify hub identity, health and publication in a supervised reboot test."
        }
        _ => "Inspect the service manager; its current state could not be verified.",
    };
    facts
}

#[cfg(any(target_os = "linux", test))]
fn linux_boot_condition(
    installed: bool,
    enabled: Option<bool>,
    linger: Option<bool>,
) -> &'static str {
    match (installed, enabled, linger) {
        (false, _, _) => "service_not_installed",
        (_, Some(false), _) => "service_not_boot_enabled",
        (_, _, Some(false)) => "user_linger_required",
        (true, Some(true), Some(true)) => "configured_not_reboot_verified",
        _ => "service_manager_observation_unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_runtime_receipt_does_not_create_identity_sessions_or_enrollment() {
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        let receipt = collect_runtime_receipt(
            &paths,
            None,
            Observation::new(
                ObservationState::Unknown,
                "runtime_record_unavailable",
                "test",
            ),
            Observation::new(
                ObservationState::Unknown,
                "no_owned_publication_observed",
                "test",
            ),
            chrono::Utc::now(),
        );
        assert!(!home.path().join(".cas").exists());
        assert!(receipt.hub_id.is_none());
        assert!(receipt.current_os_boot_id.is_none());
        assert!(receipt.recorded_hub_instance_id.is_none());
        assert_eq!(receipt.factory.health.reason, "no_registered_factories");
        assert_eq!(
            receipt.independent_monitoring.state,
            ObservationState::Unknown
        );
        assert!(!receipt.boot_prerequisites.reboot_verified);
    }

    #[test]
    fn host_policy_is_read_only_defaults_on_and_keeps_malformed_config_unknown() {
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        assert_eq!(requested_publication(&paths), Some(true));
        assert!(!home.path().join(".cas").exists());
        std::fs::create_dir(home.path().join(".cas")).unwrap();
        let config = home.path().join(".cas/config.toml");
        std::fs::write(&config, "[hub]\ntailscale_serve = false\n").unwrap();
        assert_eq!(requested_publication(&paths), Some(false));
        std::fs::write(&config, "[hub]\ntailscale_serve = true\n").unwrap();
        assert_eq!(requested_publication(&paths), Some(true));
        std::fs::write(&config, "[hub]\ntailscale_serve = 'false'\n").unwrap();
        assert_eq!(requested_publication(&paths), None);
        std::fs::write(&config, "not valid TOML [").unwrap();
        assert_eq!(requested_publication(&paths), None);
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_identity_refuses_a_symlink_without_reenrolling() {
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        let store = MachineIdentityStore::new(paths.root());
        let identity = store.load_or_create().unwrap();
        let file = paths.root().join("machine-id");
        let saved = paths.root().join("existing-id");
        std::fs::rename(&file, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &file).unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read_to_string(saved).unwrap(), identity.id);
        assert!(
            std::fs::symlink_metadata(file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn healthy_hub_receipt_never_promises_public_reachability_or_rebooted_jobs() {
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        let identity = MachineIdentityStore::new(paths.root())
            .load_or_create()
            .unwrap();
        std::fs::write(
            home.path().join(".cas/config.toml"),
            "[cloud]\ntoken = 'receipt-must-not-export-this'\n",
        )
        .unwrap();
        let receipt = collect_runtime_receipt(
            &paths,
            None,
            Observation::new(
                ObservationState::Healthy,
                "loopback_health_and_lock_ready",
                "test",
            ),
            Observation::new(
                ObservationState::Failed,
                "serve_publication_unavailable",
                "test",
            ),
            chrono::Utc::now(),
        );
        assert_eq!(receipt.hub_id.as_deref(), Some(identity.id.as_str()));
        assert_eq!(receipt.hub.state, ObservationState::Healthy);
        assert_eq!(receipt.serve_publication.state, ObservationState::Failed);
        assert_eq!(
            receipt.external_reachability.state,
            ObservationState::Unknown
        );
        assert_eq!(receipt.factory.jobs_resumed, None);
        assert_eq!(receipt.factory.health.state, ObservationState::Unknown);
        let output = serde_json::to_string(&receipt).unwrap();
        assert!(!output.contains("receipt-must-not-export-this"));
        assert!(!output.contains(home.path().to_str().unwrap()));
        assert!(!home.path().join(".cas/sessions").exists());
        assert!(
            chrono::DateTime::parse_from_rfc3339(&receipt.observed_at).unwrap()
                >= chrono::DateTime::parse_from_rfc3339(&receipt.probe_window_started_at).unwrap()
        );
    }

    #[test]
    fn boot_identity_distinguishes_reboot_from_process_restart_and_hub_scope() {
        let first = scoped_identity("os-boot", "hub-a", "boot-one");
        assert_eq!(first, scoped_identity("os-boot", "hub-a", "boot-one"));
        assert_ne!(first, scoped_identity("os-boot", "hub-a", "boot-two"));
        assert_ne!(first, scoped_identity("os-boot", "hub-b", "boot-one"));
        assert_ne!(
            scoped_identity("hub-instance", "hub-a", "42:first"),
            scoped_identity("hub-instance", "hub-a", "43:second")
        );
    }

    #[test]
    fn surviving_factory_process_never_claims_resumed_jobs_or_supervisor_health() {
        let current = factory_observation(2, 1);
        assert_eq!(current.health.state, ObservationState::Unknown);
        assert_eq!(current.health.reason, "supervisor_health_unverified");
        assert_eq!(current.stale_records, 1);
        assert_eq!(current.jobs_resumed, None);
        assert_eq!(
            factory_observation(2, 0).health.state,
            ObservationState::Failed
        );
        assert_eq!(
            factory_observation(0, 0).health.state,
            ObservationState::Unknown
        );
    }

    #[test]
    fn linux_recovery_requires_persistent_enablement_and_linger() {
        assert_eq!(
            linux_boot_condition(true, Some(true), Some(true)),
            "configured_not_reboot_verified"
        );
        assert_eq!(
            linux_boot_condition(true, Some(false), Some(true)),
            "service_not_boot_enabled"
        );
        assert_eq!(
            linux_boot_condition(true, Some(true), Some(false)),
            "user_linger_required"
        );
        assert_eq!(
            linux_boot_condition(true, None, Some(true)),
            "service_manager_observation_unavailable"
        );
        assert_eq!(
            linux_boot_condition(false, None, None),
            "service_not_installed"
        );
    }

    #[test]
    fn macos_boot_identity_uses_kernel_fields_not_localized_date() {
        assert_eq!(
            parse_macos_boot_time("{ sec = 1234, usec = 56 } Mon Oct 5"),
            Some("1234:56".into())
        );
        assert_eq!(
            parse_macos_boot_time("{ sec = 1234, usec = 56 } different locale"),
            Some("1234:56".into())
        );
        assert_eq!(parse_macos_boot_time("{ sec = 0, usec = 56 }"), None);
        assert_eq!(
            parse_macos_boot_time("{ sec = 1234, usec = 1000000 }"),
            None
        );
        assert_eq!(parse_macos_boot_time("permission denied"), None);
    }

    #[cfg(unix)]
    #[test]
    fn status_diagnostic_bounds_a_hung_manager_child() {
        let started = Instant::now();
        assert!(
            bounded_output(
                "/bin/sh",
                &["-c", "exec sleep 10"],
                Duration::from_millis(50)
            )
            .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let output = bounded_output(
            "/bin/sh",
            &["-c", "printf 'Linger=yes\\n'"],
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.trim(), "Linger=yes");
    }
}
