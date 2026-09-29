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
    let candidates = launch_path(&home);
    let executable =
        find_executable(name, &candidates).ok_or(LaunchError::MissingBinary { cli: name })?;
    // The factory daemon launches workers and capability probes by bare CLI
    // name. Keep the checked supervisor binary first, then every other
    // installed provider, before the service PATH. All consumers inherit this
    // exact PATH through the PTY command environment.
    let path = provider_path(&executable, &candidates);
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
    let configured = configured_profile(cli)?;
    let profile_name = choose_profile_name(profile, configured.as_deref(), None);
    if !valid_profile_name(profile_name) {
        return Err(LaunchError::ProbeFailed {
            cli: name,
            detail: "invalid profile name".into(),
        });
    }
    let ambient_dir = if profile.is_none() && configured.is_none() {
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
        if !probe_login(cli, &executable, &set, &remove)? {
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

fn probe_login(
    cli: SupervisorCli,
    executable: &Path,
    set: &[(OsString, OsString)],
    remove: &[OsString],
) -> Result<bool, LaunchError> {
    let mut command = Command::new(executable);
    for key in remove {
        command.env_remove(key);
    }
    for (key, value) in set {
        command.env(key, value);
    }
    match cli {
        SupervisorCli::Claude => {
            command.args(["auth", "status", "--json"]);
        }
        SupervisorCli::Codex => {
            command.args(["login", "status"]);
        }
        _ => return Ok(true),
    }
    let output = bounded_output(command, Duration::from_secs(5)).map_err(|detail| {
        LaunchError::ProbeFailed {
            cli: cli_name(cli),
            detail,
        }
    })?;
    Ok(match cli {
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
        _ => true,
    })
}

fn configured_profile(cli: SupervisorCli) -> Result<Option<String>, LaunchError> {
    let config = crate::config::Config::load(&crate::store::known_repos::host_cas_dir()).map_err(
        |error| LaunchError::ProbeFailed {
            cli: cli_name(cli),
            detail: format!("host config: {error}"),
        },
    )?;
    let profiles = config.hub.and_then(|hub| hub.launch_profiles);
    Ok(match cli {
        SupervisorCli::Claude => profiles.and_then(|profiles| profiles.claude),
        SupervisorCli::Codex => profiles.and_then(|profiles| profiles.codex),
        _ => None,
    })
}

fn choose_profile_name<'a>(
    explicit: Option<&'a str>,
    configured: Option<&'a str>,
    ambient: Option<&'a str>,
) -> &'a str {
    explicit.or(configured).or(ambient).unwrap_or("main")
}

pub fn default_profile_name(cli: SupervisorCli) -> Result<String, LaunchError> {
    if let Some(configured) = configured_profile(cli)? {
        return Ok(configured);
    }
    let home = dirs::home_dir().ok_or_else(|| LaunchError::ProbeFailed {
        cli: cli_name(cli),
        detail: "home directory is unavailable".into(),
    })?;
    let key = match cli {
        SupervisorCli::Claude => "CLAUDE_CONFIG_DIR",
        SupervisorCli::Codex => "CODEX_HOME",
        _ => return Ok("default".into()),
    };
    Ok(std::env::var_os(key)
        .and_then(|dir| profile_name_from_dir(cli, &home, Path::new(&dir)))
        .unwrap_or_else(|| "main".into()))
}

fn cli_name(cli: SupervisorCli) -> &'static str {
    match cli {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        SupervisorCli::OpenCode => "opencode",
    }
}

/// Map a `cas claude` / `cas codex` selector directory to its account name.
pub fn profile_name_from_dir(cli: SupervisorCli, home: &Path, dir: &Path) -> Option<String> {
    let (main, prefix) = match cli {
        SupervisorCli::Claude => (".claude", ".claude-"),
        SupervisorCli::Codex => (".codex", ".codex-"),
        _ => return None,
    };
    let path = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        home.join(dir)
    };
    if path.parent()? != home {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if name == main {
        return Some("main".into());
    }
    let suffix = name.strip_prefix(prefix)?;
    valid_profile_name(suffix).then(|| suffix.to_string())
}

pub fn captured_profile(
    existing: Option<&str>,
    cli: SupervisorCli,
    home: &Path,
    dir: &Path,
) -> Option<String> {
    existing
        .map(str::to_owned)
        .or_else(|| profile_name_from_dir(cli, home, dir))
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct LaunchProfile {
    pub name: String,
    pub logged_in: bool,
    pub is_default: bool,
}

/// Profiles for Commander account picker; login is checked through
/// the official CLI rather than by opening any credential file.
pub fn profiles(cli: SupervisorCli) -> Result<Vec<LaunchProfile>, LaunchError> {
    let (main_dir, prefix) = match cli {
        SupervisorCli::Claude => (".claude", ".claude-"),
        SupervisorCli::Codex => (".codex", ".codex-"),
        _ => return Ok(Vec::new()),
    };
    let home = dirs::home_dir().ok_or_else(|| LaunchError::ProbeFailed {
        cli: cli_name(cli),
        detail: "home directory is unavailable".into(),
    })?;
    let default = configured_profile(cli)?
        .or_else(|| {
            let key = if cli == SupervisorCli::Claude {
                "CLAUDE_CONFIG_DIR"
            } else {
                "CODEX_HOME"
            };
            std::env::var_os(key).and_then(|dir| profile_name_from_dir(cli, &home, Path::new(&dir)))
        })
        .unwrap_or_else(|| "main".into());
    let layout = crate::cli::account_picker::ProfileLayout {
        main_dir,
        named_prefix: prefix,
        main_name: "main",
    };
    let scanned = crate::cli::account_picker::scan_profiles_with(layout, &home, None, |_, _| {
        crate::cli::account_picker::LoginState::Unknown
    })
    .map_err(|error| LaunchError::ProbeFailed {
        cli: cli_name(cli),
        detail: error.to_string(),
    })?;
    // The login-shell PATH and executable are shared by all status checks.
    let candidates = launch_path(&home);
    let executable = find_executable(cli_name(cli), &candidates)
        .ok_or(LaunchError::MissingBinary { cli: cli_name(cli) })?;
    let path = std::env::join_paths(provider_path(&executable, &candidates)).map_err(|error| {
        LaunchError::ProbeFailed {
            cli: cli_name(cli),
            detail: format!("invalid launch PATH: {error}"),
        }
    })?;
    let remove: Vec<OsString> = match cli {
        SupervisorCli::Claude => vec![
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        ],
        SupervisorCli::Codex => vec![
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "CODEX_HOME",
        ],
        _ => unreachable!(),
    }
    .into_iter()
    .map(OsString::from)
    .collect();
    let rows = std::thread::scope(|scope| {
        let handles: Vec<_> = scanned
            .into_iter()
            .map(|p| {
                let executable = &executable;
                let path = &path;
                let remove = &remove;
                let default = &default;
                scope.spawn(move || {
                    let mut set = vec![(OsString::from("PATH"), path.clone())];
                    if p.name != "main" {
                        let key = match cli {
                            SupervisorCli::Claude => "CLAUDE_CONFIG_DIR",
                            _ => "CODEX_HOME",
                        };
                        set.push((OsString::from(key), p.directory.as_os_str().to_owned()));
                        if cli == SupervisorCli::Claude {
                            set.push((
                                OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
                                p.directory.as_os_str().to_owned(),
                            ));
                        }
                    }
                    LaunchProfile {
                        is_default: p.name == default.as_str(),
                        logged_in: probe_login(cli, executable, &set, remove).unwrap_or(false),
                        name: p.name,
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("profile status thread panicked"))
            .collect()
    });
    Ok(rows)
}

fn launch_path(home: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    // A user service does not inherit the terminal's PATH. Probe a login shell
    // only for its PATH; neither its output nor any credential value is retained.
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let mut command = Command::new(shell);
    command.args([
        "-lc",
        "printf '__CAS_PATH_BEGIN__%s__CAS_PATH_END__' \"$PATH\"",
    ]);
    if let Ok(output) = bounded_output(command, Duration::from_secs(3)) {
        if output.status.success() {
            if let Some(probed) = parse_shell_path(&output.stdout) {
                paths.extend(std::env::split_paths(&OsString::from(probed)));
            }
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

fn parse_shell_path(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let start = text.rfind("__CAS_PATH_BEGIN__")? + "__CAS_PATH_BEGIN__".len();
    let tail = &text[start..];
    let end = tail.find("__CAS_PATH_END__")?;
    Some(tail[..end].to_string())
}

fn provider_path(executable: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut path = Vec::new();
    if let Some(parent) = executable.parent() {
        path.push(parent.to_path_buf());
    }
    for provider in ["claude", "codex", "grok", "opencode"] {
        if let Some(binary) = find_executable(provider, candidates) {
            if let Some(parent) = binary.parent() {
                if !path.iter().any(|existing| existing == parent) {
                    path.push(parent.to_path_buf());
                }
            }
        }
    }
    if let Some(ambient) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&ambient) {
            if directory.is_absolute() && !path.contains(&directory) {
                path.push(directory);
            }
        }
    }
    for directory in candidates {
        if !path.contains(directory) {
            path.push(directory.clone());
        }
    }
    path
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

    #[test]
    fn configured_profile_beats_ambient_and_main() {
        assert_eq!(
            choose_profile_name(None, Some("daniel@petrastella.io"), Some("other")),
            "daniel@petrastella.io"
        );
        assert_eq!(choose_profile_name(None, None, Some("other")), "other");
        assert_eq!(choose_profile_name(None, None, None), "main");
    }

    #[test]
    fn install_profile_capture_maps_paths_and_preserves_explicit_value() {
        let home = Path::new("/home/operator");
        assert_eq!(
            captured_profile(
                None,
                SupervisorCli::Claude,
                home,
                Path::new("/home/operator/.claude-x")
            ),
            Some("x".into())
        );
        assert_eq!(
            captured_profile(
                None,
                SupervisorCli::Claude,
                home,
                Path::new("/home/operator/.claude")
            ),
            Some("main".into())
        );
        assert_eq!(
            captured_profile(
                Some("chosen"),
                SupervisorCli::Claude,
                home,
                Path::new("/home/operator/.claude-x")
            ),
            Some("chosen".into())
        );
    }

    #[test]
    fn shell_path_sentinels_ignore_banner_noise() {
        assert_eq!(
            parse_shell_path(b"welcome\n__CAS_PATH_BEGIN__/a:/b__CAS_PATH_END__\nbye"),
            Some("/a:/b".into())
        );
        assert_eq!(parse_shell_path(b"welcome\n/a:/b"), None);
    }
}
