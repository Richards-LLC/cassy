//! Read-only launch resolution. Creation belongs exclusively to explicit login.
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::{Account, LoginState, discover_with};
use crate::accounts::Provider;

/// Resolve an existing folder alias first, then a unique login email. An
/// ambiguous email never selects a subscription implicitly. This does no
/// provider probing and never creates, renames or writes a directory.
pub fn resolve_existing(provider: Provider, home: &Path, selector: &str) -> Result<Account> {
    let rows = discover_with(provider, home, false, |_, _| LoginState::Unknown)?;
    if let Some(row) = rows.iter().find(|row| row.name == selector) {
        return Ok(row.clone());
    }
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(selector))
        })
        .collect();
    match matches.as_slice() {
        [row] => return Ok((*row).clone()),
        [] => {}
        _ => bail!(
            "ambiguous {} account {selector:?}; choose a folder alias: {}",
            provider.name(),
            matches
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
    let closest = rows
        .iter()
        .min_by_key(|row| (distance(selector, &row.name), &row.name))
        .filter(|row| distance(selector, &row.name) <= 2)
        .map(|row| format!("\nClosest alias: {}", row.name))
        .unwrap_or_default();
    bail!(
        "unknown {} account {selector:?}{closest}\nLog in a new account: cas {} login {}",
        provider.name(),
        provider.name(),
        shell_word(selector)
    )
}

/// Resolve worker config_dir inputs without creating a profile. Existing
/// explicit paths (including external directories) retain their old meaning;
/// bare aliases and display emails use the same rule as interactive launches.
pub fn resolve_config_dir(provider: Provider, home: &Path, raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("config_dir is empty");
    }
    if raw.contains('/') || raw.starts_with('.') || raw.starts_with('~') || raw.contains('\\') {
        let path = raw
            .strip_prefix("~/")
            .map_or_else(|| PathBuf::from(raw), |rest| home.join(rest));
        if path.is_dir() {
            return Ok(path);
        }
        let alias = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(raw)
            .strip_prefix(provider.prefix())
            .unwrap_or(raw);
        let cause = resolve_existing(provider, home, alias)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| format!("use an existing {} account directory", provider.name()));
        bail!(
            "account directory does not exist: {}\n{cause}",
            path.display()
        );
    }
    Ok(resolve_existing(provider, home, raw)?.folder)
}

pub(crate) fn resolve_worker_accounts(
    specs: &mut [cas_mux::WorkerSpec],
    home: &Path,
) -> Result<()> {
    for spec in specs {
        let provider = match spec.cli {
            cas_mux::SupervisorCli::Claude => Provider::Claude,
            cas_mux::SupervisorCli::Codex => Provider::Codex,
            _ => continue,
        };
        if let Some(raw) = spec.config_dir.as_deref() {
            spec.config_dir = Some(
                resolve_config_dir(provider, home, raw)?
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(())
}

fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-+".contains(c))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<_> = b.chars().collect();
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, a) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &b) in b.iter().enumerate() {
            let previous = row[j + 1];
            row[j + 1] = (diagonal + usize::from(a != b))
                .min(row[j] + 1)
                .min(previous + 1);
            diagonal = previous;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_alias_email_and_worker_directory_resolution_share_one_refusal_policy() {
        use base64::Engine as _;
        let home = tempfile::tempdir().unwrap();
        for (provider, cli) in [
            (Provider::Claude, cas_mux::SupervisorCli::Claude),
            (Provider::Codex, cas_mux::SupervisorCli::Codex),
        ] {
            for (name, email) in [
                ("main", "main@example.com"),
                ("alt", "shared@example.com"),
                ("work", "shared@example.com"),
            ] {
                let dir = home.path().join(if name == "main" {
                    format!(".{}", provider.name())
                } else {
                    format!("{}{name}", provider.prefix())
                });
                std::fs::create_dir(&dir).unwrap();
                match provider {
                    Provider::Claude => std::fs::write(
                        if name == "main" {
                            home.path().join(".claude.json")
                        } else {
                            dir.join(".claude.json")
                        },
                        serde_json::json!({"oauthAccount":{"emailAddress":email}}).to_string(),
                    )
                    .unwrap(),
                    Provider::Codex => {
                        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(serde_json::json!({"email":email}).to_string());
                        std::fs::write(
                            dir.join("auth.json"),
                            serde_json::json!({"tokens":{"id_token":format!("h.{claims}.s")}})
                                .to_string(),
                        )
                        .unwrap();
                    }
                }
            }
            if provider == Provider::Claude {
                assert_eq!(resolve_existing(provider, home.path(), "main@example.com").unwrap().name, "main");
            } else {
                assert!(resolve_existing(provider, home.path(), "main@example.com").is_err(),
                    "Codex token email cannot authorize an alias lookup");
            }
            assert_eq!(
                resolve_existing(provider, home.path(), "alt").unwrap().name,
                "alt"
            );
            let ambiguous = resolve_existing(provider, home.path(), "shared@example.com")
                .unwrap_err()
                .to_string();
            if provider == Provider::Claude {
                assert!(ambiguous.contains("ambiguous") && ambiguous.contains("alt") && ambiguous.contains("work"));
            } else {
                assert!(ambiguous.contains("unknown codex account"), "{ambiguous}");
            }
            let error = resolve_existing(provider, home.path(), "altt")
                .unwrap_err()
                .to_string();
            assert!(error.contains("Closest alias: alt"));
            let unrelated = resolve_existing(provider, home.path(), "nobody@example.net")
                .unwrap_err()
                .to_string();
            assert!(!unrelated.contains("Closest alias:"), "{unrelated}");
            assert!(error.contains(&format!("cas {} login altt", provider.name())));
            let mut specs = vec![cas_mux::WorkerSpec {
                name: None,
                cli,
                model: None,
                effort: None,
                config_dir: Some(if provider == Provider::Claude { "main@example.com" } else { "main" }.into()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            }];
            resolve_worker_accounts(&mut specs, home.path()).unwrap();
            assert_eq!(
                specs[0].config_dir.as_deref(),
                home.path().join(format!(".{}", provider.name())).to_str()
            );
            specs[0].config_dir = Some("altt".into());
            assert!(resolve_worker_accounts(&mut specs, home.path()).is_err());
            assert!(
                !home
                    .path()
                    .join(format!("{}altt", provider.prefix()))
                    .exists()
            );
            assert!(
                resolve_config_dir(
                    provider,
                    home.path(),
                    &format!("~/{}missing", provider.prefix())
                )
                .is_err()
            );
        }
    }
}
