use super::{config_gen::get_cas_hooks_config, configure_claude_hooks_with_config_dirs};
use crate::config::HookConfig;
use crate::hooks::{HookInput, handlers::handle_session_start};
use crate::test_support::TestEnvGuard;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=CAS Test",
            "-c",
            "user.email=cas@example.test",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn legacy_matchers_do_not_dirty_tracked_settings_on_update_or_session_start_cas_c91e() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    env.remove("CAS_CLONE_PATH");
    env.remove("CAS_AGENT_NAME");
    let project = tempfile::tempdir().unwrap();
    let repo = project.path();
    let cas_root = crate::store::init_cas_dir(repo).unwrap();
    env.set("CAS_ROOT", &cas_root);
    // The persisted defaults observed read-only in the main checkout.
    let legacy_config = r#"
[hooks.post_tool_use]
matcher = ["Write", "Edit", "Bash"]
[hooks.pre_tool_use]
matcher = ["Read", "Glob", "Grep", "Write", "Edit", "Bash", "WebFetch", "WebSearch", "Task", "SendMessage"]
"#;
    std::fs::write(cas_root.join("config.toml"), legacy_config).unwrap();
    let settings_path = repo.join(".claude/settings.json");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let tracked = include_str!("../../../../.claude/settings.json");
    std::fs::write(&settings_path, tracked).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["add", ".claude/settings.json"]);
    git(
        repo,
        &["commit", "-q", "-m", "tracked canonical hook settings"],
    );

    // This is the production writer used by cas update --sync and cas init.
    configure_claude_hooks_with_config_dirs(repo, false, &[]).unwrap();
    let generated = std::fs::read_to_string(&settings_path).unwrap();
    let expected: serde_json::Value = serde_json::from_str(tracked).unwrap();
    let actual: serde_json::Value = serde_json::from_str(&generated).unwrap();
    for event in ["PreToolUse", "PostToolUse"] {
        assert_eq!(
            actual["hooks"][event][0]["matcher"], expected["hooks"][event][0]["matcher"],
            "{event} rewrote the canonical tracked matcher"
        );
    }
    assert_eq!(
        generated, tracked,
        "update must preserve every tracked byte"
    );
    let input = HookInput {
        session_id: "cas-c91e-session".into(),
        cwd: repo.to_string_lossy().into_owned(),
        hook_event_name: "SessionStart".into(),
        ..Default::default()
    };
    handle_session_start(&input, Some(&cas_root)).unwrap();
    configure_claude_hooks_with_config_dirs(repo, false, &[]).unwrap();
    assert_eq!(std::fs::read_to_string(&settings_path).unwrap(), tracked);
    assert!(
        git(
            repo,
            &["status", "--porcelain", "--", ".claude/settings.json"]
        )
        .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(cas_root.join("config.toml")).unwrap(),
        legacy_config
    );
}

#[test]
fn custom_and_disabled_matchers_remain_operator_choices_cas_c91e() {
    let mut config = HookConfig::default();
    config.pre_tool_use.matcher = vec!["NotebookEdit".into(), "Read".into()];
    config.post_tool_use.matcher = vec!["NotebookEdit".into(), "Bash".into()];
    let generated = get_cas_hooks_config(&config);
    let pre = generated["hooks"]["PreToolUse"][0]["matcher"]
        .as_str()
        .unwrap();
    assert!(pre.starts_with("NotebookEdit|Read|"));
    assert!(pre.ends_with(crate::config::hooks::SLACK_POLICY_MATCHER));
    assert_eq!(
        generated["hooks"]["PostToolUse"][0]["matcher"],
        "NotebookEdit|Bash"
    );
    config.pre_tool_use.enabled = false;
    config.post_tool_use.enabled = false;
    let disabled = get_cas_hooks_config(&config);
    assert_eq!(
        disabled["hooks"]["PreToolUse"][0]["matcher"],
        crate::config::hooks::SLACK_POLICY_MATCHER
    );
    assert!(disabled["hooks"].get("PostToolUse").is_none());
}
