//! The environment direct harnesses and update probes actually receive.
//! Private-file fallback is read-only and never executes shell text.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn canonical_credential(name: &str) -> bool {
    name == "VIOLET_VERCEL_BYPASS"
        || name == "VIOLET_SLACK_TOKEN"
        || (name.starts_with("VIOLET_SLACK_TOKEN_") && name != "VIOLET_SLACK_TOKEN_ENV")
}

pub fn credential_file() -> Option<PathBuf> {
    std::env::var_os("CAS_CREDENTIALS_FILE")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|p| !p.is_empty())
                .map(|p| PathBuf::from(p).join("cas/credentials.env"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/cas/credentials.env"))
        })
}

pub fn literal_assignment(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let line = line.strip_prefix("export ").unwrap_or(line).trim();
    let (name, value) = line.split_once('=')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
    {
        return None;
    }
    // Shell comments start only at an unquoted # following whitespace.
    let mut quote = None;
    let mut escaped = false;
    let mut previous_space = true;
    let mut end = value.len();
    for (i, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            previous_space = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch == '#' && previous_space {
            end = i;
            break;
        }
        previous_space = ch.is_whitespace();
    }
    let value = value[..end].trim();
    if value.contains(['$', '`']) {
        return None;
    }
    let value = if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].replace("\\\"", "\"")
    } else {
        value.into()
    };
    Some((name.into(), value))
}

fn read_literal_file(path: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    const LIMIT: u64 = 1024 * 1024;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(std::io::Error::other("unsafe credentials file"));
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::other("unsafe credentials file"));
    }
    let mut text = String::new();
    file.take(LIMIT).read_to_string(&mut text)?;
    Ok(text)
}

pub fn literal_file_values(path: &Path) -> BTreeMap<String, String> {
    match read_literal_file(path) {
        Ok(text) => text.lines().filter_map(literal_assignment).collect(),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn_once(&format!("credentials:{}", path.display()), || {
                    tracing::warn!(path = %path.display(), "Violet credentials file refused; require a regular file owned by the current user with no group/other permissions and at most 1 MiB");
                });
            }
            BTreeMap::new()
        }
    }
}

/// CLI launch/update diagnostics are visible even without a tracing subscriber.
/// The daemon's private-file resolver retains its tracing-only warning path.
pub fn warn_credentials_file_for_cli() -> Option<(PathBuf, String)> {
    let path = credential_file()?;
    let error = read_literal_file(&path).err()?;
    if error.kind() == std::io::ErrorKind::NotFound {
        return None;
    }
    let quoted = path.to_string_lossy().replace('\'', "'\\''");
    let warning = format!(
        "Violet credentials file refused: {}; require a regular file with current-user ownership, no group/other permissions and at most 1 MiB; correct the owner if needed, then run chmod 600 '{quoted}'",
        path.display()
    );
    // Avoid an additional tracing notice after printing the CLI remedy.
    warn_once(&format!("credentials:{}", path.display()), || {});
    warn_once(&format!("cli-credentials:{}", path.display()), || {
        eprintln!("warning: {warning}")
    });
    Some((path, warning))
}

fn warn_once(key: &str, warn: impl FnOnce()) {
    static WARNED: std::sync::OnceLock<std::sync::Mutex<BTreeSet<String>>> =
        std::sync::OnceLock::new();
    if WARNED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.into())
    {
        warn();
    }
}

pub fn resolved_environment(
    overrides: &[(String, String)],
    removed: &[String],
) -> BTreeMap<String, String> {
    let mut env: BTreeMap<_, _> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    env.extend(overrides.iter().cloned());
    for name in removed {
        env.remove(name);
    }
    if let Some(path) = credential_file() {
        for (name, value) in literal_file_values(&path) {
            if canonical_credential(&name)
                && !removed.contains(&name)
                && !value.trim().is_empty()
                && !env.get(&name).is_some_and(|v| !v.trim().is_empty())
            {
                env.insert(name, value);
            }
        }
    }
    env
}

fn references(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once("${") {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        result.push(name.into());
        rest = tail;
    }
    result
}

pub fn registration_references(
    harness: &str,
    env: &BTreeMap<String, String>,
) -> (PathBuf, BTreeSet<String>) {
    let Some(home) = env.get("HOME").map(PathBuf::from) else {
        return (PathBuf::new(), BTreeSet::new());
    };
    let (path, names) = if harness == "claude" {
        let path = env
            .get("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join(".claude.json");
        let doc: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let names = doc["mcpServers"]["violet"]["headers"]
            .as_object()
            .into_iter()
            .flat_map(|h| h.values())
            .filter_map(|v| v.as_str())
            .flat_map(references)
            .collect::<Vec<_>>();
        (path, names)
    } else {
        let path = env
            .get("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".codex"))
            .join("config.toml");
        let doc: toml::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or(toml::Value::Table(Default::default()));
        let entry = doc.get("mcp_servers").and_then(|s| s.get("violet"));
        let mut names = entry
            .and_then(|e| e.get("bearer_token_env_var"))
            .and_then(|v| v.as_str())
            .map(|s| vec![s.to_owned()])
            .unwrap_or_default();
        if let Some(headers) = entry
            .and_then(|e| e.get("env_http_headers"))
            .and_then(|v| v.as_table())
        {
            names.extend(
                headers
                    .values()
                    .filter_map(|v| v.as_str())
                    .map(str::to_owned),
            );
        }
        (path, names)
    };
    (path, names.into_iter().collect())
}

pub fn missing_references(harness: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    let (path, names) = registration_references(harness, env);
    names
        .into_iter()
        .filter(|name| !env.get(name).is_some_and(|v| !v.trim().is_empty()))
        .map(|name| format!("{}: {name} is empty or unset", path.display()))
        .collect()
}

pub fn hydrate(harness: &str, overrides: &mut Vec<(String, String)>, removed: &[String]) {
    hydrate_with_grants(harness, overrides, removed, &BTreeSet::new());
}

pub(crate) fn hydrate_with_grants(
    harness: &str,
    overrides: &mut Vec<(String, String)>,
    removed: &[String],
    grants: &BTreeSet<String>,
) {
    let env = resolved_environment(overrides, removed);
    let (_, mut names) = registration_references(harness, &env);
    names.extend(grants.iter().cloned());
    for warning in missing_references(harness, &env) {
        warn_once(
            warning
                .rsplit_once(": ")
                .map(|(_, name)| name)
                .unwrap_or(&warning),
            || tracing::warn!("Violet launch: {warning}"),
        );
    }
    for (name, value) in env
        .into_iter()
        .filter(|(name, _)| canonical_credential(name) && names.contains(name))
    {
        if let Some((_, current)) = overrides.iter_mut().rev().find(|(key, _)| key == &name) {
            *current = value;
        } else {
            overrides.push((name, value));
        }
    }
}

pub fn configure_command(command: &mut Command, harness: &str) {
    warn_credentials_file_for_cli();
    let mut overrides = Vec::new();
    let mut removed = Vec::new();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            overrides.push((
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            ));
        } else {
            removed.push(key.to_string_lossy().into_owned());
        }
    }
    hydrate(harness, &mut overrides, &removed);
    command.envs(
        overrides
            .into_iter()
            .filter(|(name, _)| canonical_credential(name)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_guard::TestEnvGuard;
    fn private_file(path: &Path, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn registrations(home: &Path, token: &str) {
        std::fs::write(home.join(".claude.json"), format!(r#"{{"mcpServers":{{"violet":{{"headers":{{"Authorization":"Bearer ${{{token}}}"}}}}}}}}"#)).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            format!("[mcp_servers.violet]\nbearer_token_env_var='{token}'\n"),
        )
        .unwrap();
    }

    #[test]
    fn cas_678d_round3_literal_comments_and_private_file_safety() {
        use std::os::unix::fs::symlink;
        let guard = TestEnvGuard::temp_home();
        let path = guard.home().join("credentials.env");
        private_file(
            &path,
            "TOKEN='contains # hash' # comment\nOTHER=literal # comment\n",
        );
        assert_eq!(literal_file_values(&path)["TOKEN"], "contains # hash");
        assert_eq!(literal_file_values(&path)["OTHER"], "literal");
        let link = guard.home().join("link");
        symlink(&path, &link).unwrap();
        assert!(literal_file_values(&link).is_empty());
        private_file(&path, &"x".repeat(1024 * 1024 + 1));
        assert!(literal_file_values(&path).is_empty());
        assert!(literal_file_values(guard.home()).is_empty());
    }

    #[test]
    fn cas_678d_round3_hydration_injects_only_registered_account() {
        use std::os::unix::fs::PermissionsExt;
        let mut guard = TestEnvGuard::temp_home();
        let home = guard.home().to_path_buf();
        guard.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
        guard.set("VIOLET_SLACK_TOKEN_REGISTERED", "");
        guard.set("VIOLET_SLACK_TOKEN_UNRELATED", "");
        std::fs::write(home.join("credentials.env"), "VIOLET_SLACK_TOKEN_REGISTERED='registered'\nVIOLET_SLACK_TOKEN_UNRELATED='unrelated'\n").unwrap();
        std::fs::set_permissions(
            home.join("credentials.env"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_REGISTERED}"}}}}"#).unwrap();
        let mut pane = vec![];
        hydrate("claude", &mut pane, &[]);
        assert!(
            pane.iter()
                .any(|(k, v)| k == "VIOLET_SLACK_TOKEN_REGISTERED" && v == "registered")
        );
        assert!(
            !pane
                .iter()
                .any(|(k, _)| k == "VIOLET_SLACK_TOKEN_UNRELATED")
        );
    }

    #[test]
    fn cas_678d_round3_refuses_public_credentials_file() {
        use std::os::unix::fs::PermissionsExt;
        let guard = TestEnvGuard::temp_home();
        let path = guard.home().join("credentials.env");
        std::fs::write(&path, "VIOLET_SLACK_TOKEN_FIXTURE='private'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(literal_file_values(&path).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn violet_env_actual_pty_spawn_and_respawn_load_fresh_private_values() {
        use std::os::unix::fs::PermissionsExt;
        let mut guard = TestEnvGuard::temp_home();
        let home = guard.home().to_path_buf();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["claude", "codex", "cas"] {
            let path = bin.join(name);
            std::fs::write(
                &path,
                "#!/bin/sh\nprintf '%s' \"$VIOLET_SLACK_TOKEN_FIXTURE\"\n",
            )
            .unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        guard.set("PATH", "/usr/bin:/bin");
        registrations(&home, "VIOLET_SLACK_TOKEN_FIXTURE");
        guard.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
        guard.set("VIOLET_SLACK_TOKEN_FIXTURE", "");
        for harness in ["claude", "codex"] {
            for (value, worker, granted) in [
                ("first-fixture", false, false),
                ("second-fixture", false, false),
                ("allowed-worker", true, true),
                ("denied-worker", true, false),
            ] {
                private_file(
                    &home.join("credentials.env"),
                    &format!("export VIOLET_SLACK_TOKEN_FIXTURE='{value}'\n"),
                );
                let mut config = crate::PtyConfig {
                    command: harness.into(),
                    cwd: Some(home.clone()),
                    env: vec![("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))],
                    ..Default::default()
                };
                if worker {
                    config.env.push(("CAS_AGENT_ROLE".into(), "worker".into()));
                }
                if granted {
                    config.grant_worker_credential("VIOLET_SLACK_TOKEN_FIXTURE");
                }
                let mut pty = crate::Pty::spawn("violet-launch-fixture", config).unwrap();
                let mut output = Vec::new();
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while let Some(event) = pty.recv().await {
                        match event {
                            crate::PtyEvent::Output(bytes) => output.extend(bytes),
                            crate::PtyEvent::Exited(_) => break,
                            crate::PtyEvent::Error(error) => panic!("{error}"),
                        }
                    }
                })
                .await
                .unwrap();
                assert_eq!(
                    String::from_utf8_lossy(&output).contains(value),
                    !worker || granted,
                    "{harness} value={value}"
                );
            }
        }
    }

    #[test]
    fn violet_env_missing_reference_warning_names_the_variable_only() {
        let mut guard = TestEnvGuard::temp_home();
        guard.remove("VIOLET_SLACK_TOKEN_MISSING_FIXTURE");
        let path = guard.home().join(".claude.json");
        std::fs::write(&path, r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_MISSING_FIXTURE}"}}}}"#).unwrap();
        let warnings = missing_references("claude", &resolved_environment(&[], &[]));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("VIOLET_SLACK_TOKEN_MISSING_FIXTURE is empty or unset"));
    }
    #[test]
    fn violet_env_child_resolution_matches_probe_and_respects_denials() {
        let mut guard = TestEnvGuard::temp_home();
        let path = guard.home().join("credentials.env");
        guard.set("CAS_CREDENTIALS_FILE", &path);
        guard.set("VIOLET_SLACK_TOKEN_FIXTURE", "");
        private_file(
            &path,
            "export VIOLET_SLACK_TOKEN_FIXTURE='fixture-value'\nexport VIOLET_SLACK_TOKEN_BAD='$(pass show secret)'\n",
        );
        registrations(guard.home(), "VIOLET_SLACK_TOKEN_FIXTURE");
        let expected = resolved_environment(&[], &[]);
        assert_eq!(
            expected
                .get("VIOLET_SLACK_TOKEN_FIXTURE")
                .map(String::as_str),
            Some("fixture-value")
        );
        assert!(!expected.contains_key("VIOLET_SLACK_TOKEN_BAD"));
        for harness in ["claude", "codex"] {
            let mut command = Command::new(harness);
            configure_command(&mut command, harness);
            assert_eq!(
                command
                    .get_envs()
                    .find(|(k, _)| *k == "VIOLET_SLACK_TOKEN_FIXTURE")
                    .and_then(|(_, v)| v)
                    .and_then(|v| v.to_str()),
                Some("fixture-value")
            );
            let mut pane = vec![];
            hydrate(harness, &mut pane, &[]);
            assert!(
                pane.iter()
                    .any(|(k, v)| k == "VIOLET_SLACK_TOKEN_FIXTURE" && v == "fixture-value")
            );
        }
        assert!(
            !resolved_environment(&[], &["VIOLET_SLACK_TOKEN_FIXTURE".into()])
                .contains_key("VIOLET_SLACK_TOKEN_FIXTURE")
        );
        guard.set("VIOLET_SLACK_TOKEN_FIXTURE", "keychain-fixture");
        assert_eq!(
            resolved_environment(&[], &[])["VIOLET_SLACK_TOKEN_FIXTURE"],
            "keychain-fixture"
        );
    }
}
