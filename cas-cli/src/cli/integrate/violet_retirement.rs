//! Remove only registrations for the retired production hub and move the
//! production registration to the canonical hostname. Custom upstreams,
//! restrictive policy, comments and unrelated settings survive.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cas_types::violet_compatibility::{
    canonical_violet_credential_name, is_violet_hub_url, violet_compatibility, violet_hub_url,
};
use cmcp_core::config::{
    ExternalToolConfig, Scope, VIOLET_SERVER, VIOLET_TOOLS, violet_credential_names,
    violet_credential_value,
};

use super::fs as ifs;
use super::violet::{HubProbe, ProbeOutcome, ProxyHubProbe};

fn retire_toml_entry(document: &mut toml_edit::DocumentMut, key: &str) -> bool {
    let contract = violet_compatibility();
    let Some(servers) = document
        .get_mut(key)
        .and_then(|item| item.as_table_like_mut())
    else {
        return false;
    };
    let Some(legacy) = servers.get(&contract.retired_server) else {
        return false;
    };
    if !legacy
        .get("url")
        .and_then(|item| item.as_str())
        .is_some_and(is_violet_hub_url)
    {
        return false;
    }
    let legacy = legacy.clone();
    if !servers.contains_key(VIOLET_SERVER) {
        servers.insert(VIOLET_SERVER, legacy);
    }
    servers.remove(&contract.retired_server);
    true
}

/// Replace a string value in place, keeping its surrounding formatting.
fn set_string(item: &mut toml_edit::Item, text: &str) {
    if let Some(value) = item.as_value_mut() {
        let decor = value.decor().clone();
        *value = toml_edit::Value::from(text);
        *value.decor_mut() = decor;
    }
}

/// Point a production entry at the canonical URL; optionally rename proxy refs.
fn modernize_toml_entry(
    document: &mut toml_edit::DocumentMut,
    key: &str,
    rename_env: bool,
) -> bool {
    let Some(entry) = document
        .get_mut(key)
        .and_then(|servers| servers.as_table_like_mut())
        .and_then(|servers| servers.get_mut(VIOLET_SERVER))
        .and_then(|entry| entry.as_table_like_mut())
    else {
        return false;
    };
    let Some(url) = entry.get_mut("url") else {
        return false;
    };
    if !url.as_str().is_some_and(is_violet_hub_url) {
        return false;
    }
    let mut changed = false;
    if url.as_str() != Some(violet_hub_url()) {
        set_string(url, violet_hub_url());
        changed = true;
    }
    if !rename_env {
        return changed;
    }
    let mut rename = |item: &mut toml_edit::Item| {
        let Some(name) = item.as_str().and_then(|text| text.strip_prefix("env:")) else {
            return;
        };
        let canonical = canonical_violet_credential_name(name);
        if canonical != name {
            set_string(item, &format!("env:{canonical}"));
            changed = true;
        }
    };
    if let Some(auth) = entry.get_mut("auth") {
        rename(auth);
    }
    if let Some(headers) = entry
        .get_mut("headers")
        .and_then(|headers| headers.as_table_like_mut())
    {
        for (_, value) in headers.iter_mut() {
            rename(value);
        }
    }
    changed
}

fn route(value: &toml_edit::Value) -> Option<ExternalToolConfig> {
    if let Some(text) = value.as_str() {
        return ExternalToolConfig::parse_allowlist_entry(text).ok();
    }
    let table = value.as_inline_table()?;
    ExternalToolConfig::parse_allowlist_entry(&format!(
        "{}.{}",
        table.get("server")?.as_str()?,
        table.get("tool")?.as_str()?
    ))
    .ok()
}

/// Called by both the integration planner and update/sync. An unrelated file
/// or explicit project opt-out never gains routes.
pub(super) fn retire_proxy_document(document: &mut toml_edit::DocumentMut) -> bool {
    let retired = retire_proxy_server(document);
    modernize_toml_entry(document, "servers", true) | retired
}

fn retire_proxy_server(document: &mut toml_edit::DocumentMut) -> bool {
    let contract = violet_compatibility();
    let server_access = document
        .get("servers")
        .and_then(|servers| servers.get(&contract.retired_server))
        .and_then(|server| server.get("worker_access"))
        .cloned();
    if !retire_toml_entry(document, "servers") {
        return false;
    }
    if let Some(access) = server_access {
        document["worker_access"].or_insert(toml_edit::table());
        document["worker_access"][VIOLET_SERVER] = access;
    }
    if let Some(access) = document
        .get_mut("worker_access")
        .and_then(|item| item.as_table_like_mut())
        && let Some(legacy) = access.remove(&contract.retired_server)
    {
        // Read-only is the sole supported restriction. Keep the stricter policy.
        access.insert(VIOLET_SERVER, legacy);
    }
    for key in ["allowlist", "worker_read_routes"] {
        if let Some(array) = document.get_mut(key).and_then(|item| item.as_array_mut()) {
            for value in array.iter_mut() {
                if let Some(mut parsed) = route(value)
                    && parsed.server == contract.retired_server
                {
                    parsed.server = VIOLET_SERVER.into();
                    if let Some(index) = contract
                        .retired_tools
                        .iter()
                        .position(|tool| tool == &parsed.tool)
                    {
                        parsed.tool = VIOLET_TOOLS[index].into();
                    }
                    let decor = value.decor().clone();
                    *value = toml_edit::Value::from(parsed.canonical_entry());
                    *value.decor_mut() = decor;
                }
            }
        }
    }
    true
}

fn retire_claude_document(document: &mut serde_json::Value) -> bool {
    retire_claude_document_with(document, &mut str::to_owned)
}

/// Rewrite Claude's interpolation syntax without altering literal header text.
fn substitute_references(text: &str, resolve: &mut impl FnMut(&str) -> String) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("${") {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        result.push_str(before);
        result.push_str(&resolve(name));
        rest = tail;
    }
    result.push_str(rest);
    result
}

fn retire_claude_document_with(
    document: &mut serde_json::Value,
    rename: &mut impl FnMut(&str) -> String,
) -> bool {
    let contract = violet_compatibility();
    let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(|item| item.as_object_mut())
    else {
        return false;
    };
    let mut changed = false;
    if servers
        .get(&contract.retired_server)
        .and_then(|server| server.get("url"))
        .and_then(|url| url.as_str())
        .is_some_and(is_violet_hub_url)
    {
        let legacy = servers
            .remove(&contract.retired_server)
            .expect("checked above");
        servers.entry(VIOLET_SERVER.to_owned()).or_insert(legacy);
        changed = true;
    }
    if let Some(entry) = servers.get_mut(VIOLET_SERVER)
        && entry["url"].as_str().is_some_and(is_violet_hub_url)
    {
        if entry["url"].as_str() != Some(violet_hub_url()) {
            entry["url"] = serde_json::Value::from(violet_hub_url());
            changed = true;
        }
        if let Some(headers) = entry.get_mut("headers").and_then(|h| h.as_object_mut()) {
            for value in headers.values_mut() {
                if let Some(text) = value.as_str() {
                    let rewritten =
                        substitute_references(text, &mut |name| format!("${{{}}}", rename(name)));
                    if rewritten != text {
                        *value = rewritten.into();
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

pub(super) fn retire_claude_entry(document: &mut serde_json::Value) -> bool {
    retire_claude_document(document)
}
pub(super) fn retire_codex_entry(document: &mut toml_edit::DocumentMut) -> bool {
    retire_codex_entry_with(document, &mut str::to_owned)
}

fn retire_codex_entry_with(
    document: &mut toml_edit::DocumentMut,
    rename: &mut impl FnMut(&str) -> String,
) -> bool {
    let retired = retire_toml_entry(document, "mcp_servers");
    let mut changed = modernize_toml_entry(document, "mcp_servers", false) | retired;
    if let Some(entry) = document
        .get_mut("mcp_servers")
        .and_then(|servers| servers.get_mut(VIOLET_SERVER))
        .and_then(|entry| entry.as_table_like_mut())
        && entry
            .get("url")
            .and_then(|v| v.as_str())
            .is_some_and(is_violet_hub_url)
    {
        let mut rewrite = |value: &mut toml_edit::Item| {
            if let Some(name) = value.as_str() {
                let canonical = rename(name);
                if canonical != name {
                    set_string(value, &canonical);
                    changed = true;
                }
            }
        };
        if let Some(value) = entry.get_mut("bearer_token_env_var") {
            rewrite(value);
        }
        if let Some(headers) = entry
            .get_mut("env_http_headers")
            .and_then(|h| h.as_table_like_mut())
        {
            for (_, value) in headers.iter_mut() {
                rewrite(value);
            }
        }
    }
    changed
}

fn retire_file(path: &Path, format: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if format == "proxy" {
        validate_proxy_owner(path)?;
    }
    let raw = ifs::read_capped(path)?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let rewritten = if format == "claude" {
        let mut document: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        retire_claude_document(&mut document)
            .then(|| serde_json::to_string_pretty(&document))
            .transpose()?
            .map(|text| format!("{text}\n"))
    } else {
        let mut document: toml_edit::DocumentMut = raw
            .parse()
            .with_context(|| format!("parsing {}", path.display()))?;
        let changed = if format == "proxy" {
            retire_proxy_document(&mut document)
        } else {
            retire_codex_entry(&mut document)
        };
        changed.then(|| document.to_string())
    };
    if let Some(rewritten) = rewritten {
        ifs::atomic_write_create_dirs(path, &rewritten)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn retire_project_proxy(path: &Path) -> Result<bool> {
    retire_file(path, "proxy")
}

/// Repair persisted proxy references during update, before either the CLI or
/// `cas serve` loads them. This is deliberately separate from runtime fallback.
/// Host-wide update calls it for each discovered project's native sync phase.
pub fn repair_proxy_credentials_on_update(project_proxy: Option<&Path>) {
    for (path, reason) in proxy_update_advisories(project_proxy, true) {
        crate::cli::update::report_update_warning(
            &format!("Violet update advisory: {}: {reason}", path.display()),
            "Retry: cas update --user",
        );
    }
}

/// Recompute refusals for doctor; never persist state or a credential value.
pub(super) fn proxy_credential_advisories(project_proxy: Option<&Path>) -> Vec<(PathBuf, String)> {
    proxy_update_advisories(project_proxy, false)
}

fn proxy_update_advisories(project_proxy: Option<&Path>, apply: bool) -> Vec<(PathBuf, String)> {
    let mut update = None;
    let mut report = ProfileRetirement::default();
    let mut paths = Vec::new();
    if let Ok(user) = Scope::User.config_path() {
        paths.push(user);
    }
    paths.extend(project_proxy.map(Path::to_path_buf));
    paths.sort();
    paths.dedup();
    let mut warnings = Vec::new();
    for path in paths {
        // Each file is independent: a bad user proxy cannot strand a healthy
        // project's repair or prevent its rules and skills from syncing.
        let result = (|| -> Result<()> {
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) if !apply => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if apply {
                validate_proxy_owner(&path)?;
            }
            let modified = if apply {
                std::fs::metadata(&path)?.modified().ok()
            } else {
                None
            };
            // Runtime accepts dotfile symlinks. Doctor may inspect their target,
            // but only an unusable default warrants a repair refusal advisory.
            // Read/parse failures belong to the normal doctor configuration row.
            let read_path = if apply {
                path.clone()
            } else {
                match path.canonicalize() {
                    Ok(path) => path,
                    Err(_) => return Ok(()),
                }
            };
            let raw = match ifs::read_capped(&read_path) {
                Ok(raw) => raw,
                Err(_) if !apply => return Ok(()),
                Err(error) => return Err(error),
            };
            let mut doc: toml_edit::DocumentMut = match raw.parse() {
                Ok(doc) => doc,
                Err(_) if !apply => return Ok(()),
                Err(_) => anyhow::bail!("parsing Violet proxy config {}", path.display()),
            };
            if !default_proxy_auth(&doc) {
                return Ok(());
            }
            if update.is_none() {
                if let Some((path, reason)) = cas_pty::violet_env::warn_credentials_file_for_cli() {
                    anyhow::bail!(
                        "Violet credential repair refused {}: {reason}",
                        path.display()
                    );
                }
                let home =
                    dirs::home_dir().context("could not determine HOME for Violet migration")?;
                let refs: Vec<_> = registered_update_accounts(&home)
                    .into_iter()
                    .chain(std::iter::once("VIOLET_SLACK_TOKEN_CASSY_PROXY".to_owned()))
                    .flat_map(|name| violet_credential_names(&name))
                    .collect();
                update = Some(UpdateMigration::new(
                    &home,
                    cas_pty::violet_env::resolved_environment_for_references(&[], &[], &refs),
                    &ProxyHubProbe,
                ));
            }
            let update = update.as_ref().expect("initialized above");
            let Some(replacement) = update.proxy_auth_replacement(&doc, &path)? else {
                return Ok(());
            };
            if !apply {
                validate_proxy_owner(&path)?;
                anyhow::ensure!(
                    !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
                    "{} is a symlink; refusing to write through it",
                    path.display()
                );
            }
            if apply {
                set_string(
                    &mut doc["servers"][VIOLET_SERVER]["auth"],
                    &format!("env:{replacement}"),
                );
                anyhow::ensure!(
                    ifs::atomic_write_if_unchanged(&path, &raw, modified, &doc.to_string())?,
                    "Violet proxy config changed during credential repair at {}; retry cas update",
                    path.display()
                );
                report.credential_repair(proxy_repair_message(&path, &replacement));
            }
            Ok(())
        })();
        if let Err(error) = result {
            warnings.push((path, error.to_string()));
        }
    }
    warnings
}

fn proxy_repair_message(path: &Path, replacement: &str) -> String {
    let scope = if Scope::User.config_path().is_ok_and(|user| user == path) {
        "; shared user proxy: this account applies to every inheriting profile"
    } else {
        ""
    };
    format!(
        "{}: VIOLET_SLACK_TOKEN_CASSY_PROXY → {replacement} (registered account){scope}",
        path.display()
    )
}

fn validate_proxy_owner(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        validate_proxy_owner_for_uid(path, unsafe { libc::geteuid() })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::symlink_metadata(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_proxy_owner_for_uid(path: &Path, current_uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.uid() == current_uid,
        "foreign_owned_violet_proxy: {} is owned by uid {}, expected uid {}; refusing write",
        path.display(),
        metadata.uid(),
        current_uid
    );
    Ok(())
}

fn managed_user_default(doc: &toml_edit::DocumentMut, path: &Path) -> bool {
    Scope::User.config_path().is_ok_and(|user| user == path)
        && doc
            .to_string()
            .lines()
            .any(|line| line.trim() == "# managed")
        && default_proxy_auth(doc)
}

fn default_proxy_auth(doc: &toml_edit::DocumentMut) -> bool {
    doc.get("servers")
        .and_then(|servers| servers.get(VIOLET_SERVER))
        .is_some_and(|entry| {
            entry
                .get("url")
                .and_then(|v| v.as_str())
                .is_some_and(is_violet_hub_url)
                && entry.get("auth").and_then(|v| v.as_str())
                    == Some("env:VIOLET_SLACK_TOKEN_CASSY_PROXY")
        })
}

/// Only bearer references at the production hub are account registrations.
/// Inspect the active/default and explicitly configured runtime profiles;
/// backup directories, unrelated exports and custom endpoints cannot select an account.
fn registered_update_accounts(home: &Path) -> std::collections::BTreeSet<String> {
    let mut environment =
        BTreeMap::from([("HOME".to_owned(), home.to_string_lossy().into_owned())]);
    let mut roots: Vec<_> = home.canonicalize().ok().into_iter().collect();
    for key in ["CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
        if let Some(path) = std::env::var_os(key).map(PathBuf::from) {
            roots.extend(path.canonicalize().ok());
            environment.insert(key.into(), path.to_string_lossy().into_owned());
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for harness in ["claude", "codex"] {
        let (path, references) =
            cas_pty::violet_env::registration_token_references(harness, &environment);
        let Ok(Some(path)) = profile_target(&roots, &path) else {
            continue;
        };
        let Ok(raw) = ifs::read_capped(&path) else {
            continue;
        };
        let production = if harness == "claude" {
            serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .is_some_and(|doc| {
                    doc["mcpServers"][VIOLET_SERVER]["url"]
                        .as_str()
                        .is_some_and(is_violet_hub_url)
                })
        } else {
            raw.parse::<toml_edit::DocumentMut>()
                .ok()
                .is_some_and(|doc| {
                    doc.get("mcp_servers")
                        .and_then(|s| s.get(VIOLET_SERVER))
                        .and_then(|entry| entry.get("url"))
                        .and_then(|v| v.as_str())
                        .is_some_and(is_violet_hub_url)
                })
        };
        if !production {
            continue;
        }
        for token in references
            .into_iter()
            .map(|name| canonical_violet_credential_name(&name))
        {
            if UpdateMigration::managed_token(&token)
                && token != "VIOLET_SLACK_TOKEN_CASSY_PROXY"
                && token
                    .bytes()
                    .all(|b| b == b'_' || b.is_ascii_alphanumeric())
            {
                names.insert(token);
            }
        }
    }
    names
}

#[derive(Default)]
struct ProfileRetirement {
    inspected: usize,
    migrated: usize,
    warnings: Vec<(PathBuf, String)>,
    authenticated: Vec<PathBuf>,
    credential_warnings: Vec<String>,
    unexported: std::collections::BTreeSet<String>,
}

impl ProfileRetirement {
    fn warn(&mut self, path: &Path, error: impl std::fmt::Display) {
        // Display only the outer error context: parser chains may contain
        // credential values from the file being parsed.
        let reason = error.to_string();
        if self
            .warnings
            .contains(&(path.to_path_buf(), reason.clone()))
        {
            return;
        }
        crate::cli::update::report_update_warning(
            &format!("Violet migration skipped {}: {reason}", path.display()),
            "Review the named profile/link target, then retry: cas update --user",
        );
        self.warnings.push((path.to_path_buf(), reason));
    }
    fn credential_repair(&mut self, warning: String) {
        if !self.credential_warnings.contains(&warning) {
            crate::cli::update::report_update_warning(
                &format!("Violet credential reference repaired: {warning}"),
                "Verify: cas doctor",
            );
            self.credential_warnings.push(warning);
        }
    }
}

fn warn_skipped(path: &Path, error: impl std::fmt::Display) {
    ProfileRetirement::default().warn(path, error);
}

/// Resolve profile links before using the generic symlink-refusing readers
/// and writers. Both files and linked parent directories must stay inside
/// HOME or an explicit profile directory (CLAUDE_CONFIG_DIR, CODEX_HOME).
fn profile_target(roots: &[PathBuf], path: &Path) -> Result<Option<PathBuf>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("statting {}", path.display())),
        Ok(_) => {}
    }
    let target = path
        .canonicalize()
        .with_context(|| format!("dangling or inaccessible profile path {}", path.display()))?;
    anyhow::ensure!(
        roots.iter().any(|root| target.starts_with(root)),
        "target {} is outside HOME and explicit profile directories",
        target.display()
    );
    Ok(Some(target))
}

/// Enumerate installed machine profiles, including alternate Claude/Codex
/// accounts. Explicit profile directories are included even outside HOME.
/// One inaccessible profile never interrupts the remaining sync.
fn retire_machine_profiles(home: &Path, extra: &[(&str, PathBuf)]) -> ProfileRetirement {
    retire_machine_profiles_with(home, extra, None)
}

fn retire_machine_profiles_with(
    home: &Path,
    extra: &[(&str, PathBuf)],
    update: Option<&UpdateMigration<'_>>,
) -> ProfileRetirement {
    let mut report = ProfileRetirement::default();
    let mut roots = match home.canonicalize() {
        Ok(home) => vec![home],
        Err(error) => {
            report.warn(home, error);
            return report;
        }
    };
    // Explicit profile directories are trusted even outside HOME; a missing
    // one has nothing to migrate.
    roots.extend(
        extra
            .iter()
            .filter_map(|(_, directory)| directory.canonicalize().ok()),
    );
    let mut profiles = vec![("claude", home.to_path_buf())];
    match std::fs::read_dir(home) {
        Ok(entries) => {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        report.warn(home, error);
                        continue;
                    }
                };
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".claude") {
                    profiles.push(("claude", entry.path()));
                }
                if name.starts_with(".codex") {
                    profiles.push(("codex", entry.path()));
                }
            }
        }
        Err(error) => report.warn(home, error),
    }
    profiles.push(("skills", home.join(".agents")));
    profiles.push(("skills", home.join(".grok")));
    profiles.extend(extra.iter().cloned());
    profiles.sort();
    profiles.dedup();
    let mut visited_files = HashSet::new();
    let mut visited_skills = HashSet::new();
    for (format, directory) in profiles {
        if format != "skills" {
            let path = directory.join(if format == "claude" {
                ".claude.json"
            } else {
                "config.toml"
            });
            match profile_target(&roots, &path) {
                Ok(Some(target)) if visited_files.insert(target.clone()) => {
                    report.inspected += 1;
                    let result = if let Some(update) = update {
                        update.migrate_profile(&target, format, &mut report)
                    } else {
                        retire_file(&target, format)
                    };
                    match result {
                        Ok(true) => report.migrated += 1,
                        Ok(false) => {}
                        Err(error) => report.warn(&path, error),
                    }
                }
                Ok(_) => {}
                Err(error) => report.warn(&path, error),
            }
        }
        let skills_dir = if directory == home {
            home.join(".claude/skills")
        } else {
            directory.join("skills")
        };
        match profile_target(&roots, &skills_dir) {
            Ok(Some(target)) if visited_skills.insert(target.clone()) => {
                if let Err(error) = crate::builtins::prune_retired_hub_skills(&target) {
                    report.warn(&skills_dir, error);
                }
            }
            Ok(_) => {}
            Err(error) => report.warn(&skills_dir, error),
        }
    }
    tracing::debug!(
        inspected = report.inspected,
        migrated = report.migrated,
        "Violet profile migration"
    );
    report
}

pub fn retire_installed_hub(project_proxy: Option<&Path>) -> Result<()> {
    if let Some(home) = dirs::home_dir() {
        let extra: Vec<_> = [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")]
            .into_iter()
            .filter_map(|(format, name)| {
                std::env::var_os(name).map(|path| (format, PathBuf::from(path)))
            })
            .collect();
        retire_machine_profiles(&home, &extra);
    }
    if let Ok(user) = Scope::User.config_path() {
        if let Err(error) = retire_project_proxy(&user) {
            warn_skipped(&user, error);
        }
    }
    if let Some(project) = project_proxy {
        if let Err(error) = retire_project_proxy(project) {
            warn_skipped(project, error);
        }
    }
    Ok(())
}

/// `cas update` owns both the rewrite and its authenticated receipt. Run once
/// in the user-level phase, rather than once for every registered project.
pub fn migrate_installed_hub_on_update() -> Result<()> {
    let probe = UpdateBudgetProbe {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
    };
    migrate_installed_hub_on_update_with_probe(&probe)
}

pub(crate) fn migrate_installed_hub_on_update_with_probe(probe: &dyn HubProbe) -> Result<()> {
    let report = migrate_installed_hub_with_probe(probe)?;
    anyhow::ensure!(
        report.warnings.is_empty(),
        "Violet update incomplete for {} profile(s); see the named warnings above",
        report.warnings.len()
    );
    Ok(())
}

struct UpdateBudgetProbe {
    deadline: std::time::Instant,
}

impl HubProbe for UpdateBudgetProbe {
    fn list_tools(&self, server: &cmcp_core::config::ServerConfig) -> ProbeOutcome {
        let remaining = self
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return ProbeOutcome::Skipped {
                reason: "Violet update probe budget exhausted".into(),
            };
        }
        ProxyHubProbe
            .list_tools_with_timeout(server, remaining.min(std::time::Duration::from_secs(10)))
    }
}

fn migrate_installed_hub_with_probe(probe: &dyn HubProbe) -> Result<ProfileRetirement> {
    if let Some(refusal) = cas_pty::violet_env::warn_credentials_file_for_cli() {
        // The CLI helper printed this path once; retain it in the receipt and
        // refuse unsafe input before a migration can read or rewrite it.
        let mut report = ProfileRetirement::default();
        report.warnings.push(refusal);
        return Ok(report);
    }
    let private_before = cas_pty::violet_env::credential_file()
        .map(|path| cas_pty::violet_env::literal_file_values(&path))
        .unwrap_or_default();
    let values = super::violet::update_machine_credentials()?;
    let home = dirs::home_dir().context("could not determine HOME for Violet migration")?;
    let extra: Vec<_> = [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")]
        .into_iter()
        .filter_map(|(format, name)| std::env::var_os(name).map(|p| (format, PathBuf::from(p))))
        .collect();
    let mut update = UpdateMigration::new(&home, values, probe);
    update.private_before = private_before;
    let mut report = retire_machine_profiles_with(&home, &extra, Some(&update));
    if let Ok(user) = Scope::User.config_path()
        && user.is_file()
    {
        match update.migrate_profile(&user, "proxy", &mut report) {
            Ok(true) => report.migrated += 1,
            Ok(false) => {}
            Err(error) => report.warn(&user, error),
        }
    }
    Ok(report)
}

struct UpdateMigration<'a> {
    home: PathBuf,
    values: BTreeMap<String, String>,
    candidates: Vec<String>,
    private_before: BTreeMap<String, String>,
    probe: &'a dyn HubProbe,
}

impl<'a> UpdateMigration<'a> {
    fn new(home: &Path, values: BTreeMap<String, String>, probe: &'a dyn HubProbe) -> Self {
        let candidates = values
            .keys()
            .filter(|name| {
                Self::managed_token(name) && values.get(*name).is_some_and(|v| !v.trim().is_empty())
            })
            .cloned()
            .collect();
        Self {
            home: home.to_owned(),
            values,
            candidates,
            private_before: Default::default(),
            probe,
        }
    }

    fn proxy_auth_replacement(
        &self,
        doc: &toml_edit::DocumentMut,
        path: &Path,
    ) -> Result<Option<String>> {
        if !default_proxy_auth(doc) || self.usable("VIOLET_SLACK_TOKEN_CASSY_PROXY") {
            return Ok(None);
        }
        // Rewriting a managed default disables its explicit selector/generic
        // fallback. Preserve it whenever the real runtime can already resolve it.
        if managed_user_default(doc, path)
            && crate::cli::violet_cmd::credentials::managed_token_value().is_ok()
        {
            return Ok(None);
        }
        let accounts = registered_update_accounts(&self.home);
        anyhow::ensure!(
            accounts.len() <= 1,
            "ambiguous_violet_credential: {}: multiple registered account credentials ({}); no reference changed",
            path.display(),
            accounts.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        let replacement = accounts.first().with_context(|| format!(
            "missing_registered_violet_credential: {}: VIOLET_SLACK_TOKEN_CASSY_PROXY is empty or unset and no account bearer reference is registered; no reference changed", path.display()))?;
        self.value(replacement)?;
        Ok(Some(replacement.clone()))
    }

    fn repair_proxy_auth(
        &self,
        doc: &mut toml_edit::DocumentMut,
        path: &Path,
        report: &mut ProfileRetirement,
    ) -> Result<bool> {
        let Some(replacement) = self.proxy_auth_replacement(doc, path)? else {
            return Ok(false);
        };
        set_string(
            &mut doc["servers"][VIOLET_SERVER]["auth"],
            &format!("env:{replacement}"),
        );
        report.credential_repair(proxy_repair_message(path, &replacement));
        Ok(true)
    }

    fn usable(&self, name: &str) -> bool {
        violet_credential_value(name, |name| self.values.get(name).cloned())
            .is_some_and(|value| !value.trim().is_empty())
    }

    fn managed_token(name: &str) -> bool {
        name.starts_with("VIOLET_SLACK_TOKEN_") && name != "VIOLET_SLACK_TOKEN_ENV"
    }

    fn rename(&self, name: &str, path: &Path, report: &mut ProfileRetirement) -> String {
        let canonical = canonical_violet_credential_name(name);
        let legacy = name
            .strip_prefix(violet_compatibility().legacy_token_prefix.as_str())
            .is_some_and(|suffix| suffix.starts_with('_'));
        let replacement = if self.usable(&canonical) {
            canonical
        } else if legacy
            && !self.usable(name)
            && !self
                .private_before
                .get(name)
                .is_some_and(|v| !v.trim().is_empty())
            && self.candidates.len() == 1
        {
            self.candidates[0].clone()
        } else {
            name.to_owned()
        };
        // This receipt describes the literal name being rewritten. Runtime
        // alias fallback must not hide an absent original binding from it.
        if replacement != name
            && !self
                .values
                .get(name)
                .is_some_and(|value| !value.trim().is_empty())
        {
            let reason = if self
                .private_before
                .get(name)
                .is_some_and(|v| !v.trim().is_empty())
            {
                "private credential renamed"
            } else {
                "legacy name had no value"
            };
            let warning = format!("{}: {name} → {replacement} ({reason})", path.display());
            report.credential_repair(warning);
        }
        replacement
    }

    fn value(&self, name: &str) -> Result<String> {
        violet_credential_value(name, |name| self.values.get(name).cloned())
            .filter(|v| !v.trim().is_empty())
            .with_context(|| {
                let candidates = if self.candidates.len() > 1 {
                    format!(
                        "; ambiguous canonical candidates: {}",
                        self.candidates.join(", ")
                    )
                } else {
                    String::new()
                };
                format!("credential {name} is empty or unset{candidates}; no request sent")
            })
    }

    fn expand(&self, text: &str) -> Result<String> {
        let mut missing = None;
        let expanded = substitute_references(text, &mut |name| match self.value(name) {
            Ok(value) => value,
            Err(error) => {
                missing = Some(error);
                String::new()
            }
        });
        if let Some(error) = missing {
            return Err(error);
        }
        Ok(expanded)
    }

    fn migrate_profile(
        &self,
        path: &Path,
        format: &str,
        report: &mut ProfileRetirement,
    ) -> Result<bool> {
        self.migrate_profile_transaction(path, format, report, false, &|_| {})
    }

    fn migrate_profile_transaction(
        &self,
        path: &Path,
        format: &str,
        report: &mut ProfileRetirement,
        retried: bool,
        before_write: &dyn Fn(&Path),
    ) -> Result<bool> {
        if format == "proxy" {
            validate_proxy_owner(path)?;
        }
        let modified = std::fs::metadata(path)?.modified().ok();
        let raw = ifs::read_capped(path)?;
        if raw.trim().is_empty() {
            return Ok(false);
        }
        let mut referenced = std::collections::BTreeSet::new();
        let mut headers = std::collections::HashMap::new();
        let mut auth = None;
        let (changed, rewritten, url, sse) = if format == "claude" {
            let mut doc: serde_json::Value = serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?;
            let changed =
                retire_claude_document_with(&mut doc, &mut |name| self.rename(name, path, report));
            let entry = &doc["mcpServers"][VIOLET_SERVER];
            if !entry["url"].as_str().is_some_and(is_violet_hub_url) {
                return Ok(false);
            }
            if let Some(entries) = entry["headers"].as_object() {
                for (name, value) in entries {
                    let text = value.as_str().context("Violet header is not a string")?;
                    substitute_references(text, &mut |name| {
                        referenced.insert(name.to_owned());
                        name.to_owned()
                    });
                    headers.insert(name.clone(), self.expand(text));
                }
            }
            (
                changed,
                format!("{}\n", serde_json::to_string_pretty(&doc)?),
                entry["url"].as_str().unwrap().to_owned(),
                entry["type"].as_str() == Some("sse"),
            )
        } else if format == "proxy" {
            let mut doc: toml_edit::DocumentMut = raw
                .parse()
                .with_context(|| format!("parsing {}", path.display()))?;
            let repaired = self.repair_proxy_auth(&mut doc, path, report)?;
            // Unmarked files permit only the exact stale production default
            // repair. Their custom references and other content stay untouched.
            if !repaired && !raw.lines().any(|line| line.trim() == "# managed") {
                return Ok(false);
            }
            let managed = raw.lines().any(|line| line.trim() == "# managed");
            let managed_default = managed_user_default(&doc, path);
            let changed = if managed {
                retire_proxy_document(&mut doc) | repaired
            } else {
                repaired
            };
            let Some(entry) = doc
                .get_mut("servers")
                .and_then(|s| s.get_mut(VIOLET_SERVER))
                .and_then(|s| s.as_table_like_mut())
            else {
                return Ok(false);
            };
            let Some(url) = entry
                .get("url")
                .and_then(|v| v.as_str())
                .filter(|url| is_violet_hub_url(url))
                .map(str::to_owned)
            else {
                return Ok(false);
            };
            if let Some(item) = entry.get_mut("auth") {
                if let Some(name) = item
                    .as_str()
                    .and_then(|v| v.strip_prefix("env:"))
                    .map(str::to_owned)
                {
                    let replacement = name;
                    referenced.insert(replacement.clone());
                    auth = Some(if managed_default {
                        crate::cli::violet_cmd::credentials::managed_token_value()
                            .map_err(anyhow::Error::new)
                    } else {
                        self.value(&replacement)
                    });
                } else if let Some(value) = item.as_str() {
                    auth = Some(Ok(value.into()));
                }
            }
            if let Some(entries) = entry.get("headers").and_then(|h| h.as_table_like()) {
                for (name, item) in entries.iter() {
                    let value = item.as_str().context("Violet header is not a string")?;
                    let value = if let Some(name) = value.strip_prefix("env:") {
                        referenced.insert(name.into());
                        self.value(name)
                    } else {
                        Ok(value.into())
                    };
                    headers.insert(name.into(), value);
                }
            }
            (changed, doc.to_string(), url, false)
        } else {
            let mut doc: toml_edit::DocumentMut = raw
                .parse()
                .with_context(|| format!("parsing {}", path.display()))?;
            let changed =
                retire_codex_entry_with(&mut doc, &mut |name| self.rename(name, path, report));
            let Some(entry) = doc.get("mcp_servers").and_then(|s| s.get(VIOLET_SERVER)) else {
                return Ok(false);
            };
            if !entry
                .get("url")
                .and_then(|v| v.as_str())
                .is_some_and(is_violet_hub_url)
            {
                return Ok(false);
            }
            if let Some(name) = entry.get("bearer_token_env_var").and_then(|v| v.as_str()) {
                referenced.insert(name.to_owned());
                auth = Some(self.value(name));
            }
            if let Some(entries) = entry.get("http_headers").and_then(|h| h.as_table_like()) {
                for (name, value) in entries.iter() {
                    headers.insert(
                        name.into(),
                        Ok(value
                            .as_str()
                            .context("Violet header is not a string")?
                            .into()),
                    );
                }
            }
            if let Some(entries) = entry
                .get("env_http_headers")
                .and_then(|h| h.as_table_like())
            {
                for (name, value) in entries.iter() {
                    if let Some(name) = value.as_str() {
                        referenced.insert(name.to_owned());
                    }
                    headers.insert(
                        name.into(),
                        self.value(
                            value
                                .as_str()
                                .context("Violet header variable is not a string")?,
                        ),
                    );
                }
            }
            (
                changed,
                doc.to_string(),
                entry.get("url").and_then(|v| v.as_str()).unwrap().into(),
                false,
            )
        };
        // Commit repairs even when authentication cannot be verified. The
        // receipt then names the failure instead of claiming the upgrade worked.
        if changed {
            before_write(path);
            if format == "claude" {
                if !ifs::atomic_write_if_unchanged(path, &raw, modified, &rewritten)? {
                    anyhow::ensure!(
                        !retried,
                        "Claude config changed twice during Violet migration; retry cas update"
                    );
                    return self.migrate_profile_transaction(
                        path,
                        format,
                        report,
                        true,
                        before_write,
                    );
                }
            } else {
                ifs::atomic_write_create_dirs(path, &rewritten)?;
            }
        }
        let headers: std::collections::HashMap<_, _> = headers
            .into_iter()
            .map(|(name, value)| value.map(|value| (name, value)))
            .collect::<Result<_>>()?;
        let auth = auth.transpose()?;
        let authorization = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str());
        let header_auth = authorization.and_then(|value| value.strip_prefix("Bearer "));
        if authorization.is_some() {
            anyhow::ensure!(
                header_auth.is_some_and(|v| !v.trim().is_empty()),
                "Violet Authorization header has an empty bearer; no request sent"
            );
        }
        anyhow::ensure!(
            auth.as_deref()
                .or(header_auth)
                .is_some_and(|v| !v.trim().is_empty()),
            "Violet bearer is empty or unset; no request sent"
        );
        let server = if sse {
            cmcp_core::config::ServerConfig::Sse {
                url,
                auth,
                headers,
                oauth: false,
            }
        } else {
            cmcp_core::config::ServerConfig::Http {
                url,
                auth,
                headers,
                oauth: false,
            }
        };
        match self.probe.list_tools(&server) {
            ProbeOutcome::Tools {
                tools,
                schema_problems,
            } if schema_problems.is_empty()
                && VIOLET_TOOLS
                    .iter()
                    .all(|required| tools.iter().any(|t| t == required)) =>
            {
                eprintln!(
                    "Violet {}: authenticated tools/list PASS (Cassy launches) ({})",
                    path.display(),
                    tools.join(", ")
                );
                let unexported: Vec<_> = referenced
                    .into_iter()
                    .filter(|name| {
                        cas_pty::violet_env::canonical_credential(name)
                            && !std::env::var(name)
                                .ok()
                                .is_some_and(|v| !v.trim().is_empty())
                            && report.unexported.insert(name.clone())
                    })
                    .collect();
                if !unexported.is_empty() {
                    eprintln!(
                        "Violet {}: {} not exported in updater environment; bare claude/codex in a new shell may lack these variables; use cas claude / cas codex or export them",
                        path.display(),
                        unexported.join(", ")
                    );
                }
                report.authenticated.push(path.to_owned());
            }
            ProbeOutcome::Unauthorized => anyhow::bail!(
                "authenticated tools/list returned 401; check the named credential references"
            ),
            ProbeOutcome::Tools { .. } => anyhow::bail!(
                "authenticated tools/list lacks required Violet tools or a usable schema"
            ),
            ProbeOutcome::Skipped { .. } => anyhow::bail!(
                "authenticated tools/list skipped: Violet update probe time budget exhausted"
            ),
            _ => anyhow::bail!(
                "authenticated tools/list unavailable; retry cas update when the hub is reachable"
            ),
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_token(label: &str) -> String {
        format!("{}_{label}", violet_compatibility().legacy_token_prefix)
    }

    fn legacy_env_prefix() -> &'static str {
        violet_compatibility()
            .legacy_token_prefix
            .strip_suffix("SLACK_TOKEN")
            .unwrap()
    }

    #[test]
    fn cas_678d_update_rewrites_account_profile_credential_references() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        env.set("XDG_CONFIG_HOME", home.join(".config"));
        let account = home.join(".claude-support@gabber.studio");
        std::fs::create_dir_all(&account).unwrap();
        env.set("CLAUDE_CONFIG_DIR", &account);
        let path = account.join(".claude.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "mcpServers": {"violet": {"type": "http", "url": violet_hub_url(),
                    "headers": {"Authorization": format!("Bearer ${{{}}}", legacy_token("CASSY_PROXY")),
                        "x-vercel-protection-bypass": format!("${{{}}}", violet_compatibility().legacy_bypass_env)}}},
                "unrelated": true
            })
            .to_string(),
        )
        .unwrap();
        env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "fixture-token");
        env.set("VIOLET_VERCEL_BYPASS", "fixture-bypass");
        let probe = UpdateProbe {
            calls: Default::default(),
            outcome: ProbeOutcome::Tools {
                tools: VIOLET_TOOLS.map(String::from).to_vec(),
                schema_problems: vec![],
            },
        };
        migrate_installed_hub_with_probe(&probe).unwrap();
        let document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            document["mcpServers"]["violet"]["headers"]["Authorization"],
            "Bearer ${VIOLET_SLACK_TOKEN_CASSY_PROXY}"
        );
        assert_eq!(
            document["mcpServers"]["violet"]["headers"]["x-vercel-protection-bypass"],
            "${VIOLET_VERCEL_BYPASS}"
        );
        assert_eq!(document["unrelated"], true);
    }

    struct UpdateProbe {
        calls: std::cell::RefCell<Vec<cmcp_core::config::ServerConfig>>,
        outcome: ProbeOutcome,
    }

    impl HubProbe for UpdateProbe {
        fn list_tools(&self, server: &cmcp_core::config::ServerConfig) -> ProbeOutcome {
            self.calls.borrow_mut().push(server.clone());
            self.outcome.clone()
        }
    }

    #[test]
    fn cas_8235_user_update_repairs_only_a_unique_registered_account() {
        for (managed, ambiguous, missing) in [
            (true, false, false),
            (true, true, false),
            (false, false, false),
            (true, false, true),
        ] {
            let (mut env, probe) = update_fixture();
            std::fs::write(
                env.home().join(".claude.json"),
                serde_json::json!({
                    "mcpServers": {"violet": {"url": violet_hub_url(),
                        "headers": {"Authorization": "Bearer ${VIOLET_SLACK_TOKEN_SOUNDWAVE}"}}}
                })
                .to_string(),
            )
            .unwrap();
            if missing {
                std::fs::write(
                    env.home().join("credentials.env"),
                    "VIOLET_VERCEL_BYPASS=fixture-bypass\n",
                )
                .unwrap();
            }
            if ambiguous {
                env.set("VIOLET_SLACK_TOKEN_SECOND", "second-fixture");
                let alternate = env.home().join(".codex-second");
                env.set("CODEX_HOME", &alternate);
                std::fs::create_dir_all(&alternate).unwrap();
                std::fs::write(alternate.join("config.toml"), format!(
                    "[mcp_servers.violet]\nurl='{}'\nbearer_token_env_var='VIOLET_SLACK_TOKEN_SECOND'\n", violet_hub_url())).unwrap();
            }
            let path = Scope::User.config_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original = format!(
                "{}# preserve\nallowlist = ['violet.violet_read']\n[servers.violet]\ntransport = 'http'\nurl = '{}'\nauth = 'env:VIOLET_SLACK_TOKEN_CASSY_PROXY'\n",
                if managed { "# managed\n" } else { "" },
                violet_hub_url()
            );
            std::fs::write(&path, &original).unwrap();
            let report = migrate_installed_hub_with_probe(&probe).unwrap();
            let written = std::fs::read_to_string(&path).unwrap();
            if !ambiguous && !missing {
                assert!(written.contains(if managed {
                    "env:VIOLET_SLACK_TOKEN_CASSY_PROXY"
                } else {
                    "env:VIOLET_SLACK_TOKEN_SOUNDWAVE"
                }));
                assert!(written.contains("# preserve"));
                assert!(report.authenticated.contains(&path));
                if managed {
                    assert_eq!(written, original);
                    assert!(report.credential_warnings.is_empty());
                } else {
                    assert!(report.unexported.contains("VIOLET_SLACK_TOKEN_SOUNDWAVE"));
                    assert!(report.credential_warnings.iter().any(|w| w.contains(
                        "VIOLET_SLACK_TOKEN_CASSY_PROXY → VIOLET_SLACK_TOKEN_SOUNDWAVE"
                    )));
                }
            } else {
                assert_eq!(written, original);
                assert!(!report.authenticated.contains(&path));
                assert!(report.warnings.iter().any(|(p, w)| p == &path
                    && w.contains(if ambiguous {
                        "ambiguous_violet_credential"
                    } else {
                        "empty or unset"
                    })));
                assert!(report.credential_warnings.is_empty());
            }
        }
    }

    #[test]
    fn cas_678d_round4_other_managed_account_is_never_rebound() {
        let (_env, probe) = update_fixture();
        let path = Scope::User.config_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = format!(
            "# managed\n[servers.violet]\ntransport='http'\nurl='{}'\nauth='env:VIOLET_SLACK_TOKEN_OTHER_ACCOUNT'\n",
            violet_hub_url()
        );
        std::fs::write(&path, &original).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(report.authenticated.is_empty());
        assert!(probe.calls.borrow().is_empty());
    }

    #[test]
    fn cas_678d_round4_update_names_refused_credentials_path_and_fix() {
        use std::os::unix::fs::PermissionsExt;
        let (env, probe) = update_fixture();
        let path = env.home().join("credentials.env");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|(p, w)| p == &path && w.contains("chmod 600") && w.contains("owner"))
        );
        assert!(probe.calls.borrow().is_empty());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn cas_8235_review_unmarked_user_proxy_changes_only_auth_and_accepts_header_interpolation() {
        let (env, probe) = update_fixture();
        std::fs::write(env.home().join(".claude.json"), serde_json::json!({
            "mcpServers": {"violet": {"url": violet_hub_url(),
                "headers": {"Authorization": "Bearer prefix-${VIOLET_SLACK_TOKEN_SOUNDWAVE}-suffix"}}}
        }).to_string()).unwrap();
        let path = Scope::User.config_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = format!(
            "# preserve unmarked URL and policy\nallowlist=['violet.violet_read']\n[servers.violet]\ntransport='http'\nurl='{}'\nauth='env:VIOLET_SLACK_TOKEN_CASSY_PROXY'\n[servers.violet.headers]\nx-vercel-protection-bypass='env:VIOLET_VERCEL_BYPASS'\n",
            violet_compatibility().legacy_hub_url
        );
        std::fs::write(&path, &original).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(
            report.authenticated.contains(&path),
            "{:?}",
            report.warnings
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.replace(
                "'env:VIOLET_SLACK_TOKEN_CASSY_PROXY'",
                "\"env:VIOLET_SLACK_TOKEN_SOUNDWAVE\""
            )
        );
    }

    #[test]
    #[cfg(unix)]
    fn cas_8235_review_foreign_proxy_owner_is_refused_before_any_write() {
        use std::os::unix::fs::MetadataExt;
        let env = crate::test_support::TestEnvGuard::temp_home();
        let path = env.home().join("proxy.toml");
        let original = "[servers.violet]\nauth='env:VIOLET_SLACK_TOKEN_CASSY_PROXY'\n";
        std::fs::write(&path, original).unwrap();
        let uid = std::fs::symlink_metadata(&path).unwrap().uid();
        validate_proxy_owner_for_uid(&path, uid).unwrap();
        let error = validate_proxy_owner_for_uid(&path, uid.wrapping_add(1)).unwrap_err();
        assert!(error.to_string().contains("foreign_owned_violet_proxy"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    fn update_fixture() -> (crate::test_support::TestEnvGuard, UpdateProbe) {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        // Do not inherit the runner's real credentials, even as fallback candidates.
        for name in std::env::vars_os()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
        {
            if name.to_string_lossy().starts_with("VIOLET_")
                || name.to_string_lossy().starts_with(legacy_env_prefix())
            {
                env.remove(&name);
            }
        }
        let home = env.home().to_path_buf();
        env.set("XDG_CONFIG_HOME", home.join(".config"));
        env.set("SHELL", "/bin/bash");
        let credentials = home.join("credentials.env");
        std::fs::write(&credentials,
            format!("export VIOLET_SLACK_TOKEN_SOUNDWAVE='fixture-token'\nexport {}='fixture-bypass'\n", violet_compatibility().legacy_bypass_env)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        env.set("CAS_CREDENTIALS_FILE", credentials);
        env.set(legacy_token("CASSY_PROXY"), "");
        env.set(&violet_compatibility().legacy_bypass_env, "");
        env.set("VIOLET_VERCEL_BYPASS", "");
        (
            env,
            UpdateProbe {
                calls: Default::default(),
                outcome: ProbeOutcome::Tools {
                    tools: VIOLET_TOOLS.map(String::from).to_vec(),
                    schema_problems: vec![],
                },
            },
        )
    }

    fn stale_claude(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::json!({"unrelated": true,
            "mcpServers": {"violet": {"type": "http", "url": violet_hub_url(),
                "headers": {"Authorization": format!("Bearer ${{{}}}", legacy_token("CASSY_PROXY")),
                    "x-vercel-protection-bypass": format!("${{{}}}", violet_compatibility().legacy_bypass_env)}},
                "custom": {"url": "https://custom.example/mcp", "headers": {"Authorization": format!("Bearer ${{{}CUSTOM}}", legacy_env_prefix())}}}
        }).to_string()).unwrap();
    }

    #[test]
    fn cas_678d_update_migrates_and_authenticates_every_profile_without_empty_bearer() {
        let (mut env, probe) = update_fixture();
        // A bare generic variable is not a labelled machine-token candidate.
        env.set("VIOLET_SLACK_TOKEN", "unlabelled-fixture");
        let home = env.home().to_path_buf();
        let claude_profiles = [
            home.join(".claude.json"),
            home.join(".claude-support@gabber.studio/.claude.json"),
            home.join(".claude-work/.claude.json"),
        ];
        for path in &claude_profiles {
            stale_claude(path);
        }
        let external = tempfile::tempdir().unwrap();
        let external_claude = external.path().join("claude/.claude.json");
        stale_claude(&external_claude);
        env.set("CLAUDE_CONFIG_DIR", external_claude.parent().unwrap());
        let codex_profiles = [
            home.join(".codex/config.toml"),
            home.join(".codex-work/config.toml"),
            external.path().join("codex/config.toml"),
        ];
        env.set("CODEX_HOME", codex_profiles[2].parent().unwrap());
        for path in &codex_profiles {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("# keep comment\n[mcp_servers.violet]\nurl = {:?}\nbearer_token_env_var = '{}'\nenv_http_headers = {{ x-vercel-protection-bypass = '{}' }}\n", violet_hub_url(), legacy_token("CASSY_PROXY"), violet_compatibility().legacy_bypass_env)).unwrap();
        }
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.authenticated.len(), 7);
        assert_eq!(probe.calls.borrow().len(), 7);
        assert!(report.credential_warnings.iter().any(|warning| {
            warning.contains(&format!("{} →", legacy_token("CASSY_PROXY")))
                && warning.contains("VIOLET_SLACK_TOKEN_SOUNDWAVE")
        }));
        for path in claude_profiles
            .iter()
            .chain(std::iter::once(&external_claude))
        {
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                doc["mcpServers"]["violet"]["headers"]["Authorization"],
                "Bearer ${VIOLET_SLACK_TOKEN_SOUNDWAVE}"
            );
            assert_eq!(
                doc["mcpServers"]["violet"]["headers"]["x-vercel-protection-bypass"],
                "${VIOLET_VERCEL_BYPASS}"
            );
            assert_eq!(
                doc["mcpServers"]["custom"]["headers"]["Authorization"],
                format!("Bearer ${{{}CUSTOM}}", legacy_env_prefix())
            );
            assert_eq!(doc["unrelated"], true);
        }
        for path in &codex_profiles {
            let raw = std::fs::read_to_string(path).unwrap();
            let doc: toml::Value = toml::from_str(&raw).unwrap();
            assert!(raw.contains("# keep comment"));
            assert_eq!(
                doc["mcp_servers"]["violet"]["bearer_token_env_var"].as_str(),
                Some("VIOLET_SLACK_TOKEN_SOUNDWAVE")
            );
            assert_eq!(
                doc["mcp_servers"]["violet"]["env_http_headers"]["x-vercel-protection-bypass"]
                    .as_str(),
                Some("VIOLET_VERCEL_BYPASS")
            );
        }
        for server in probe.calls.borrow().iter() {
            let cmcp_core::config::ServerConfig::Http { auth, headers, .. } = server else {
                panic!("expected HTTP");
            };
            assert_eq!(
                auth.as_deref().or_else(|| headers
                    .get("Authorization")
                    .and_then(|s| s.strip_prefix("Bearer "))),
                Some("fixture-token")
            );
            assert_eq!(
                headers
                    .get("x-vercel-protection-bypass")
                    .map(String::as_str),
                Some("fixture-bypass")
            );
        }
        let credentials = std::fs::read_to_string(home.join("credentials.env")).unwrap();
        assert!(!credentials.contains(legacy_env_prefix()));
        assert!(credentials.contains("VIOLET_VERCEL_BYPASS"));
        let again = migrate_installed_hub_with_probe(&probe).unwrap();
        assert_eq!(again.migrated, 0);
        assert_eq!(again.authenticated.len(), 7);
    }

    #[test]
    fn cas_678d_update_diagnoses_missing_ambiguous_and_rejected_credentials() {
        for mode in ["empty", "unset", "selector-only", "ambiguous", "rejected"] {
            let (mut env, mut probe) = update_fixture();
            let path = env
                .home()
                .join(".claude-support@gabber.studio/.claude.json");
            stale_claude(&path);
            let credentials = env.home().join("credentials.env");
            if mode == "rejected" {
                probe.outcome = ProbeOutcome::Unauthorized;
            } else {
                std::fs::write(
                    &credentials,
                    "export VIOLET_VERCEL_BYPASS='fixture-bypass'\n",
                )
                .unwrap();
                if mode == "empty" {
                    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "");
                }
                if mode == "selector-only" {
                    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_MISSING");
                }
                if mode == "ambiguous" {
                    env.set("VIOLET_SLACK_TOKEN_ONE", "fixture-one");
                    env.set("VIOLET_SLACK_TOKEN_TWO", "fixture-two");
                }
            }
            let report = migrate_installed_hub_with_probe(&probe).unwrap();
            assert!(report.authenticated.is_empty());
            assert_eq!(report.warnings.len(), 1, "{mode}");
            let warning = &report.warnings[0].1;
            if mode == "rejected" {
                assert_eq!(probe.calls.borrow().len(), 1);
                assert!(warning.contains("401"));
            } else {
                assert!(probe.calls.borrow().is_empty(), "{mode}");
                assert!(
                    warning.contains(&legacy_token("CASSY_PROXY")),
                    "{warning}"
                );
                assert!(warning.contains("no request sent"));
            }
            assert!(!warning.contains("fixture-token"));
            assert!(!warning.contains("fixture-bypass"));
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert!(doc["mcpServers"]["violet"].is_object());
        }
    }

    #[test]
    fn cas_678d_update_rejects_empty_header_even_with_a_usable_bearer_variable() {
        let (env, probe) = update_fixture();
        let path = env.home().join(".codex/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!(
            "[mcp_servers.violet]\nurl = {:?}\nbearer_token_env_var = 'VIOLET_SLACK_TOKEN_SOUNDWAVE'\nhttp_headers = {{ Authorization = 'Bearer ' }}\n",
            violet_hub_url())).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(probe.calls.borrow().is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(
            report.warnings[0]
                .1
                .contains("Authorization header has an empty bearer")
        );
        assert!(report.warnings[0].1.contains("no request sent"));
    }

    #[test]
    fn cas_678d_round1_private_legacy_value_keeps_own_canonical_name() {
        let (env, probe) = update_fixture();
        let file = env.home().join("credentials.env");
        let raw = std::fs::read_to_string(&file).unwrap();
        std::fs::write(
            file,
            format!("{raw}export {}='fixture-account'\n", legacy_token("ACCOUNT")),
        )
        .unwrap();
        let path = env.home().join(".claude-account/.claude.json");
        stale_claude(&path);
        let raw = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&legacy_token("CASSY_PROXY"), &legacy_token("ACCOUNT"));
        std::fs::write(&path, raw).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(report.warnings.is_empty());
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            doc["mcpServers"]["violet"]["headers"]["Authorization"],
            "Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"
        );
    }

    #[test]
    fn cas_678d_round1_claude_concurrent_write_retries_once_and_preserves_changes() {
        let (env, probe) = update_fixture();
        let path = env.home().join(".claude-account/.claude.json");
        stale_claude(&path);
        let values = super::super::violet::update_machine_credentials().unwrap();
        let update = UpdateMigration::new(env.home(), values, &probe);
        let writes = std::cell::Cell::new(0);
        let hook = |path: &Path| {
            if writes.replace(writes.get() + 1) == 0 {
                let mut doc: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                doc["concurrent-setting"] = true.into();
                std::fs::write(path, doc.to_string()).unwrap();
            }
        };
        let mut report = ProfileRetirement::default();
        assert!(
            update
                .migrate_profile_transaction(&path, "claude", &mut report, false, &hook)
                .unwrap()
        );
        assert_eq!(writes.get(), 2);
        assert_eq!(
            report.credential_warnings.len(),
            2,
            "retry must deduplicate token and bypass mappings"
        );
        assert_eq!(probe.calls.borrow().len(), 1);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["concurrent-setting"], true);
        let hook = |path: &Path| {
            let raw = std::fs::read_to_string(path).unwrap();
            std::fs::write(path, format!("{raw}\n")).unwrap();
        };
        stale_claude(&path);
        assert!(
            update
                .migrate_profile_transaction(&path, "claude", &mut report, false, &hook)
                .unwrap_err()
                .to_string()
                .contains("changed twice")
        );
        assert_eq!(probe.calls.borrow().len(), 1);
    }

    #[test]
    fn cas_678d_round1_expired_probe_budget_does_not_contact_the_hub() {
        let probe = UpdateBudgetProbe {
            deadline: std::time::Instant::now(),
        };
        let server = cmcp_core::config::ServerConfig::Http {
            url: violet_hub_url().into(),
            auth: Some("fixture".into()),
            headers: Default::default(),
            oauth: false,
        };
        assert!(matches!(
            probe.list_tools(&server),
            ProbeOutcome::Skipped { .. }
        ));
    }

    #[test]
    fn cas_678d_round1_unset_canonical_account_is_never_rebound() {
        let (env, probe) = update_fixture();
        let path = env.home().join(".claude-account/.claude.json");
        stale_claude(&path);
        let raw = std::fs::read_to_string(&path).unwrap().replace(
            &legacy_token("CASSY_PROXY"),
            "VIOLET_SLACK_TOKEN_ACCOUNT",
        );
        std::fs::write(&path, raw).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(probe.calls.borrow().is_empty());
        assert!(
            report
                .warnings
                .iter()
                .any(|(_, warning)| warning.contains("VIOLET_SLACK_TOKEN_ACCOUNT"))
        );
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            doc["mcpServers"]["violet"]["headers"]["Authorization"],
            "Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"
        );
    }

    #[test]
    fn cas_678d_round1_preferred_token_does_not_break_ambiguity() {
        let (mut env, probe) = update_fixture();
        env.set("VIOLET_SLACK_TOKEN_ACCOUNT", "fixture-account");
        env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_SOUNDWAVE");
        let path = env.home().join(".claude-account/.claude.json");
        stale_claude(&path);
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(probe.calls.borrow().is_empty());
        assert!(
            report
                .warnings
                .iter()
                .any(|(_, warning)| warning.contains("ambiguous"))
        );
    }

    #[test]
    fn cas_678d_update_preserves_working_account_credentials_and_custom_endpoints() {
        let (mut env, probe) = update_fixture();
        env.set("VIOLET_SLACK_TOKEN_ACCOUNT", "fixture-account");
        env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_SOUNDWAVE");
        let path = env.home().join(".claude-account/.claude.json");
        stale_claude(&path);
        let raw = std::fs::read_to_string(&path).unwrap().replace(
            &legacy_token("CASSY_PROXY"),
            "VIOLET_SLACK_TOKEN_ACCOUNT",
        );
        std::fs::write(&path, raw).unwrap();
        let custom = env.home().join(".claude-custom/.claude.json");
        stale_claude(&custom);
        let custom_raw = std::fs::read_to_string(&custom)
            .unwrap()
            .replace(violet_hub_url(), "https://staging.example/mcp");
        std::fs::write(&custom, &custom_raw).unwrap();
        let report = migrate_installed_hub_with_probe(&probe).unwrap();
        assert!(report.warnings.is_empty());
        assert_eq!(report.authenticated.len(), 1);
        assert_eq!(std::fs::read_to_string(custom).unwrap(), custom_raw);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            doc["mcpServers"]["violet"]["headers"]["Authorization"],
            "Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"
        );
    }

    fn codex_fixture(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!(
            "# keep account settings\n[mcp_servers.{}]\nurl = {:?}\nbearer_token_env_var = \"VIOLET_ACCOUNT_TOKEN\"\n",
            violet_compatibility().retired_server, violet_hub_url()
        )).unwrap();
    }

    fn assert_codex_migrated(path: &Path) {
        let raw = std::fs::read_to_string(path).unwrap();
        let document: toml::Value = toml::from_str(&raw).unwrap();
        assert!(raw.contains("# keep account settings"));
        assert!(
            document["mcp_servers"]
                .get(&violet_compatibility().retired_server)
                .is_none()
        );
        assert_eq!(
            document["mcp_servers"]["violet"]["url"].as_str(),
            Some(violet_hub_url())
        );
        assert_eq!(
            document["mcp_servers"]["violet"]["bearer_token_env_var"].as_str(),
            Some("VIOLET_ACCOUNT_TOKEN")
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_profile_symlink_migrates_target_and_preserves_link_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("shared/config.toml");
        codex_fixture(&target);
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = home.path().join(".codex-account/config.toml");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();
        let report = retire_machine_profiles(home.path(), &[]);
        assert!(report.warnings.is_empty());
        assert_eq!(report.inspected, 1);
        assert_eq!(report.migrated, 1);
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_codex_migrated(&link);
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn shared_codex_profile_target_is_inspected_and_written_once() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("shared/config.toml");
        codex_fixture(&target);
        for (profile, destination) in [
            (".codex-a", target.clone()),
            (".codex-b", PathBuf::from("../shared/config.toml")),
        ] {
            let link = home.path().join(profile).join("config.toml");
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&destination, &link).unwrap();
        }
        let report = retire_machine_profiles(home.path(), &[]);
        assert!(report.warnings.is_empty());
        assert_eq!(
            report.inspected, 1,
            "aliases must not even reread their shared target"
        );
        assert_eq!(report.migrated, 1);
        for profile in [".codex-a", ".codex-b"] {
            let link = home.path().join(profile).join("config.toml");
            assert!(link.is_symlink());
            assert_codex_migrated(&link);
        }
        let again = retire_machine_profiles(home.path(), &[]);
        assert_eq!(again.inspected, 1);
        assert_eq!(again.migrated, 0);
        assert!(again.warnings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_and_invalid_profiles_warn_and_other_accounts_still_migrate() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_target = outside.path().join("config.toml");
        codex_fixture(&outside_target);
        let outside_before = std::fs::read(&outside_target).unwrap();
        let dangling = home.path().join(".codex-dangling/config.toml");
        let escaped = home.path().join(".codex-outside/config.toml");
        let cycle = home.path().join(".codex-cycle/config.toml");
        for path in [&dangling, &escaped, &cycle] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        symlink("missing.toml", &dangling).unwrap();
        symlink(&outside_target, &escaped).unwrap();
        symlink("config.toml", &cycle).unwrap();
        let invalid = home.path().join(".codex-invalid/config.toml");
        std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        let invalid_text = "token = \"dont-log-this-credential\"\ninvalid }\n";
        std::fs::write(&invalid, invalid_text).unwrap();
        let directory = home.path().join(".codex-directory/config.toml");
        std::fs::create_dir_all(&directory).unwrap();
        let good = home.path().join(".codex-valid/config.toml");
        codex_fixture(&good);
        let report = retire_machine_profiles(home.path(), &[]);
        assert_eq!(report.warnings.len(), 5);
        for path in [&dangling, &escaped, &cycle, &invalid, &directory] {
            assert!(
                report
                    .warnings
                    .iter()
                    .any(|(warning_path, _)| warning_path == path),
                "no warning for {}",
                path.display()
            );
        }
        assert!(
            report
                .warnings
                .iter()
                .any(|(_, reason)| reason.contains("outside HOME"))
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|(_, reason)| reason.contains("dangling"))
        );
        assert!(
            report
                .warnings
                .iter()
                .all(|(_, reason)| !reason.contains("dont-log-this-credential"))
        );
        assert_codex_migrated(&good);
        assert_eq!(std::fs::read(outside_target).unwrap(), outside_before);
        assert_eq!(std::fs::read_to_string(invalid).unwrap(), invalid_text);
        assert!(dangling.is_symlink());
        assert!(escaped.is_symlink());
        assert!(cycle.is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn claude_profile_symlink_migrates_without_replacing_link() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("shared.json");
        let contract = violet_compatibility();
        std::fs::write(&target, serde_json::json!({"mcpServers":{(contract.retired_server.as_str()):{"url":violet_hub_url(), "headers":{"Authorization":"Bearer ${VIOLET_ACCOUNT_TOKEN}"}}}}).to_string()).unwrap();
        let link = home.path().join(".claude-account/.claude.json");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let report = retire_machine_profiles(home.path(), &[]);
        assert_eq!(report.migrated, 1);
        assert!(report.warnings.is_empty());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        let document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&link).unwrap()).unwrap();
        assert!(
            document["mcpServers"]
                .get(&contract.retired_server)
                .is_none()
        );
        assert_eq!(
            document["mcpServers"]["violet"]["headers"]["Authorization"],
            "Bearer ${VIOLET_ACCOUNT_TOKEN}"
        );
    }

    #[test]
    fn explicit_profile_directory_outside_home_still_migrates() {
        let home = tempfile::tempdir().unwrap();
        let codex_home = tempfile::tempdir().unwrap();
        let config = codex_home.path().join("config.toml");
        codex_fixture(&config);
        let report =
            retire_machine_profiles(home.path(), &[("codex", codex_home.path().to_path_buf())]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.migrated, 1);
        assert_codex_migrated(&config);
    }

    #[cfg(unix)]
    #[test]
    fn profile_and_proxy_failures_do_not_abort_unrelated_project_sync() {
        use clap::Parser;
        let temp = tempfile::tempdir().unwrap();
        // Held for the whole test and dropped before `temp`: scrubs ambient
        // CODEX_HOME/CLAUDE_CONFIG_DIR so the real profiles are never touched.
        let mut env = crate::test_support::TestEnvGuard::new();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let bad = home.join(".codex-bad/config.toml");
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("missing.toml", &bad).unwrap();
        let good = home.join(".codex/config.toml");
        codex_fixture(&good);
        std::fs::create_dir_all(project.join(".cas")).unwrap();
        std::fs::write(project.join(".cas/proxy.toml"), "invalid }\n").unwrap();
        std::fs::write(
            project.join("CLAUDE.md"),
            "# Fixture\n\nProject-specific guidance.\n",
        )
        .unwrap();
        env.set("HOME", &home);
        env.set("XDG_CONFIG_HOME", home.join(".config"));
        env.set("CAS_ROOT", project.join(".cas"));
        env.set_current_dir(&project);
        let cli = crate::cli::Cli::parse_from(["cas", "sync", "agents-md", "--write"]);
        crate::cli::sync::execute(
            &crate::cli::sync::SyncCommands::AgentsMd(crate::cli::sync::AgentsMdArgs {
                check: false,
                write: true,
            }),
            &cli,
        )
        .unwrap();
        assert!(
            std::fs::read_to_string(project.join("AGENTS.md"))
                .unwrap()
                .contains("Project-specific guidance.")
        );
        assert!(bad.is_symlink());
        assert_codex_migrated(&good);
        crate::builtins::sync_all_builtins_for_project(cas_mux::SupervisorCli::Codex, &project)
            .unwrap();
        assert!(project.join(".codex/skills/cas-worker/SKILL.md").is_file());
    }

    #[test]
    fn all_machine_profiles_retire_production_entries_preserving_credentials_and_custom_upstreams()
    {
        let temp = tempfile::tempdir().unwrap();
        let contract = violet_compatibility();
        for profile in ["", ".claude", ".claude-alt", ".claude-work"] {
            let dir = temp.path().join(profile);
            std::fs::create_dir_all(&dir).unwrap();
            let entry = serde_json::json!({"type":"http", "url":contract.legacy_hub_url, "headers":{"Authorization":format!("Bearer ${{{}_LAPTOP}}", contract.legacy_token_prefix)}});
            let path = dir.join(".claude.json");
            std::fs::write(&path, serde_json::json!({"unrelated":true,"mcpServers":{(contract.retired_server.as_str()):entry,"custom":{"url":"https://custom.example/mcp"}}}).to_string()).unwrap();
        }
        for profile in [".codex", ".codex-work"] {
            let dir = temp.path().join(profile);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("config.toml"),
                format!(
                    "# keep comment\n[mcp_servers.{}]\nurl = {:?}\nbearer_token_env_var = {:?}\n",
                    contract.retired_server,
                    contract.legacy_hub_url,
                    format!("{}_LAPTOP", contract.legacy_token_prefix)
                ),
            )
            .unwrap();
        }
        let retired_skill = temp
            .path()
            .join(".agents/skills")
            .join(&contract.retired_server);
        std::fs::create_dir_all(&retired_skill).unwrap();
        std::fs::write(
            retired_skill.join("SKILL.md"),
            "---\nmetadata:\n  managed_by: cas\n---\nOld redirect\n",
        )
        .unwrap();
        let unrelated_skill = temp.path().join(".agents/skills/cas-worker");
        std::fs::create_dir_all(&unrelated_skill).unwrap();
        std::fs::write(
            unrelated_skill.join("SKILL.md"),
            "---\nmetadata:\n  managed_by: cas\n---\nWorker\n",
        )
        .unwrap();
        assert!(
            retire_machine_profiles(temp.path(), &[])
                .warnings
                .is_empty()
        );
        assert!(!retired_skill.exists());
        assert!(unrelated_skill.exists());
        for profile in ["", ".claude", ".claude-alt", ".claude-work"] {
            let path = temp.path().join(profile).join(".claude.json");
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(doc["mcpServers"].get(&contract.retired_server).is_none());
            assert_eq!(doc["mcpServers"]["violet"]["url"], violet_hub_url());
            // Generic sync retires URLs only; user update resolves credential changes.
            assert_eq!(
                doc["mcpServers"]["violet"]["headers"]["Authorization"],
                format!("Bearer ${{{}_LAPTOP}}", contract.legacy_token_prefix)
            );
            assert_eq!(doc["unrelated"], true);
            assert!(doc["mcpServers"].get("custom").is_some());
        }
        for profile in [".codex", ".codex-work"] {
            let raw =
                std::fs::read_to_string(temp.path().join(profile).join("config.toml")).unwrap();
            let doc: toml::Value = toml::from_str(&raw).unwrap();
            assert!(raw.contains("# keep comment"));
            assert!(doc["mcp_servers"].get(&contract.retired_server).is_none());
            assert_eq!(
                doc["mcp_servers"]["violet"]["url"].as_str(),
                Some(violet_hub_url())
            );
            assert_eq!(
                doc["mcp_servers"]["violet"]["bearer_token_env_var"].as_str(),
                Some(format!("{}_LAPTOP", contract.legacy_token_prefix).as_str())
            );
        }
        let custom = temp.path().join("custom.json");
        let original = serde_json::json!({"mcpServers":{(contract.retired_server.as_str()):{"url":"https://custom.example/mcp"}}}).to_string();
        std::fs::write(&custom, &original).unwrap();
        assert!(!retire_file(&custom, "claude").unwrap());
        assert_eq!(std::fs::read_to_string(custom).unwrap(), original);
        assert!(
            retire_machine_profiles(temp.path(), &[])
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn project_proxy_retirement_preserves_scoped_policy_comments_and_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("proxy.toml");
        let c = violet_compatibility();
        let original = format!(
            "# keep comment\nallowlist = [\"supervisor:{}.{}\", \"neon.*\"]\nworker_read_routes = [\"{}.{}\"]\n[worker_access]\n{} = \"read-only\"\n[servers.{}]\ntransport = \"http\"\nurl = {:?}\nauth = {:?}\n",
            c.retired_server,
            c.retired_tools[0],
            c.retired_server,
            c.retired_tools[0],
            c.retired_server,
            c.retired_server,
            c.legacy_hub_url,
            format!("env:{}_LAPTOP", c.legacy_token_prefix)
        );
        std::fs::write(&path, &original).unwrap();
        assert!(retire_project_proxy(&path).unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        let config = cmcp_core::config::Config::load_from(&path).unwrap();
        assert!(raw.contains("# keep comment"));
        assert!(!config.servers.contains_key(&c.retired_server));
        // The proxy resolves the legacy variable as a fallback, so the file
        // moves to the canonical endpoint and names in one step.
        assert_eq!(
            config.violet_env_names().unwrap().0.as_deref(),
            Some("VIOLET_SLACK_TOKEN_LAPTOP")
        );
        assert!(raw.contains(violet_hub_url()), "{raw}");
        for legacy in [
            c.legacy_hub_url.as_str(),
            c.legacy_token_prefix.as_str(),
            c.retired_server.as_str(),
        ] {
            assert!(!raw.contains(legacy), "{legacy} survived:\n{raw}");
        }
        assert!(config.allowlist.iter().any(|route| route.server == "violet"
            && route.tool == "violet_read"
            && route.supervisor_only));
        assert!(
            config
                .allowlist
                .iter()
                .any(|route| route.server == "neon" && route.tool == "*")
        );
        assert_eq!(
            config.worker_access.get("violet"),
            Some(&cmcp_core::config::WorkerAccess::ReadOnly)
        );
        assert!(!retire_project_proxy(&path).unwrap());
        let custom = original.replace(&c.legacy_hub_url, "https://custom.example/mcp");
        std::fs::write(&path, &custom).unwrap();
        assert!(!retire_project_proxy(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let opt_out = original.replace(
            &format!(
                "\"supervisor:{}.{}\", \"neon.*\"",
                c.retired_server, c.retired_tools[0]
            ),
            "",
        );
        std::fs::write(&path, opt_out).unwrap();
        retire_project_proxy(&path).unwrap();
        assert!(
            cmcp_core::config::Config::load_from(&path)
                .unwrap()
                .allowlist
                .is_empty()
        );
    }

    #[test]
    fn installed_violet_entries_move_to_violet_hub_on_update() {
        let temp = tempfile::tempdir().unwrap();
        let c = violet_compatibility();
        let proxy = temp.path().join("proxy.toml");
        let original = format!(
            "# managed\nallowlist = [\"violet.violet_read\", \"violet.violet_post\"]\n\n[servers.violet]\ntransport = \"http\"\nurl = \"{}\"\nauth = \"env:{}_CASSY_PROXY\"\n\n[servers.violet.headers]\nx-vercel-protection-bypass = \"env:{}\"\n",
            c.legacy_hub_url, c.legacy_token_prefix, c.legacy_bypass_env
        );
        std::fs::write(&proxy, &original).unwrap();
        assert!(retire_project_proxy(&proxy).unwrap());
        let raw = std::fs::read_to_string(&proxy).unwrap();
        assert_eq!(
            raw,
            format!(
                "# managed\nallowlist = [\"violet.violet_read\", \"violet.violet_post\"]\n\n[servers.violet]\ntransport = \"http\"\nurl = \"{}\"\nauth = \"env:VIOLET_SLACK_TOKEN_CASSY_PROXY\"\n\n[servers.violet.headers]\nx-vercel-protection-bypass = \"env:VIOLET_VERCEL_BYPASS\"\n",
                violet_hub_url()
            ),
            "only the URL and credential names change"
        );
        assert!(!retire_project_proxy(&proxy).unwrap(), "idempotent");

        let codex = temp.path().join("config.toml");
        std::fs::write(
            &codex,
            format!(
                "[mcp_servers.violet]\nurl = \"{}\"\nbearer_token_env_var = \"VIOLET_SLACK_TOKEN_LAPTOP\"\n",
                c.legacy_hub_url
            ),
        )
        .unwrap();
        assert!(retire_file(&codex, "codex").unwrap());
        assert!(!retire_file(&codex, "codex").unwrap());
        assert!(
            std::fs::read_to_string(&codex)
                .unwrap()
                .contains(violet_hub_url())
        );

        let claude = temp.path().join(".claude.json");
        std::fs::write(
            &claude,
            serde_json::json!({"mcpServers":{"violet":{"type":"http","url":c.legacy_hub_url}}})
                .to_string(),
        )
        .unwrap();
        assert!(retire_file(&claude, "claude").unwrap());
        assert!(!retire_file(&claude, "claude").unwrap());
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["violet"]["url"], violet_hub_url());

        // A staging hub is operator-owned under any credential names.
        let staging = original.replace(&c.legacy_hub_url, "https://staging.example/mcp/slack");
        std::fs::write(&proxy, &staging).unwrap();
        assert!(!retire_project_proxy(&proxy).unwrap());
        assert_eq!(std::fs::read_to_string(&proxy).unwrap(), staging);
    }
}
