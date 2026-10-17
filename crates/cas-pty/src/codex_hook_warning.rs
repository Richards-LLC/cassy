//! Contextual Codex feature evidence and the retained hook-timeout advisory.
use crate::PtyConfig;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A feature declaration, not proof of trust, tool visibility or authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Disabled,
    Unknown,
}

/// Evidence from the same cwd, home, environment and configuration overrides
/// as a pending launch. Never reuse this for another account or launch.
#[derive(Debug, Clone, Serialize)]
pub struct CodexCapabilityEvidence {
    pub executable: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub version: Option<String>,
    pub hooks: CapabilityState,
    pub multi_agent_v1: CapabilityState,
    pub multi_agent_v2: CapabilityState,
    pub native_dispatch: CapabilityState,
    pub native_dispatch_reason: &'static str,
}

fn effective_env(config: &PtyConfig, name: &str) -> Option<std::ffi::OsString> {
    if config.env_remove.iter().any(|key| key == name) {
        return None;
    }
    config
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into())
        .or_else(|| std::env::var_os(name))
}

fn executable_in(config: &PtyConfig) -> Option<PathBuf> {
    let command = Path::new(&config.command);
    if command.is_absolute() {
        return command.is_file().then(|| command.to_path_buf());
    }
    if command.components().count() > 1 {
        let path = config.cwd.as_ref()?.join(command);
        return path.is_file().then_some(path);
    }
    std::env::split_paths(&effective_env(config, "PATH")?)
        .map(|dir| {
            let dir = if dir.is_relative() {
                config.cwd.as_ref().map(|cwd| cwd.join(&dir)).unwrap_or(dir)
            } else {
                dir
            };
            dir.join(command)
        })
        .find(|path| path.is_file())
}

fn config_overrides(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if matches!(
            arg.as_str(),
            "-c" | "--config" | "--enable" | "--disable" | "-p" | "--profile"
        ) {
            out.push(arg.clone());
            if let Some(value) = args.next() {
                out.push(value.clone());
            }
        } else if ["--config=", "--enable=", "--disable=", "--profile="]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        {
            out.push(arg.clone());
        }
    }
    out
}

fn feature_state(output: &str, name: &str) -> CapabilityState {
    let mut matches = output.lines().filter_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first().copied() != Some(name) {
            return None;
        }
        Some(match fields.as_slice() {
            [_, "stable", "true"] => CapabilityState::Supported,
            [_, "stable", "false"] => CapabilityState::Disabled,
            _ => CapabilityState::Unknown,
        })
    });
    let value = matches.next().unwrap_or(CapabilityState::Unknown);
    if matches.next().is_some() {
        CapabilityState::Unknown
    } else {
        value
    }
}

/// Probe once at actual launch/preflight, never from transport/policy getters.
/// `features list` resolves native config layering. It does not expose the
/// model-resolved tool surface: V2, agents.enabled and model policy interact,
/// so native dispatch remains unknown until separately proven by invocation.
pub fn detect_codex_capabilities(config: &PtyConfig) -> CodexCapabilityEvidence {
    detect_with_budget(config, Duration::from_secs(2))
}

fn detect_with_budget(config: &PtyConfig, budget: Duration) -> CodexCapabilityEvidence {
    let mut evidence = CodexCapabilityEvidence {
        executable: executable_in(config),
        cwd: config.cwd.clone(),
        codex_home: effective_env(config, "CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                effective_env(config, "HOME")
                    .or_else(|| effective_env(config, "USERPROFILE"))
                    .map(|home| PathBuf::from(home).join(".codex"))
            }),
        version: None,
        hooks: CapabilityState::Unknown,
        multi_agent_v1: CapabilityState::Unknown,
        multi_agent_v2: CapabilityState::Unknown,
        native_dispatch: CapabilityState::Unknown,
        native_dispatch_reason: "Feature declarations do not prove model-resolved native tools or role invocation; use the registered independent reviewer route.",
    };
    let Some(executable) = evidence.executable.as_ref() else {
        return evidence;
    };
    if evidence.cwd.is_none() || evidence.codex_home.is_none() {
        return evidence;
    }
    let deadline = Instant::now() + budget;
    evidence.version = probe_in(executable, config, &["--version".to_string()], budget)
        .map(|version| version.trim().to_string());
    // The feature vocabulary is pinned to the reviewed 0.163.0 schema. A
    // different or failed version probe keeps the optional capabilities unknown.
    if evidence.version.as_deref() != Some("codex-cli 0.163.0") {
        return evidence;
    }
    let mut args = config_overrides(&config.args);
    args.extend(["features".to_string(), "list".to_string()]);
    if let Some(output) = probe_in(
        executable,
        config,
        &args,
        deadline.saturating_duration_since(Instant::now()),
    ) {
        evidence.hooks = feature_state(&output, "hooks");
        evidence.multi_agent_v1 = feature_state(&output, "multi_agent");
        evidence.multi_agent_v2 = feature_state(&output, "multi_agent_v2");
    }
    evidence
}

fn known_fail_open(version: &str) -> bool {
    // Audited against rust-v0.162.1, hooks/src/events/pre_tool.rs and
    // core/src/tools/hook_runtime.rs. Do not guess that later versions fixed it.
    version.split_whitespace().any(|part| part == "0.162.1")
}

fn probe(executable: &Path, budget: Duration) -> Option<String> {
    probe_in(
        executable,
        &PtyConfig::default(),
        &["--version".to_string()],
        budget,
    )
}

fn probe_in(
    executable: &Path,
    config: &PtyConfig,
    args: &[String],
    budget: Duration,
) -> Option<String> {
    if budget.is_zero() {
        return None;
    }
    let deadline = Instant::now() + budget;
    let mut command = Command::new(executable);
    command
        .args(args)
        .envs(config.env.iter().map(|(key, value)| (key, value)));
    for name in &config.env_remove {
        command.env_remove(name);
    }
    if let Some(cwd) = &config.cwd {
        command.current_dir(cwd);
    }
    let mut child = command
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
            let result = stdout
                .take(65_537)
                .read_to_string(&mut text)
                .ok()
                .filter(|_| text.len() <= 65_536)
                .map(|_| text);
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
                            .ok()
                            .flatten()
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
    fn contextual_capabilities_preserve_launch_context_without_claiming_dispatch() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        let home = root.path().join("profile");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let cli = root.path().join("codex");
        let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo 'codex-cli 0.163.0'; exit 0; fi
[ "$PWD" = "$EXPECTED_CWD" ] || exit 4
[ "$CODEX_HOME" = "$EXPECTED_HOME" ] || exit 5
[ -z "${REMOVED_PROBE_VAR+x}" ] || exit 6
[ "$1" = "-c" ] && [ "$2" = "features.hooks=false" ] || exit 7
[ "$3" = "features" ] && [ "$4" = "list" ] || exit 8
echo 'hooks stable false'
echo 'multi_agent stable true'
echo 'multi_agent_v2 stable false'
"#;
        std::fs::write(&cli, script).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = PtyConfig {
            command: cli.to_string_lossy().into_owned(),
            cwd: Some(cwd.clone()),
            args: vec!["-c".to_string(), "features.hooks=false".to_string()],
            env: vec![
                (
                    "CODEX_HOME".to_string(),
                    home.to_string_lossy().into_owned(),
                ),
                (
                    "EXPECTED_HOME".to_string(),
                    home.to_string_lossy().into_owned(),
                ),
                (
                    "EXPECTED_CWD".to_string(),
                    cwd.to_string_lossy().into_owned(),
                ),
                ("REMOVED_PROBE_VAR".to_string(), "removed".to_string()),
            ],
            env_remove: vec!["REMOVED_PROBE_VAR".to_string()],
            ..PtyConfig::default()
        };
        let evidence = detect_codex_capabilities(&config);
        assert_eq!(evidence.cwd.as_ref(), Some(&cwd));
        assert_eq!(evidence.codex_home.as_ref(), Some(&home));
        assert_eq!(evidence.hooks, CapabilityState::Disabled);
        assert_eq!(evidence.multi_agent_v1, CapabilityState::Supported);
        assert_eq!(evidence.multi_agent_v2, CapabilityState::Disabled);
        assert_eq!(evidence.native_dispatch, CapabilityState::Unknown);

        for script in [
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.163.0; else echo malformed; fi\n",
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.163.0; else exit 1; fi\n",
            "#!/bin/sh\necho codex-cli 0.999.0\n",
        ] {
            std::fs::write(&cli, script).unwrap();
            let evidence = detect_codex_capabilities(&config);
            assert_eq!(evidence.hooks, CapabilityState::Unknown);
            assert_eq!(evidence.multi_agent_v1, CapabilityState::Unknown);
            assert_eq!(evidence.native_dispatch, CapabilityState::Unknown);
        }
        std::fs::remove_file(&cli).unwrap();
        assert_eq!(
            detect_codex_capabilities(&config).hooks,
            CapabilityState::Unknown
        );
    }

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
