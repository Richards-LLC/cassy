//! Lifecycle-safe Tailscale Serve integration for Commander.
//!
//! Design-port of the command/status shape in pingdotgg/t3code's
//! `packages/tailscale/src/tailscale.ts` (MIT). CAS keeps its own ownership
//! receipt and never resets or replaces an unrelated Serve configuration.

use std::ffi::{OsStr, OsString};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bounded_process::{self, BoundedCommandError, Deadline};

#[cfg(test)]
fn private_tempdir() -> tempfile::TempDir {
    let parent = std::env::temp_dir()
        .canonicalize()
        .expect("temporary directory must be canonicalizable");
    tempfile::tempdir_in(parent).expect("canonical temporary fixture directory")
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const RECEIPT_FILE: &str = "tailscale-serve.json";
const TEARDOWN_RECEIPT_FILE: &str = "tailscale-serve-teardown.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TailscaleServeReceipt {
    pub schema_version: u32,
    pub public_url: String,
    pub local_target: String,
    pub https_port: u16,
    pub created_by_cas: bool,
    pub executable: String,
    pub status_before: Value,
    pub status_after: Value,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TailscaleTeardownReceipt {
    schema_version: u32,
    public_url: String,
    local_target: String,
    https_port: u16,
    status_before: Value,
    status_after: Value,
    recorded_at: String,
}

#[derive(Debug, Clone)]
pub struct TailscaleServeManager {
    executable: OsString,
    state_dir: PathBuf,
}

impl TailscaleServeManager {
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self::with_executable(state_dir, tailscale_executable())
    }

    pub fn with_executable(state_dir: impl AsRef<Path>, executable: impl AsRef<OsStr>) -> Self {
        Self {
            executable: executable.as_ref().to_owned(),
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    pub fn executable_display(&self) -> String {
        self.executable.to_string_lossy().into_owned()
    }

    pub fn ensure(&self, local_port: u16, https_port: u16) -> Result<TailscaleServeReceipt> {
        let local_target = format!("http://127.0.0.1:{local_port}");
        let status = self.status()?;
        let dns_name = status
            .get("Self")
            .and_then(|value| value.get("DNSName"))
            .and_then(Value::as_str)
            .map(|name| name.trim_end_matches('.'))
            .filter(|name| valid_dns_name(name))
            .context("tailscale is not logged in or has no MagicDNS name")?;
        let public_url = if https_port == 443 {
            format!("https://{dns_name}/")
        } else {
            format!("https://{dns_name}:{https_port}/")
        };
        let before = self.serve_status()?;
        let handlers = handlers_on_port(&before, https_port);
        let created_by_cas = if handlers.is_empty() {
            self.run(&[
                "serve".into(),
                "--bg".into(),
                "--yes".into(),
                format!("--https={https_port}"),
                local_target.clone(),
            ])?;
            true
        } else {
            anyhow::ensure!(
                handlers == vec![("/".to_owned(), local_target.clone())],
                "tailscale Serve HTTPS port {https_port} is already owned by another mapping"
            );
            false
        };
        let finish = || -> Result<TailscaleServeReceipt> {
            let after = self.serve_status()?;
            anyhow::ensure!(
                handlers_on_port(&after, https_port)
                    == vec![("/".to_owned(), local_target.clone())],
                "tailscale Serve did not install the requested CAS proxy"
            );
            let receipt = TailscaleServeReceipt {
                schema_version: 1,
                public_url,
                local_target: local_target.clone(),
                https_port,
                created_by_cas,
                executable: self.executable_display(),
                status_before: before,
                status_after: after,
                recorded_at: chrono::Utc::now().to_rfc3339(),
            };
            if created_by_cas {
                write_private_json(&self.state_dir.join(RECEIPT_FILE), &receipt)?;
            }
            Ok(receipt)
        };
        match finish() {
            Ok(receipt) => Ok(receipt),
            Err(error) if created_by_cas => {
                match self.rollback_created(https_port, &local_target) {
                    Ok(()) => Err(error.context("Tailscale Serve setup rolled back")),
                    Err(rollback) => Err(error.context(format!(
                        "Tailscale Serve setup failed and rollback was refused: {rollback}"
                    ))),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn rollback_created(&self, https_port: u16, local_target: &str) -> Result<()> {
        let current = self.serve_status()?;
        let handlers = handlers_on_port(&current, https_port);
        if handlers.is_empty() {
            remove_receipt_if_present(&self.state_dir.join(RECEIPT_FILE))?;
            return Ok(());
        }
        anyhow::ensure!(
            handlers == vec![("/".to_owned(), local_target.to_owned())],
            "Tailscale Serve mapping changed during setup; leaving it untouched"
        );
        self.run(&[
            "serve".into(),
            format!("--https={https_port}"),
            "off".into(),
        ])?;
        anyhow::ensure!(
            handlers_on_port(&self.serve_status()?, https_port).is_empty(),
            "Tailscale Serve rollback did not remove the CAS mapping"
        );
        remove_receipt_if_present(&self.state_dir.join(RECEIPT_FILE))?;
        Ok(())
    }

    /// Disable only a mapping CAS created and only while it is still unchanged.
    pub fn disable_owned(&self) -> Result<Option<TailscaleServeReceipt>> {
        let path = self.state_dir.join(RECEIPT_FILE);
        let Some(receipt) = self.owned_receipt()? else {
            return Ok(None);
        };
        let before = self.serve_status()?;
        anyhow::ensure!(
            handlers_on_port(&before, receipt.https_port)
                == vec![("/".to_owned(), receipt.local_target.clone())],
            "Tailscale Serve mapping changed since CAS created it; leaving it untouched"
        );
        self.run(&[
            "serve".into(),
            format!("--https={}", receipt.https_port),
            "off".into(),
        ])?;
        let after = self.serve_status()?;
        anyhow::ensure!(
            handlers_on_port(&after, receipt.https_port).is_empty(),
            "tailscale Serve did not remove the CAS mapping"
        );
        let teardown = TailscaleTeardownReceipt {
            schema_version: 1,
            public_url: receipt.public_url.clone(),
            local_target: receipt.local_target.clone(),
            https_port: receipt.https_port,
            status_before: before,
            status_after: after,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        write_private_json(&self.state_dir.join(TEARDOWN_RECEIPT_FILE), &teardown)?;
        fs::remove_file(path)?;
        Ok(Some(receipt))
    }

    /// Load CAS's current ownership receipt, if there is one.
    pub fn owned_receipt(&self) -> Result<Option<TailscaleServeReceipt>> {
        let path = self.state_dir.join(RECEIPT_FILE);
        let receipt: TailscaleServeReceipt = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("invalid CAS Tailscale receipt")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            receipt.created_by_cas,
            "CAS does not own this Tailscale mapping"
        );
        Ok(Some(receipt))
    }

    /// Check whether the exact mapping recorded in an ownership receipt is gone.
    pub fn mapping_is_absent(&self, receipt: &TailscaleServeReceipt) -> Result<bool> {
        Ok(handlers_on_port(&self.serve_status()?, receipt.https_port).is_empty())
    }

    fn status(&self) -> Result<Value> {
        self.run_json(&["status".into(), "--json".into()])
    }

    fn serve_status(&self) -> Result<Value> {
        self.run_json(&["serve".into(), "status".into(), "--json".into()])
    }

    fn run_json(&self, args: &[String]) -> Result<Value> {
        let output = self.run(args)?;
        serde_json::from_slice(&output).context("tailscale returned invalid status JSON")
    }

    fn run(&self, args: &[String]) -> Result<Vec<u8>> {
        let mut command = Command::new(&self.executable);
        command.args(args);
        let output = bounded_process::run_command(
            &mut command,
            Deadline::after(COMMAND_TIMEOUT),
            COMMAND_TIMEOUT,
        )
        .map_err(|error| match error {
            BoundedCommandError::TimedOut => anyhow::anyhow!("tailscale command timed out"),
            BoundedCommandError::Io => anyhow::anyhow!(
                "tailscale CLI is unavailable (install Tailscale and sign in first)"
            ),
        })?;
        anyhow::ensure!(
            output.status.success(),
            "tailscale command refused ({})",
            classify_stderr(&output.stderr)
        );
        Ok(output.stdout)
    }
}

fn tailscale_executable() -> OsString {
    if cfg!(windows) {
        return OsString::from("tailscale.exe");
    }
    #[cfg(target_os = "macos")]
    let bundle_paths = [
        Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        Path::new("/Applications/Tailscale.app/Contents/MacOS/tailscale"),
    ];
    #[cfg(not(target_os = "macos"))]
    let bundle_paths: [&Path; 0] = [];
    select_tailscale_executable(executable_on_path("tailscale"), &bundle_paths)
}

fn select_tailscale_executable(path_cli: Option<PathBuf>, bundle_paths: &[&Path]) -> OsString {
    if let Some(path) = path_cli {
        return path.into_os_string();
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = bundle_paths.iter().copied().find(|path| app_bundle_cli_responds(path)) {
        return path.as_os_str().to_owned();
    }
    OsString::from("tailscale")
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(target_os = "macos")]
fn app_bundle_cli_responds(path: &Path) -> bool {
    Command::new(path)
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn valid_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn classify_stderr(stderr: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if text.contains("not logged in") || text.contains("logged out") {
        "not-logged-in"
    } else if text.contains("permission denied") || text.contains("access denied") {
        "permission-denied"
    } else if text.contains("serve is not enabled") {
        "serve-not-enabled"
    } else {
        "unknown"
    }
}

fn handlers_on_port(status: &Value, port: u16) -> Vec<(String, String)> {
    let mut handlers = Vec::new();
    collect_web_handlers(status, port, &mut handlers);
    handlers.sort();
    handlers.dedup();
    handlers
}

fn collect_web_handlers(value: &Value, port: u16, output: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(web)) = object.get("Web") {
                for (host_port, entry) in web {
                    if host_port.rsplit_once(':').and_then(|(_, p)| p.parse().ok()) != Some(port) {
                        continue;
                    }
                    let Some(Value::Object(handlers)) = entry.get("Handlers") else {
                        continue;
                    };
                    for (path, handler) in handlers {
                        if let Some(proxy) = handler.get("Proxy").and_then(Value::as_str) {
                            output.push((path.clone(), proxy.trim_end_matches('/').to_owned()));
                        } else {
                            output.push((path.clone(), "<non-proxy>".to_owned()));
                        }
                    }
                }
            }
            for child in object.values() {
                collect_web_handlers(child, port, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_web_handlers(child, port, output);
            }
        }
        _ => {}
    }
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("Tailscale receipt has no parent")?;
    super::ensure_private_dir(parent)?;
    let temporary = parent.join(format!(".tailscale-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn remove_receipt_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_handlers_on_requested_https_port() {
        let status = serde_json::json!({
            "Web": {
                "host.tail.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4173"}}},
                "host.tail.ts.net:8443": {"Handlers": {"/other": {"Proxy": "http://127.0.0.1:9000"}}}
            }
        });
        assert_eq!(
            handlers_on_port(&status, 443),
            vec![("/".into(), "http://127.0.0.1:4173".into())]
        );
    }

    #[test]
    fn path_cli_precedes_bundle_cli() {
        let path_cli = PathBuf::from("/operator/tailscale");
        let selected = select_tailscale_executable(Some(path_cli.clone()), &[]);
        assert_eq!(selected, path_cli.into_os_string());
    }

    #[test]
    fn no_cli_keeps_existing_unavailable_command() {
        assert_eq!(select_tailscale_executable(None, &[]), OsString::from("tailscale"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundle_cli_is_invoked_at_its_absolute_path() {
        use std::os::unix::fs::PermissionsExt;

        let temp = private_tempdir();
        let app_cli = temp.path().join("Tailscale.app/Contents/MacOS/Tailscale");
        fs::create_dir_all(app_cli.parent().unwrap()).unwrap();
        fs::write(&app_cli, "#!/bin/sh\n[ \"$1\" = version ]\n").unwrap();
        fs::set_permissions(&app_cli, fs::Permissions::from_mode(0o700)).unwrap();

        let selected = select_tailscale_executable(None, &[&app_cli]);
        assert_eq!(selected, app_cli.into_os_string());
    }

    #[test]
    fn absent_tailscale_has_clear_sanitized_refusal() {
        let temp = private_tempdir();
        let manager = TailscaleServeManager::with_executable(
            temp.path().join("hub"),
            temp.path().join("definitely-not-installed"),
        );
        let error = manager.ensure(4173, 443).unwrap_err().to_string();
        assert!(error.contains("tailscale CLI is unavailable"));
        assert!(!error.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn mocked_binary_proves_idempotent_setup_and_owned_teardown() {
        use std::os::unix::fs::PermissionsExt;

        let temp = private_tempdir();
        let binary = temp.path().join("tailscale");
        let calls = temp.path().join("calls");
        let state = temp.path().join("serve-state");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n'status --json') printf '%s' '{{\"Self\":{{\"DNSName\":\"node.tail.ts.net.\"}}}}' ;;\n'serve status --json') if [ -f '{}' ]; then printf '%s' '{{\"Web\":{{\"node.tail.ts.net:443\":{{\"Handlers\":{{\"/\":{{\"Proxy\":\"http://127.0.0.1:4173\"}}}}}}}}}}'; else printf '%s' '{{}}'; fi ;;\n'serve --bg --yes --https=443 http://127.0.0.1:4173') touch '{}' ;;\n'serve --https=443 off') rm -f '{}' ;;\n*) exit 9 ;;\nesac\n",
            calls.display(),
            state.display(),
            state.display(),
            state.display()
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let manager = TailscaleServeManager::with_executable(temp.path().join("hub"), &binary);

        let first = manager.ensure(4173, 443).unwrap();
        assert!(first.created_by_cas);
        let second = manager.ensure(4173, 443).unwrap();
        assert!(!second.created_by_cas);
        assert_eq!(first.public_url, "https://node.tail.ts.net/");
        assert!(manager.disable_owned().unwrap().is_some());

        let calls = fs::read_to_string(calls).unwrap();
        assert_eq!(calls.matches("serve --bg").count(), 1);
        assert_eq!(calls.matches("serve --https=443 off").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn mocked_binary_refuses_unrelated_mapping_without_mutation() {
        use std::os::unix::fs::PermissionsExt;

        let temp = private_tempdir();
        let binary = temp.path().join("tailscale");
        let calls = temp.path().join("calls");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n'status --json') printf '%s' '{{\"Self\":{{\"DNSName\":\"node.tail.ts.net.\"}}}}' ;;\n'serve status --json') printf '%s' '{{\"Web\":{{\"node.tail.ts.net:443\":{{\"Handlers\":{{\"/\":{{\"Proxy\":\"http://127.0.0.1:9999\"}}}}}}}}}}' ;;\n*) exit 9 ;;\nesac\n",
            calls.display()
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let manager = TailscaleServeManager::with_executable(temp.path().join("hub"), &binary);

        let error = manager.ensure(4173, 443).unwrap_err().to_string();
        assert!(error.contains("already owned"));
        assert!(!fs::read_to_string(calls).unwrap().contains("serve --bg"));
    }

    #[cfg(unix)]
    #[test]
    fn failed_post_creation_verification_rolls_back_without_receipt() {
        use std::os::unix::fs::PermissionsExt;

        let temp = private_tempdir();
        let binary = temp.path().join("tailscale");
        let calls = temp.path().join("calls");
        let count = temp.path().join("status-count");
        let state = temp.path().join("serve-state");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n'status --json') printf '%s' '{{\"Self\":{{\"DNSName\":\"node.tail.ts.net.\"}}}}' ;;\n'serve status --json') n=0; [ -f '{}' ] && n=$(/bin/cat '{}'); n=$((n+1)); printf '%s' \"$n\" > '{}'; case \"$n\" in 1|2|4) printf '%s' '{{}}' ;; 3) printf '%s' '{{\"Web\":{{\"node.tail.ts.net:443\":{{\"Handlers\":{{\"/\":{{\"Proxy\":\"http://127.0.0.1:4173\"}}}}}}}}}}' ;; esac ;;\n'serve --bg --yes --https=443 http://127.0.0.1:4173') touch '{}' ;;\n'serve --https=443 off') rm -f '{}' ;;\n*) exit 9 ;;\nesac\n",
            calls.display(),
            count.display(),
            count.display(),
            count.display(),
            state.display(),
            state.display(),
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let hub = temp.path().join("hub");
        let manager = TailscaleServeManager::with_executable(&hub, &binary);

        let error = manager.ensure(4173, 443).unwrap_err().to_string();
        assert!(error.contains("setup rolled back"), "{error}");
        let calls = fs::read_to_string(calls).unwrap();
        assert_eq!(calls.matches("serve --bg").count(), 1);
        assert_eq!(calls.matches("serve --https=443 off").count(), 1);
        assert!(!state.exists());
        assert!(!hub.join(RECEIPT_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn owned_teardown_refuses_externally_altered_mapping_and_keeps_receipt() {
        use std::os::unix::fs::PermissionsExt;

        let temp = private_tempdir();
        let binary = temp.path().join("tailscale");
        let calls = temp.path().join("calls");
        let state = temp.path().join("serve-state");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n'status --json') printf '%s' '{{\"Self\":{{\"DNSName\":\"node.tail.ts.net.\"}}}}' ;;\n'serve status --json') if [ -f '{}' ]; then if grep -q 9999 '{}'; then printf '%s' '{{\"Web\":{{\"node.tail.ts.net:443\":{{\"Handlers\":{{\"/\":{{\"Proxy\":\"http://127.0.0.1:9999\"}}}}}}}}}}'; else printf '%s' '{{\"Web\":{{\"node.tail.ts.net:443\":{{\"Handlers\":{{\"/\":{{\"Proxy\":\"http://127.0.0.1:4173\"}}}}}}}}}}'; fi; else printf '%s' '{{}}'; fi ;;\n'serve --bg --yes --https=443 http://127.0.0.1:4173') printf '%s' 4173 > '{}' ;;\n'serve --https=443 off') rm -f '{}' ;;\n*) exit 9 ;;\nesac\n",
            calls.display(),
            state.display(),
            state.display(),
            state.display(),
            state.display(),
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let hub = temp.path().join("hub");
        let manager = TailscaleServeManager::with_executable(&hub, &binary);

        assert!(manager.ensure(4173, 443).unwrap().created_by_cas);
        fs::write(&state, "9999").unwrap();
        assert_eq!(
            handlers_on_port(&manager.serve_status().unwrap(), 443),
            vec![("/".into(), "http://127.0.0.1:9999".into())],
            "the mock must expose the external mapping before teardown checks it"
        );
        let error = manager.disable_owned().unwrap_err().to_string();
        assert!(error.contains("mapping changed"), "{error}");
        assert!(hub.join(RECEIPT_FILE).exists());
        fs::remove_file(&state).unwrap();
        let stale = manager.disable_owned().unwrap_err().to_string();
        assert!(stale.contains("mapping changed"), "{stale}");
        assert!(hub.join(RECEIPT_FILE).exists());
        assert!(
            !fs::read_to_string(calls)
                .unwrap()
                .contains("serve --https=443 off")
        );
    }
}
