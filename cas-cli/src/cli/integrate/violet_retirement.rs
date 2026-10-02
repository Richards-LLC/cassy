//! Remove only registrations for the retired production hub. Custom upstreams,
//! credentials, restrictive policy, comments and unrelated settings survive.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cas_types::violet_compatibility::{violet_compatibility, violet_hub_url};
use cmcp_core::config::{ExternalToolConfig, Scope, VIOLET_SERVER, VIOLET_TOOLS};

use super::fs as ifs;

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
    if legacy.get("url").and_then(|item| item.as_str()) != Some(violet_hub_url()) {
        return false;
    }
    let legacy = legacy.clone();
    if !servers.contains_key(VIOLET_SERVER) {
        servers.insert(VIOLET_SERVER, legacy);
    }
    servers.remove(&contract.retired_server);
    true
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
    let contract = violet_compatibility();
    let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(|item| item.as_object_mut())
    else {
        return false;
    };
    if servers
        .get(&contract.retired_server)
        .and_then(|server| server.get("url"))
        .and_then(|url| url.as_str())
        != Some(violet_hub_url())
    {
        return false;
    }
    let legacy = servers
        .remove(&contract.retired_server)
        .expect("checked above");
    servers.entry(VIOLET_SERVER.to_owned()).or_insert(legacy);
    true
}

pub(super) fn retire_claude_entry(document: &mut serde_json::Value) -> bool {
    retire_claude_document(document)
}
pub(super) fn retire_codex_entry(document: &mut toml_edit::DocumentMut) -> bool {
    retire_toml_entry(document, "mcp_servers")
}

fn retire_file(path: &Path, format: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
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

#[derive(Default)]
struct ProfileRetirement {
    inspected: usize,
    migrated: usize,
    warnings: Vec<(PathBuf, String)>,
}

impl ProfileRetirement {
    fn warn(&mut self, path: &Path, error: impl std::fmt::Display) {
        // Display only the outer error context: parser chains may contain
        // credential values from the file being parsed.
        let reason = error.to_string();
        eprintln!(
            "warning: Violet migration skipped {}: {reason}",
            path.display()
        );
        self.warnings.push((path.to_path_buf(), reason));
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
                    match retire_file(&target, format) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let entry = serde_json::json!({"type":"http", "url":violet_hub_url(), "headers":{"Authorization":format!("Bearer ${{{}_LAPTOP}}", contract.legacy_token_prefix)}});
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
                    violet_hub_url(),
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
            violet_hub_url(),
            format!("env:{}_LAPTOP", c.legacy_token_prefix)
        );
        std::fs::write(&path, &original).unwrap();
        assert!(retire_project_proxy(&path).unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        let config = cmcp_core::config::Config::load_from(&path).unwrap();
        assert!(raw.contains("# keep comment"));
        assert!(!config.servers.contains_key(&c.retired_server));
        assert_eq!(
            config.violet_env_names().unwrap().0,
            Some(format!("{}_LAPTOP", c.legacy_token_prefix))
        );
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
        let custom = original.replace(violet_hub_url(), "https://custom.example/mcp");
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
}
