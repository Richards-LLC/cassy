//! Credential selection at the CLI's production, pre-network connect boundary.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cas_pty::violet_env;
use cmcp_core::config::{
    Config, Scope, ServerConfig, VIOLET_DEFAULT_TOKEN_ENV, VIOLET_SERVER,
    canonical_violet_credential_name, is_violet_hub_url, violet_credential_names,
    violet_credential_value,
};

use super::hub::VioletError;

// Only the user-owned managed default may select a different account. Re-read
// provenance conservatively: a changed registration loses this exception.
fn managed_default(server: &ServerConfig, root: Option<&Path>) -> bool {
    let Ok(user) = Scope::User.config_path() else {
        return false;
    };
    let project = root.map(|root| root.join("proxy.toml"));
    let Ok((config, sources)) = Config::load_merged_with_sources(project.as_deref()) else {
        return false;
    };
    sources.get(VIOLET_SERVER) == Some(&user)
        && config.servers.get(VIOLET_SERVER) == Some(server)
        && std::fs::read_to_string(user)
            .is_ok_and(|raw| raw.lines().any(|line| line.trim() == "# managed"))
        && matches!(server, ServerConfig::Http { url, auth: Some(auth), .. }
            | ServerConfig::Sse { url, auth: Some(auth), .. }
            if is_violet_hub_url(url) && auth == &format!("env:{VIOLET_DEFAULT_TOKEN_ENV}"))
}

fn missing(detail: impl AsRef<str>) -> VioletError {
    let path = violet_env::credential_file()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "credentials.env (no configured path)".into());
    VioletError::local(
        "missing_credential",
        format!(
            "{}; tried the process environment and {path}; check the named variable and registration with `cas doctor`",
            detail.as_ref()
        ),
    )
}

fn value(
    name: &str,
    env: &BTreeMap<String, String>,
    removed: &[String],
) -> Result<String, VioletError> {
    if removed.contains(&name.to_owned()) {
        return Err(missing(format!(
            "credential variable {name} is withheld by worker policy"
        )));
    }
    violet_credential_value(name, |candidate| env.get(candidate).cloned())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| missing(format!("credential variable {name} is empty or unset")))
}

fn selected_token(
    env: &BTreeMap<String, String>,
    native: &[(std::path::PathBuf, BTreeSet<String>)],
    removed: &[String],
) -> Result<String, VioletError> {
    // A denied default cannot be replaced with another account.
    if removed.iter().any(|name| name == VIOLET_DEFAULT_TOKEN_ENV) {
        return value(VIOLET_DEFAULT_TOKEN_ENV, env, removed);
    }
    if let Some(selector) = env
        .get("VIOLET_SLACK_TOKEN_ENV")
        .filter(|v| !v.trim().is_empty())
    {
        let selector = selector.trim();
        if !selector
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        {
            return Err(missing(
                "VIOLET_SLACK_TOKEN_ENV must name an environment variable",
            ));
        }
        return value(selector, env, removed);
    }
    for name in [VIOLET_DEFAULT_TOKEN_ENV, "VIOLET_SLACK_TOKEN"] {
        if let Ok(token) = value(name, env, removed) {
            return Ok(token);
        }
    }
    let registered: BTreeSet<_> = native
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|name| canonical_violet_credential_name(name))
        .collect();
    if registered.len() == 1 {
        let name = registered.first().unwrap();
        return value(name, env, removed).map_err(|_| {
            let paths = native
                .iter()
                .filter(|(_, names)| !names.is_empty())
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            missing(format!(
                "registered credential variable {name} from {paths} is empty, unset or withheld"
            ))
        });
    }
    let candidates = if registered.is_empty() {
        env.iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(name, _)| canonical_violet_credential_name(name))
            .filter(|name| {
                name.starts_with("VIOLET_SLACK_TOKEN_") && name != "VIOLET_SLACK_TOKEN_ENV"
            })
            .collect::<BTreeSet<_>>()
    } else {
        registered
    };
    if candidates.len() == 1 {
        return value(candidates.first().unwrap(), env, removed);
    }
    if candidates.is_empty() {
        return value(VIOLET_DEFAULT_TOKEN_ENV, env, removed);
    }
    Err(missing(format!(
        "multiple Violet token variables found ({}); selection is ambiguous; set VIOLET_SLACK_TOKEN_ENV",
        candidates.into_iter().collect::<Vec<_>>().join(", ")
    )))
}

pub(super) fn resolve(
    server: &mut ServerConfig,
    root: Option<&Path>,
    removed: &[String],
) -> Result<(), VioletError> {
    let managed = managed_default(server, root);
    let base = violet_env::resolved_environment(&[], removed);
    let native: Vec<_> = if managed {
        ["claude", "codex"]
            .iter()
            .map(|harness| violet_env::registration_token_references(harness, &base))
            .collect()
    } else {
        vec![]
    };
    let mut refs: Vec<_> = native
        .iter()
        .flat_map(|(_, names)| names.iter().cloned())
        .collect();
    if let Some(selector) = base.get("VIOLET_SLACK_TOKEN_ENV") {
        refs.push(selector.trim().into());
    }
    match server {
        ServerConfig::Http { auth, headers, .. } | ServerConfig::Sse { auth, headers, .. } => {
            refs.extend(
                auth.iter()
                    .chain(headers.values())
                    .filter_map(|v| v.strip_prefix("env:").map(str::to_owned)),
            );
        }
        ServerConfig::Stdio { .. } => return Ok(()),
    }
    refs.push("VIOLET_SLACK_TOKEN".into());
    let refs: Vec<_> = refs
        .into_iter()
        .flat_map(|name| violet_credential_names(&name))
        .collect();
    let env = violet_env::resolved_environment_for_references(&[], removed, &refs);
    let strict = |reference: &mut String| -> Result<(), VioletError> {
        if let Some(name) = reference
            .strip_prefix("env:")
            .filter(|name| !name.is_empty())
        {
            *reference = value(name, &env, removed)?;
        }
        Ok(())
    };
    match server {
        ServerConfig::Http { auth, headers, .. } | ServerConfig::Sse { auth, headers, .. } => {
            if let Some(auth) = auth {
                if managed {
                    *auth = selected_token(&env, &native, removed)?;
                } else {
                    strict(auth)?;
                }
            }
            for header in headers.values_mut() {
                strict(header)?;
            }
        }
        ServerConfig::Stdio { .. } => {}
    }
    Ok(())
}
