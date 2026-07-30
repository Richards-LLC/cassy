//! Integration tests for the cas-mcp-proxy (code-mode-mcp) crate.
//!
//! These tests verify the config API, catalog serialization format,
//! and compatibility with the proxy_catalog.json cache consumed by
//! SessionStart context injection.

#![cfg(feature = "mcp-proxy")]

use std::collections::HashMap;
use std::path::Path;

use cmcp_core::config::{Config, Scope, ServerConfig};
use cmcp_core::{CatalogEntry, ProxyEngine, UpstreamState};

mod support;
use support::CasSandbox;

// ── Config round-trip ────────────────────────────────────────────────

#[test]
fn config_round_trip_all_transports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proxy.toml");

    let mut config = Config::default();

    config.add_server(
        "my-stdio".to_string(),
        ServerConfig::Stdio {
            command: "npx".to_string(),
            args: vec!["mcp-server-git".to_string()],
            env: HashMap::from([("HOME".to_string(), "/tmp".to_string())]),
        },
    );

    config.add_server(
        "my-http".to_string(),
        ServerConfig::Http {
            url: "https://mcp.example.com/api".to_string(),
            auth: Some("secret-token".to_string()),
            headers: HashMap::new(),
            oauth: false,
        },
    );

    config.add_server(
        "my-sse".to_string(),
        ServerConfig::Sse {
            url: "https://mcp.example.com/sse".to_string(),
            auth: None,
            headers: HashMap::from([("X-Custom".to_string(), "value".to_string())]),
            oauth: true,
        },
    );

    config.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(config, loaded);
    assert_eq!(loaded.servers.len(), 3);
}

#[test]
fn config_add_remove_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proxy.toml");

    let mut config = Config::default();
    config.add_server(
        "srv".to_string(),
        ServerConfig::Stdio {
            command: "old".to_string(),
            args: vec![],
            env: HashMap::new(),
        },
    );
    config.save_to(&path).unwrap();

    // Overwrite with new config
    config.add_server(
        "srv".to_string(),
        ServerConfig::Stdio {
            command: "new".to_string(),
            args: vec!["--flag".to_string()],
            env: HashMap::new(),
        },
    );
    config.save_to(&path).unwrap();

    let loaded = Config::load_from(&path).unwrap();
    match &loaded.servers["srv"] {
        ServerConfig::Stdio { command, args, .. } => {
            assert_eq!(command, "new");
            assert_eq!(args, &["--flag"]);
        }
        _ => panic!("expected Stdio"),
    }

    // Remove
    let mut loaded = loaded;
    assert!(loaded.remove_server("srv"));
    assert!(!loaded.remove_server("srv")); // Already gone
    loaded.save_to(&path).unwrap();

    let final_config = Config::load_from(&path).unwrap();
    assert!(final_config.servers.is_empty());
}

#[test]
fn config_load_missing_returns_empty() {
    let config = Config::load_from(Path::new("/tmp/nonexistent-cas-test/proxy.toml")).unwrap();
    assert!(config.servers.is_empty());
}

#[test]
fn config_merge_project_over_user() {
    let dir = tempfile::tempdir().unwrap();

    // Simulate project config
    let project_path = dir.path().join("project.toml");
    let mut project = Config::default();
    project.add_server(
        "shared".to_string(),
        ServerConfig::Http {
            url: "https://project.example.com".to_string(),
            auth: None,
            headers: HashMap::new(),
            oauth: false,
        },
    );
    project.save_to(&project_path).unwrap();

    // load_merged with project path
    let merged = Config::load_merged(Some(&project_path)).unwrap();
    assert!(merged.servers.contains_key("shared"));
}

#[test]
fn scope_user_config_path_valid() {
    let path = Scope::User.config_path().unwrap();
    assert!(path.to_string_lossy().contains("code-mode-mcp"));
    assert!(path.to_string_lossy().ends_with("config.toml"));
}

// ── Catalog serialization ────────────────────────────────────────────

#[test]
fn catalog_entry_serializes_to_json() {
    let entry = CatalogEntry {
        name: "take_screenshot".to_string(),
        description: Some("Captures a screenshot of the page".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" }
            }
        }),
    };

    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["name"], "take_screenshot");
    assert_eq!(json["description"], "Captures a screenshot of the page");
    assert!(json["input_schema"]["properties"]["url"].is_object());
}

#[test]
fn catalog_entries_by_server_format_compatible_with_cache() {
    // The proxy_catalog.json cache format expected by build_mcp_tools_section
    // is: { "server_name": ["tool1", "tool2"] }
    // write_proxy_catalog_cache converts CatalogEntry → just names.
    // Verify that our CatalogEntry.name is what gets written.

    let entries = vec![
        CatalogEntry {
            name: "navigate_page".to_string(),
            description: Some("Navigate to URL".to_string()),
            input_schema: serde_json::json!({}),
        },
        CatalogEntry {
            name: "take_screenshot".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        },
    ];

    // Simulate the conversion done in write_proxy_catalog_cache
    let mut catalog: HashMap<String, Vec<String>> = HashMap::new();
    catalog.insert(
        "chrome-devtools".to_string(),
        entries.iter().map(|e| e.name.clone()).collect(),
    );

    let json = serde_json::to_string(&catalog).unwrap();

    // Verify it can be deserialized as BTreeMap<String, Vec<String>>
    // (the format build_mcp_tools_section expects)
    let parsed: std::collections::BTreeMap<String, Vec<String>> =
        serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["chrome-devtools"].len(), 2);
    assert!(parsed["chrome-devtools"].contains(&"navigate_page".to_string()));
    assert!(parsed["chrome-devtools"].contains(&"take_screenshot".to_string()));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connected_upstream_tool_error_then_transport_failure_recovers_once() {
    let sandbox = CasSandbox::new();
    let pid_file = sandbox.path().join("proxy-upstream.pid");
    let script = r#"echo $$ > "$CAS_TEST_PID_FILE"
exec env \
  -u CAS_AGENT_ROLE -u CAS_FACTORY_MODE \
  -u CAS_FACTORY_SUPERVISOR_CLI -u CAS_FACTORY_WORKER_CLI \
  -u CAS_SESSION_ID -u CAS_FACTORY_SESSION -u CAS_AGENT_ID -u CAS_TASK_ID \
  "$CAS_TEST_BIN" serve"#;
    let config = ServerConfig::Stdio {
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        env: HashMap::from([
            (
                "CAS_TEST_BIN".to_string(),
                env!("CARGO_BIN_EXE_cas").to_string(),
            ),
            (
                "CAS_TEST_PID_FILE".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ),
            (
                "CAS_ROOT".to_string(),
                sandbox.cas_root().to_string_lossy().into_owned(),
            ),
            (
                "CAS_DIR".to_string(),
                sandbox.cas_root().to_string_lossy().into_owned(),
            ),
            (
                "CLAUDE_PROJECT_DIR".to_string(),
                sandbox.path().to_string_lossy().into_owned(),
            ),
            (
                "HOME".to_string(),
                sandbox.home_dir().to_string_lossy().into_owned(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                sandbox.xdg_config_home().to_string_lossy().into_owned(),
            ),
        ]),
    };
    let engine = ProxyEngine::from_configs(HashMap::from([("optional-live".to_string(), config)]))
        .await
        .unwrap();
    assert_eq!(
        engine.health_snapshot().await.servers[0].state,
        UpstreamState::Healthy
    );

    let protocol_error = engine
        .call_tool(
            "optional-live",
            "task",
            Some(serde_json::Map::from_iter([
                (
                    "action".to_string(),
                    serde_json::Value::String("show".to_string()),
                ),
                (
                    "id".to_string(),
                    serde_json::Value::String("cas-does-not-exist".to_string()),
                ),
            ])),
        )
        .await;
    assert!(protocol_error.is_err());
    assert_eq!(
        engine.health_snapshot().await.servers[0].state,
        UpstreamState::Healthy,
        "upstream MCP application errors must not degrade transport health"
    );

    let created = engine
        .call_tool(
            "optional-live",
            "task",
            Some(serde_json::Map::from_iter([
                (
                    "action".to_string(),
                    serde_json::Value::String("create".to_string()),
                ),
                (
                    "title".to_string(),
                    serde_json::Value::String("proxy tool error fixture".to_string()),
                ),
            ])),
        )
        .await
        .expect("create fixture task through live upstream");
    let created_text = created["content"][0]["text"].as_str().unwrap();
    let task_id = created_text
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .find(|part| part.starts_with("cas-"))
        .expect("created task response contains task id");
    let tool_error = engine
        .call_tool(
            "optional-live",
            "task",
            Some(serde_json::Map::from_iter([
                (
                    "action".to_string(),
                    serde_json::Value::String("close".to_string()),
                ),
                (
                    "id".to_string(),
                    serde_json::Value::String(task_id.to_string()),
                ),
                (
                    "completion_receipt".to_string(),
                    serde_json::Value::String(
                        serde_json::json!({
                            "task_id": "cas-wrong",
                            "worker_agent_id": "worker-test",
                            "proof_reference": "proof:test",
                            "scope_summary": "test",
                            "repo_selector": "project:test",
                            "source_branch": "factory/test",
                            "commit_sha": "0000000000000000000000000000000000000000",
                            "merge_base_sha": "0000000000000000000000000000000000000000",
                            "target_branch": "main",
                            "target_sha": "0000000000000000000000000000000000000000"
                        })
                        .to_string(),
                    ),
                ),
            ])),
        )
        .await
        .expect("normal upstream tool error remains a protocol success");
    assert_eq!(tool_error["isError"], true);
    assert_eq!(
        engine.health_snapshot().await.servers[0].state,
        UpstreamState::Healthy,
        "tool-level isError must not degrade connection health"
    );

    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", pid.trim()])
            .status()
            .unwrap()
            .success()
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let failed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(
            engine.call_tool("optional-live", "task", None),
            engine.call_tool("optional-live", "task", None)
        )
    })
    .await
    .expect("dead upstream calls must fail within a bounded interval");
    assert!(failed.0.is_err());
    assert!(failed.1.is_err());

    let degraded = engine.health_snapshot().await.servers.remove(0);
    assert_eq!(degraded.state, UpstreamState::Backoff);
    assert_eq!(degraded.attempts, 2);
    assert_eq!(
        degraded.consecutive_failures, 1,
        "simultaneous failures from one generation must increment once"
    );
    assert_eq!(
        degraded.last_error_code.as_deref(),
        Some("connection_failed")
    );
    let due = degraded.next_retry_at_ms.unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    tokio::time::sleep(std::time::Duration::from_millis(
        due.saturating_sub(now).saturating_add(50),
    ))
    .await;

    assert_eq!(engine.retry_unhealthy().await, 1);
    let recovered = engine.health_snapshot().await.servers.remove(0);
    assert_eq!(recovered.state, UpstreamState::Healthy);
    assert_eq!(recovered.consecutive_failures, 0);
    assert_eq!(recovered.next_retry_at_ms, None);
    assert!(recovered.tool_count > 0);
    engine.shutdown().await;
}
