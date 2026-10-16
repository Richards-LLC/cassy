//! Advisory only: native Codex 0.162.1 continues after PreToolUse timeouts.
use crate::PtyConfig;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn known_fail_open(version: &str) -> bool {
    // Audited against rust-v0.162.1, hooks/src/events/pre_tool.rs and
    // core/src/tools/hook_runtime.rs. Do not guess that later versions fixed it.
    version.split_whitespace().any(|part| part == "0.162.1")
}

fn probe(executable: &Path, budget: Duration) -> Option<String> {
    let deadline = Instant::now() + budget;
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new()
        .spawn(move || {
            let mut text = String::new();
            let result = stdout.take(256).read_to_string(&mut text).map(|_| text);
            let _ = sender.send(result);
        })
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status
                    .success()
                    .then(|| {
                        receiver
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .ok()?
                            .ok()
                    })
                    .flatten();
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

pub(crate) fn warn_at_launch(config: &PtyConfig) {
    let path = config
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| std::ffi::OsString::from(value))
        .or_else(|| std::env::var_os("PATH"));
    if config.env_remove.iter().any(|key| key == "PATH") {
        return;
    }
    let Some(path) = path else {
        return;
    };
    let executable = std::env::split_paths(&path)
        .map(|dir| {
            let dir = if dir.is_relative() {
                config.cwd.as_ref().map(|cwd| cwd.join(&dir)).unwrap_or(dir)
            } else {
                dir
            };
            dir.join("codex")
        })
        .find(|path: &PathBuf| path.is_file());
    let Some(executable) = executable else {
        return;
    };
    if let Some(version) =
        probe(&executable, Duration::from_secs(2)).filter(|version| known_fail_open(version))
    {
        tracing::warn!(version = %version.trim(), "Codex PreToolUse hook timeouts fail open: Cassy raises its hook budget to 120s and bounds Cargo policy validation; native hook timeout enforcement still requires an upstream fail-closed option. Worker launch continues.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn launch_probe_is_bounded_and_soft_on_errors_cas_2c8e() {
        use std::os::unix::fs::PermissionsExt;
        assert!(!known_fail_open("codex-cli 0.163.0"));
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("codex");
        std::fs::write(&cli, "#!/bin/sh\necho codex-cli 0.162.1\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(known_fail_open(
            &probe(&cli, Duration::from_secs(1)).unwrap()
        ));
        std::fs::write(&cli, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(probe(&cli, Duration::from_secs(1)).is_none());
        std::fs::write(&cli, "#!/bin/sh\nexec sleep 5\n").unwrap();
        assert!(probe(&cli, Duration::from_millis(30)).is_none());
        assert!(probe(&root.path().join("missing"), Duration::from_secs(1)).is_none());
    }
}
