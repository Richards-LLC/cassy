//! Remove only registrations for the retired production hub. Custom upstreams,
//! credentials, restrictive policy, comments and unrelated settings survive.
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

/// Enumerate installed machine profiles, including alternate Claude/Codex
/// accounts. Explicit profile directories are included even outside HOME.
fn retire_machine_profiles(home: &Path, extra: &[(&str, PathBuf)]) -> Result<()> {
    let mut profiles = vec![("claude", home.to_path_buf())];
    if home.is_dir() {
        for entry in std::fs::read_dir(home)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
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
    profiles.push(("skills", home.join(".agents")));
    profiles.push(("skills", home.join(".grok")));
    profiles.extend(extra.iter().cloned());
    profiles.sort();
    profiles.dedup();
    for (format, directory) in profiles {
        if format != "skills" {
            retire_file(
                &directory.join(if format == "claude" {
                    ".claude.json"
                } else {
                    "config.toml"
                }),
                format,
            )?;
        }
        let skills_dir = if directory == home {
            home.join(".claude/skills")
        } else {
            directory.join("skills")
        };
        crate::builtins::prune_retired_hub_skills(&skills_dir)?;
    }
    Ok(())
}

pub fn retire_installed_hub(project_proxy: Option<&Path>) -> Result<()> {
    if let Some(home) = dirs::home_dir() {
        let extra: Vec<_> = [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")]
            .into_iter()
            .filter_map(|(format, name)| {
                std::env::var_os(name).map(|path| (format, PathBuf::from(path)))
            })
            .collect();
        retire_machine_profiles(&home, &extra)?;
    }
    if let Ok(user) = Scope::User.config_path() {
        retire_project_proxy(&user)?;
    }
    if let Some(project) = project_proxy {
        retire_project_proxy(project)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        retire_machine_profiles(temp.path(), &[]).unwrap();
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
        retire_machine_profiles(temp.path(), &[]).unwrap();
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
