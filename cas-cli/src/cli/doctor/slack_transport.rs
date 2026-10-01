//! Read-only local evidence for a Codex Slack app exposed outside hook policy.
use super::{Check, CheckStatus};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn has_slack_tools(value: &Value) -> bool {
    value
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("server_name").and_then(Value::as_str) == Some("codex_apps")
                    && (tool
                        .get("connector_name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case("slack"))
                        || tool
                            .get("tool_name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| name.to_ascii_lowercase().contains("slack")))
            })
        })
}

fn has_accessible_slack(value: &Value) -> bool {
    value
        .get("connectors")
        .and_then(Value::as_array)
        .is_some_and(|connectors| {
            connectors.iter().any(|connector| {
                connector
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case("slack"))
                    && connector.get("isAccessible").and_then(Value::as_bool) == Some(true)
                    && connector.get("isEnabled").and_then(Value::as_bool) != Some(false)
            })
        })
}

fn metadata_finding(home: &Path) -> Option<&'static str> {
    for (directory, predicate, finding) in [
        (
            "codex_apps_tools",
            has_slack_tools as fn(&Value) -> bool,
            "cached codex_apps Slack tools",
        ),
        (
            "codex_app_directory",
            has_accessible_slack as fn(&Value) -> bool,
            "cached accessible Codex Slack app",
        ),
    ] {
        let Ok(entries) = std::fs::read_dir(home.join("cache").join(directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            // Directory caches can be large, but never read an unbounded file.
            if std::fs::metadata(&path).map_or(true, |meta| meta.len() > 32 * 1024 * 1024) {
                continue;
            }
            let Some(value) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            else {
                continue;
            };
            if predicate(&value) {
                return Some(finding);
            }
        }
    }
    None
}

pub(super) fn check_for_homes(homes: &[PathBuf]) -> Check {
    let findings: Vec<_> = homes
        .iter()
        .filter_map(|home| {
            metadata_finding(home).map(|finding| format!("{finding} in {}", home.display()))
        })
        .collect();
    if findings.is_empty() {
        return Check::new(
            "Slack transport",
            CheckStatus::Info,
            "no accessible Slack app in local Codex metadata; live account linkage is not probed",
        );
    }
    Check::new(
        "Slack transport",
        CheckStatus::Warning,
        format!(
            "{} (cache may be stale); use violet.violet_read / violet.violet_post and the `violet` skill, never the Codex Slack app. Codex MCP writes may bypass pre-tool hooks",
            findings.join("; ")
        ),
    )
}

pub(super) fn check() -> Check {
    let mut homes = Vec::new();
    if let Some(home) = dirs::home_dir() {
        homes.push(home.join(".codex"));
        if let Ok(entries) = std::fs::read_dir(&home) {
            homes.extend(entries.flatten().filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .filter(|name| name.starts_with(".codex-"))
                    .map(|_| entry.path())
            }));
        }
    }
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        homes.push(home.into());
    }
    homes.sort();
    homes.dedup();
    check_for_homes(&homes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn linked_codex_slack_tool_metadata_warns_and_names_violet() {
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("cache/codex_apps_tools");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("account.json"), json!({"tools":[{"server_name":"codex_apps", "connector_name":"Slack", "tool_name":"_slack_send_message"}]}).to_string()).unwrap();
        let check = check_for_homes(&[home.path().into()]);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("violet.violet_post"));
        assert!(check.message.contains("cache may be stale"));
    }

    #[test]
    fn catalogue_listing_is_not_evidence_of_linkage() {
        assert!(!has_accessible_slack(
            &json!({"connectors":[{"name":"Slack", "isAccessible":false, "isEnabled":true}]})
        ));
        assert!(has_accessible_slack(
            &json!({"connectors":[{"name":"Slack", "isAccessible":true, "isEnabled":true}]})
        ));
        assert!(!has_accessible_slack(
            &json!({"connectors":[{"name":"Slack", "isAccessible":true, "isEnabled":false}]})
        ));
        assert!(!has_slack_tools(
            &json!({"tools":[{"server_name":"violet", "connector_name":"Slack"}]})
        ));
        let home = tempfile::tempdir().unwrap();
        assert!(matches!(
            check_for_homes(&[home.path().into()]).status,
            CheckStatus::Info
        ));
    }
}
