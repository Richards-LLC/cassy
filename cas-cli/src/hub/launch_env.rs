//! Resolve the service process's CLI launch environment without reading credentials.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cas_mux::SupervisorCli;

/// Environment to apply to the PTY command for a hub-started supervisor.
#[derive(Debug, Clone)]
pub struct LaunchEnvironment {
    pub executable: PathBuf,
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LaunchError {
    #[error("{cli} CLI is not installed or executable")]
    MissingBinary { cli: &'static str },
    #[error("{cli} profile is missing: {path}")]
    ProfileMissing { cli: &'static str, path: PathBuf },
    #[error("{cli} is not logged in for profile {profile}")]
    NotLoggedIn { cli: &'static str, profile: String },
    #[error("{cli} launch environment could not be resolved: {detail}")]
    ProbeFailed { cli: &'static str, detail: String },
}

/// `profile` is the same optional name accepted by `cas claude` / `cas codex`.
/// `None` uses the provider's machine-default profile.
pub fn resolve(
    cli: SupervisorCli,
    profile: Option<&str>,
) -> Result<LaunchEnvironment, LaunchError> {
    let name = match cli {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        SupervisorCli::OpenCode => "opencode",
    };
    let home = dirs::home_dir().ok_or_else(|| LaunchError::ProbeFailed {
        cli: name,
        detail: "home directory is unavailable".into(),
    })?;
    let path = launch_path(&home);
    let executable =
        find_executable(name, &path).ok_or(LaunchError::MissingBinary { cli: name })?;
    let mut set = vec![
        (
            OsString::from("PATH"),
            std::env::join_paths(&path).map_err(|error| LaunchError::ProbeFailed {
                cli: name,
                detail: format!("invalid launch PATH: {error}"),
            })?,
        ),
        (OsString::from("TERM"), OsString::from("xterm-256color")),
        (
            OsString::from("LANG"),
            std::env::var_os("LANG")
                .filter(|value| {
                    let locale = value.to_string_lossy();
                    locale != "C" && locale != "POSIX" && !locale.is_empty()
                })
                .unwrap_or_else(|| {
                    OsString::from(if cfg!(target_os = "macos") {
                        "en_US.UTF-8"
                    } else {
                        "C.UTF-8"
                    })
                }),
        ),
    ];
    let mut remove = Vec::new();
    let profile_name = profile.unwrap_or("main");
    if !valid_profile_name(profile_name) {
        return Err(LaunchError::ProbeFailed {
            cli: name,
            detail: "invalid profile name".into(),
        });
    }
    let ambient_dir = if profile.is_none() {
        match cli {
            SupervisorCli::Claude => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            SupervisorCli::Codex => std::env::var_os("CODEX_HOME").map(PathBuf::from),
            _ => None,
        }
    } else {
        None
    };
    let profile_dir = ambient_dir.or_else(|| match cli {
        SupervisorCli::Claude => Some(crate::cli::claude::resolve_profile_dir(&home, profile_name)),
        SupervisorCli::Codex => Some(crate::cli::codex::resolve_profile_dir(&home, profile_name)),
        _ => None,
    });
    if let Some(dir) = &profile_dir {
        if !dir.is_dir() {
            return Err(LaunchError::ProfileMissing {
                cli: name,
                path: dir.clone(),
            });
        }
        match cli {
            SupervisorCli::Claude => {
                remove.extend(
                    [
                        "ANTHROPIC_API_KEY",
                        "ANTHROPIC_AUTH_TOKEN",
                        "CLAUDE_CODE_OAUTH_TOKEN",
                        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
                        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
                    ]
                    .map(OsString::from),
                );
                if profile_name == "main"
                    && profile_dir
                        .as_ref()
                        .is_some_and(|p| p == &home.join(".claude"))
                {
                    remove.extend(
                        ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"]
                            .map(OsString::from),
                    );
                } else {
                    set.push(("CLAUDE_CONFIG_DIR".into(), dir.as_os_str().to_owned()));
                    set.push((
                        "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
                        dir.as_os_str().to_owned(),
                    ));
                }
            }
            SupervisorCli::Codex => {
                remove.extend(
                    ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"].map(OsString::from),
                );
                if profile_name == "main"
                    && profile_dir
                        .as_ref()
                        .is_some_and(|p| p == &home.join(".codex"))
                {
                    remove.push("CODEX_HOME".into());
                } else {
                    set.push(("CODEX_HOME".into(), dir.as_os_str().to_owned()));
                }
            }
            _ => {}
        }
    }
    if matches!(cli, SupervisorCli::Claude | SupervisorCli::Codex) {
        let mut command = Command::new(&executable);
        command.env(
            "PATH",
            std::env::join_paths(&path).map_err(|error| LaunchError::ProbeFailed {
                cli: name,
                detail: format!("invalid launch PATH: {error}"),
            })?,
        );
        for key in &remove {
            command.env_remove(key);
        }
        for (key, value) in &set {
            command.env(key, value);
        }
        match cli {
            SupervisorCli::Claude => {
                command.args(["auth", "status", "--json"]);
            }
            SupervisorCli::Codex => {
                command.args(["login", "status"]);
            }
            _ => unreachable!(),
        }
        let output = bounded_output(command, Duration::from_secs(5))
            .map_err(|detail| LaunchError::ProbeFailed { cli: name, detail })?;
        let logged_in = match cli {
            SupervisorCli::Claude => serde_json::from_slice::<serde_json::Value>(&output.stdout)
                .ok()
                .and_then(|json| json.get("loggedIn").and_then(serde_json::Value::as_bool))
                .unwrap_or(false),
            SupervisorCli::Codex => {
                output.status.success()
                    && !String::from_utf8_lossy(&output.stdout)
                        .to_ascii_lowercase()
                        .contains("not logged in")
                    && !String::from_utf8_lossy(&output.stderr)
                        .to_ascii_lowercase()
                        .contains("not logged in")
            }
            _ => unreachable!(),
        };
        if !logged_in {
            return Err(LaunchError::NotLoggedIn {
                cli: name,
                profile: profile_dir
                    .as_ref()
                    .map_or_else(|| profile_name.into(), |p| p.display().to_string()),
            });
        }
    }
    Ok(LaunchEnvironment {
        executable,
        set,
        remove,
    })
}

fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

/// Operator-facing readiness for each supported Commander launch CLI.
pub fn readiness() -> Vec<(&'static str, Result<PathBuf, LaunchError>)> {
    [
        ("claude", SupervisorCli::Claude),
        ("codex", SupervisorCli::Codex),
        ("grok", SupervisorCli::Grok),
    ]
    .into_iter()
    .map(|(name, cli)| (name, resolve(cli, None).map(|env| env.executable)))
    .collect()
}

fn launch_path(home: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    // A user service does not inherit the terminal's PATH. Probe a login shell
    // only for its PATH; neither its output nor any credential value is retained.
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let mut command = Command::new(shell);
    command.args(["-lc", "printf '%s' \"$PATH\""]);
    if let Ok(output) = bounded_output(command, Duration::from_secs(3)) {
        if output.status.success() {
            paths.extend(std::env::split_paths(&OsString::from(
                String::from_utf8_lossy(&output.stdout).as_ref(),
            )));
        }
    }
    if let Some(ambient) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&ambient));
    }
    for suffix in [
        ".local/bin",
        ".npm-global/bin",
        ".volta/bin",
        ".asdf/shims",
        ".local/share/mise/shims",
        ".bun/bin",
    ] {
        paths.push(home.join(suffix));
    }
    for prefix in [
        home.join(".nvm/versions/node"),
        home.join(".local/share/fnm/node-versions"),
    ] {
        if let Ok(entries) = std::fs::read_dir(prefix) {
            for entry in entries.flatten() {
                paths.push(entry.path().join("bin"));
                paths.push(entry.path().join("installation/bin"));
            }
        }
    }
    paths.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    paths.retain(|p| p.is_absolute());
    paths.dedup();
    paths
}

fn find_executable(name: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().map(|dir| dir.join(name)).find(|path| {
        path.is_file() && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                path.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    })
}

fn bounded_output(mut command: Command, timeout: Duration) -> Result<std::process::Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|error| error.to_string()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("CLI status probe timed out".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_cannot_escape_the_provider_home() {
        for value in ["../other", "a/b", "a\\b", "", ".", ".."] {
            assert!(!valid_profile_name(value), "{value:?}");
        }
        assert!(valid_profile_name("daniel@petrastella.io"));
    }
}
