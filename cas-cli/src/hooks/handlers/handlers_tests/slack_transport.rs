use crate::config::{Config, SlackConfig, SlackTransport};
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};
use serde_json::{Value, json};

fn input(tool: &str, args: Value, cwd: &std::path::Path) -> HookInput {
    HookInput {
        session_id: "slack-policy-session".into(),
        cwd: cwd.display().to_string(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(args),
        ..HookInput::default()
    }
}

fn reason(output: HookOutput) -> Option<String> {
    let value = serde_json::to_value(output).unwrap();
    (value
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
        == Some("deny"))
    .then(|| {
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .unwrap()
            .as_str()
            .unwrap()
            .into()
    })
}

#[test]
fn slack_policy_denies_claude_send_outside_factory_without_cas_root() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().unwrap();
    env.set("XDG_CONFIG_HOME", config_home.path());
    for harness in ["claude", "grok", "codex"] {
        env.set("CAS_HOOK_HARNESS", harness);
        let deny = reason(
            handle_pre_tool_use(
                &input(
                    "mcp__claude_ai_Slack__slack_send_message",
                    json!({"text":"release note"}),
                    config_home.path(),
                ),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(deny.contains("violet.violet_post"));
        assert!(deny.contains("`violet` skill"));
        assert!(deny.contains("violet.violet_read"));
    }
}

#[test]
fn slack_policy_allows_violet_proxy_and_reads() {
    let _env = TestEnvGuard::temp_home();
    let cwd = tempfile::tempdir().unwrap();
    for tool in [
        "mcp__violet__violet_post",
        "violet.violet_post",
        "mcp__claude_ai_Slack__slack_read_channel",
    ] {
        assert!(
            reason(handle_pre_tool_use(&input(tool, json!({}), cwd.path()), None).unwrap())
                .is_none(),
            "{tool}"
        );
    }
    assert!(
        reason(
            handle_pre_tool_use(
                &input(
                    "mcp__cs__mcp_execute",
                    json!({"code":"violet.violet_post({})"}),
                    cwd.path()
                ),
                None
            )
            .unwrap()
        )
        .is_none()
    );
}

#[test]
fn slack_policy_any_bypasses_and_default_reenables() {
    let _env = TestEnvGuard::temp_home();
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.set("slack.transport", "any").unwrap();
    config.save_toml(root.path()).unwrap();
    let send = input(
        "mcp__claude_ai_Slack__slack_send_message",
        json!({}),
        root.path(),
    );
    assert!(reason(handle_pre_tool_use(&send, Some(root.path())).unwrap()).is_none());
    config.set("slack.transport", "violet").unwrap();
    config.save_toml(root.path()).unwrap();
    assert!(
        reason(handle_pre_tool_use(&send, Some(root.path())).unwrap())
            .unwrap()
            .contains("slack.transport=violet")
    );
}

#[test]
fn slack_transport_registry_get_set_list_merge_and_roundtrip() {
    let mut config = Config::default();
    assert_eq!(config.get("slack.transport").as_deref(), Some("violet"));
    assert!(
        config
            .list()
            .contains(&("slack.transport".into(), "violet".into()))
    );
    config.set("slack.transport", "any").unwrap();
    assert!(config.set("slack.transport", "invalid").is_err());
    let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(roundtrip.slack.unwrap().transport, SlackTransport::Any);
    let mut empty = Config::default();
    empty.merge_missing(&config);
    assert_eq!(empty.get("slack.transport").as_deref(), Some("any"));
    config.slack = Some(SlackConfig {
        transport: SlackTransport::Violet,
        ..SlackConfig::default()
    });
    config.merge_missing(&empty);
    assert_eq!(config.get("slack.transport").as_deref(), Some("violet"));
    assert!(
        crate::config::registry()
            .generate_markdown()
            .contains("slack.transport")
    );
}

#[test]
fn global_slack_opt_out_applies_without_project_and_project_can_override() {
    let mut env = TestEnvGuard::temp_home();
    let home = tempfile::tempdir().unwrap();
    env.set("XDG_CONFIG_HOME", home.path());
    let global = crate::config::global_cas_dir().unwrap();
    std::fs::create_dir_all(&global).unwrap();
    let mut config = Config::default();
    config.set("slack.transport", "any").unwrap();
    config.save_toml(&global).unwrap();
    let send = input("mcp__slack__send_message", json!({}), home.path());
    assert!(reason(handle_pre_tool_use(&send, None).unwrap()).is_none());
    let project = tempfile::tempdir().unwrap();
    config.set("slack.transport", "violet").unwrap();
    config.save_toml(project.path()).unwrap();
    assert!(reason(handle_pre_tool_use(&send, Some(project.path())).unwrap()).is_some());
}
