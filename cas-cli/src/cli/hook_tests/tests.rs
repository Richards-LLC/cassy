use crate::cli::hook::*;
use crate::cli::hook::config_gen::{get_cas_hooks_config, has_cas_hook_entries};
use crate::cli::hook::configure_claude_hooks_with_home;
use crate::config::HookConfig;
use tempfile::TempDir;
use toml::map::Map;

/// Create a TempDir that acts as an isolated $HOME with no global settings.
/// Pass this to `configure_claude_hooks_with_home` / `global_has_cas_hooks_in`
/// so tests never read the real `~/.claude/settings.json` (cas-1888).
fn isolated_home() -> TempDir {
    TempDir::new().unwrap()
}

#[test]
fn test_configure_creates_settings() {
    // Use an isolated fake home so global_has_cas_hooks_in always returns false
    // regardless of the developer's real ~/.claude/settings.json (cas-1888).
    let fake_home = isolated_home();
    let temp = TempDir::new().unwrap();
    let result = configure_claude_hooks_with_home(temp.path(), false, Some(fake_home.path())).unwrap();

    assert!(result); // Created new file
    assert!(temp.path().join(".claude/settings.json").exists());

    let content = std::fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

    // With isolated home (no global settings), hooks must always be written.
    assert!(settings.pointer("/hooks/SessionStart").is_some());
    assert!(settings.pointer("/hooks/SessionEnd").is_some());
    assert!(settings.pointer("/hooks/Stop").is_some());
    assert!(settings.pointer("/hooks/SubagentStop").is_some());
    assert!(settings.pointer("/hooks/PostToolUse").is_some());
    assert!(settings.pointer("/hooks/UserPromptSubmit").is_some());

    // Shell-form fixture: hook entries must carry a "command" string and NO
    // "args" array. /doctor on CC 2.1.159 rejects type:"command" hooks that
    // lack a string `command`, so the malformed cas-9a60 exec-form
    // (`args` only, no `command`) silently disabled every hook. cas-c17b
    // convergence: this matches teams.rs::factory_hooks_block.
    let session_start_cmd = first_hook_command(&settings, "SessionStart");
    assert_eq!(
        session_start_cmd,
        Some("cas hook SessionStart"),
        "cas init should emit shell-form command for SessionStart hook"
    );
    assert_eq!(
        first_hook_args(&settings, "SessionStart"),
        None,
        "cas init SessionStart hook must not carry an args array"
    );
    let stop_cmd = first_hook_command(&settings, "Stop");
    assert_eq!(
        stop_cmd,
        Some("cas hook Stop"),
        "cas init should emit shell-form command for Stop hook"
    );
    assert_eq!(
        first_hook_args(&settings, "Stop"),
        None,
        "cas init Stop hook must not carry an args array"
    );

    // Permissions should always be written
    let allow = settings
        .pointer("/permissions/allow")
        .expect("permissions.allow missing");
    let allow_arr = allow.as_array().expect("permissions.allow is not array");
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Bash(cas :*)")),
        "Bash(cas :*) permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__task")),
        "mcp__cas__task permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__coordination")),
        "mcp__cas__coordination permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__memory")),
        "mcp__cas__memory permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__search")),
        "mcp__cas__search permission missing"
    );
}

#[test]
fn test_configure_merges_existing() {
    // Isolated home → global_has_cas_hooks_in always returns false (cas-1888).
    let fake_home = isolated_home();
    let temp = TempDir::new().unwrap();
    let claude_dir = temp.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    // Create existing settings with custom content
    let existing = serde_json::json!({
        "permissions": {
            "allow": ["Read", "Write"]
        },
        "hooks": {
            "CustomHook": [{"hooks": [{"type": "command", "command": "echo custom"}]}]
        }
    });
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    // Configure CAS hooks using isolated home so the check is deterministic.
    let result = configure_claude_hooks_with_home(temp.path(), false, Some(fake_home.path())).unwrap();
    assert!(!result); // Updated, not created

    let content = std::fs::read_to_string(claude_dir.join("settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Isolated home → no global hooks → CAS hooks must be added.
    assert!(settings.pointer("/hooks/SessionStart").is_some());
    assert!(settings.pointer("/hooks/Stop").is_some());
    assert!(settings.pointer("/hooks/PostToolUse").is_some());

    // Existing permissions should always be preserved and CAS permissions added
    let allow = settings
        .pointer("/permissions/allow")
        .expect("permissions.allow missing");
    let allow_arr = allow.as_array().expect("permissions.allow is not array");

    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Read")),
        "Original Read permission should be preserved"
    );
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Write")),
        "Original Write permission should be preserved"
    );
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Bash(cas :*)")),
        "Bash(cas :*) permission should be added"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__task")),
        "mcp__cas__task permission should be added"
    );
}

#[test]
fn test_strip_cas_hooks() {
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}],
            "SessionStart": [
                {"hooks": [{"type": "command", "command": "cas hook SessionStart"}]},
                {"hooks": [{"type": "command", "command": "cas factory check-staleness"}]}
            ],
            "CustomHook": [{"hooks": [{"type": "command", "command": "echo custom"}]}]
        },
        "permissions": {"allow": ["Read"]}
    });

    let modified = strip_cas_hooks(&mut settings);
    assert!(modified);

    // CAS hooks should be removed
    assert!(settings.pointer("/hooks/PreToolUse").is_none());
    assert!(settings.pointer("/hooks/SessionStart").is_none());

    // Non-CAS hook should be preserved
    assert!(settings.pointer("/hooks/CustomHook").is_some());

    // Permissions should be untouched
    assert!(settings.pointer("/permissions/allow").is_some());
}

#[test]
fn test_strip_cas_hooks_removes_empty_hooks_object() {
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        },
        "permissions": {"allow": ["Read"]}
    });

    strip_cas_hooks(&mut settings);

    // hooks object should be completely removed when empty
    assert!(settings.get("hooks").is_none());
    assert!(settings.get("permissions").is_some());
}

#[test]
fn test_has_cas_hook_entries() {
    let with_hooks = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        }
    });
    assert!(has_cas_hook_entries(&with_hooks));

    let without_hooks = serde_json::json!({
        "hooks": {
            "Custom": [{"hooks": [{"type": "command", "command": "echo test"}]}]
        }
    });
    assert!(!has_cas_hook_entries(&without_hooks));

    let no_hooks = serde_json::json!({"permissions": {}});
    assert!(!has_cas_hook_entries(&no_hooks));
}

#[test]
fn test_configure_codex_creates_config() {
    let temp = TempDir::new().unwrap();
    let result = configure_codex_mcp_server(temp.path()).unwrap();

    assert!(result);
    let config_path = temp.path().join(".codex/config.toml");
    assert!(config_path.exists());

    let content = std::fs::read_to_string(&config_path).unwrap();
    let config: toml::Value = toml::from_str(&content).unwrap();
    let entry = config
        .get("mcp_servers")
        .and_then(|v| v.get("cas"))
        .and_then(|v| v.as_table())
        .expect("mcp_servers.cas missing");

    assert_eq!(
        entry.get("command"),
        Some(&toml::Value::String("cas".to_string()))
    );
    assert_eq!(
        entry.get("args"),
        Some(&toml::Value::Array(vec![toml::Value::String(
            "serve".to_string()
        )]))
    );
    assert_eq!(
        entry.get("env"),
        Some(&toml::Value::Table({
            let mut env = Map::new();
            env.insert(
                "CAS_CODEX_FALLBACK_SESSION".to_string(),
                toml::Value::String("1".to_string()),
            );
            env
        }))
    );

    let hooks_path = temp.path().join(".codex/hooks.json");
    assert!(hooks_path.exists());
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hooks_path).unwrap()).unwrap();
    assert_eq!(
        hooks.pointer("/hooks/PreToolUse/0/matcher"),
        Some(&serde_json::json!("^Bash$"))
    );
    assert_eq!(
        hooks.pointer("/hooks/PreToolUse/0/hooks/0/command"),
        Some(&serde_json::json!("cas hook PreToolUse"))
    );
    assert_eq!(
        hooks.pointer("/hooks/PreToolUse/0/hooks/0/timeout"),
        Some(&serde_json::json!(3))
    );
    assert_eq!(
        hooks.pointer("/hooks/PostToolUse/0/matcher"),
        Some(&serde_json::json!("^Bash$"))
    );
    assert_eq!(
        hooks.pointer("/hooks/PostToolUse/0/hooks/0/type"),
        Some(&serde_json::json!("command"))
    );
    assert_eq!(
        hooks.pointer("/hooks/PostToolUse/0/hooks/0/command"),
        Some(&serde_json::json!("cas hook PostToolUse"))
    );
    assert_eq!(
        hooks.pointer("/hooks/PostToolUse/0/hooks/0/timeout"),
        Some(&serde_json::json!(3))
    );
}

#[test]
fn test_configure_codex_updates_existing_entry() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();

    let content = r#"
[mcp_servers.context7]
command = "cas"
args = ["old"]
env = { CAS_LOG = "debug" }
"#;
    std::fs::write(codex_dir.join("config.toml"), content).unwrap();

    let result = configure_codex_mcp_server(temp.path()).unwrap();
    assert!(result);

    let updated = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&updated).unwrap();
    let entry = config
        .get("mcp_servers")
        .and_then(|v| v.get("context7"))
        .and_then(|v| v.as_table())
        .expect("mcp_servers.context7 missing");

    assert_eq!(
        entry.get("command"),
        Some(&toml::Value::String("cas".to_string()))
    );
    assert_eq!(
        entry.get("args"),
        Some(&toml::Value::Array(vec![toml::Value::String(
            "serve".to_string()
        )]))
    );
    assert_eq!(
        entry.get("env"),
        Some(&toml::Value::Table({
            let mut env = Map::new();
            env.insert(
                "CAS_LOG".to_string(),
                toml::Value::String("debug".to_string()),
            );
            env.insert(
                "CAS_CODEX_FALLBACK_SESSION".to_string(),
                toml::Value::String("1".to_string()),
            );
            env
        }))
    );
}

#[test]
fn test_configure_codex_merges_hooks_and_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let existing = serde_json::json!({
        "description": "Keep this metadata",
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "^apply_patch$",
                    "hooks": [{
                        "type": "command",
                        "command": "custom pre-tool hook"
                    }]
                }
            ],
            "PostToolUse": [
                {
                    "matcher": "^apply_patch$",
                    "hooks": [{
                        "type": "command",
                        "command": "custom post-tool hook"
                    }]
                },
                {
                    "matcher": ".*",
                    "hooks": [{
                        "type": "command",
                        "command": "cas hook PostToolUse",
                        "timeout": 999
                    }]
                }
            ],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": "custom stop hook"
                }]
            }]
        }
    });
    std::fs::write(
        codex_dir.join("hooks.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    assert!(configure_codex_mcp_server(temp.path()).unwrap());
    let hooks_path = codex_dir.join("hooks.json");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
    assert_eq!(
        hooks.get("description"),
        Some(&serde_json::json!("Keep this metadata"))
    );
    assert_eq!(
        hooks.pointer("/hooks/Stop/0/hooks/0/command"),
        Some(&serde_json::json!("custom stop hook"))
    );
    let post_tool = hooks
        .pointer("/hooks/PostToolUse")
        .and_then(|value| value.as_array())
        .expect("PostToolUse array missing");
    assert_eq!(post_tool.len(), 2, "custom hook plus one CAS hook");
    assert_eq!(
        post_tool[0].pointer("/hooks/0/command"),
        Some(&serde_json::json!("custom post-tool hook"))
    );
    assert_eq!(
        post_tool[1].get("matcher"),
        Some(&serde_json::json!("^Bash$"))
    );
    assert_eq!(
        post_tool[1].pointer("/hooks/0/timeout"),
        Some(&serde_json::json!(3))
    );
    let pre_tool = hooks
        .pointer("/hooks/PreToolUse")
        .and_then(|value| value.as_array())
        .expect("PreToolUse array missing");
    assert_eq!(pre_tool.len(), 2, "custom hook plus one CAS hook");
    assert_eq!(
        pre_tool[0].pointer("/hooks/0/command"),
        Some(&serde_json::json!("custom pre-tool hook"))
    );
    assert_eq!(
        pre_tool[1].get("matcher"),
        Some(&serde_json::json!("^Bash$"))
    );
    assert_eq!(
        pre_tool[1].pointer("/hooks/0/command"),
        Some(&serde_json::json!("cas hook PreToolUse"))
    );
    assert_eq!(
        pre_tool[1].pointer("/hooks/0/timeout"),
        Some(&serde_json::json!(3))
    );

    assert!(
        !configure_codex_mcp_server(temp.path()).unwrap(),
        "second configuration pass should not rewrite either Codex file"
    );
}

// Note: configure_mcp_server tests removed because they require the claude CLI
// which isn't available in test environments. The function now uses `claude mcp add`.

// =============================================================================
// Characterization tests for hook emission format (shell-form)
//
// get_cas_hooks_config emits shell-form `"command": "cas hook <Event>"`
// (and `"command": "cas factory check-staleness"`), converging on the same
// shape as ui/factory/daemon/runtime/teams.rs::factory_hooks_block (cas-c17b).
//
// The cas-9a60 exec-form attempt emitted `{"type":"command","args":[...]}`
// with NO top-level `command` string. That is malformed: CC's /doctor requires
// a string `command` for every type:"command" hook, so it rejected all 12
// entries and the harness silently disabled every CAS hook (see
// docs/requests/BUG-hooks-exec-form-missing-command.md). #58441 closing was a
// red herring — valid exec-form is `{"command":"cas","args":[...]}` and
// `command` is required regardless.
//
// Both legacy on-disk forms (malformed exec-form `args[0]=="cas"` and
// shell-form) remain recognised by has_cas_hook_entries / strip_cas_hooks so
// users upgrade cleanly on the next `cas init`.
// =============================================================================

/// Extract the first hook entry's "command" value for a given event name.
/// Returns None when the event is absent or the hook has no "command" key
/// (i.e. it is already using exec-form "args").
fn first_hook_command<'a>(config: &'a serde_json::Value, event: &str) -> Option<&'a str> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .iter()
        .find_map(|entry| {
            entry
                .get("hooks")?
                .as_array()?
                .iter()
                .find_map(|h| h.get("command")?.as_str())
        })
}

/// Extract the first hook entry's "args" array for a given event name.
/// Returns None when the event is absent or the hook has no "args" key.
fn first_hook_args<'a>(config: &'a serde_json::Value, event: &str) -> Option<Vec<&'a str>> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .iter()
        .find_map(|entry| {
            entry.get("hooks")?.as_array()?.iter().find_map(|h| {
                let args = h.get("args")?.as_array()?;
                Some(args.iter().filter_map(|v| v.as_str()).collect())
            })
        })
}

/// Extract the "command" value of the `idx`-th top-level hook registration
/// for a given event name (0-indexed).  Used to reach the second SessionStart
/// entry (`check-staleness`) which `first_hook_command` cannot reach.
fn nth_hook_command<'a>(
    config: &'a serde_json::Value,
    event: &str,
    idx: usize,
) -> Option<&'a str> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .get(idx)?
        .get("hooks")?
        .as_array()?
        .iter()
        .find_map(|h| h.get("command")?.as_str())
}

/// Extract the "args" array of the `idx`-th top-level hook registration
/// for a given event name (0-indexed).  Mirror of `nth_hook_command` for
/// exec-form entries that carry `"args"` instead of `"command"`.
fn nth_hook_args<'a>(
    config: &'a serde_json::Value,
    event: &str,
    idx: usize,
) -> Option<Vec<&'a str>> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .get(idx)?
        .get("hooks")?
        .as_array()?
        .iter()
        .find_map(|h| {
            let args = h.get("args")?.as_array()?;
            Some(args.iter().filter_map(|v| v.as_str()).collect())
        })
}

/// AC#2 — every event hook emitted by get_cas_hooks_config carries the
/// shell-form `"command": "cas hook <Event>"` string. /doctor on CC 2.1.159
/// requires this string; the malformed cas-9a60 exec-form lacked it.
#[test]
fn hook_entries_emit_shell_form_command() {
    let config = get_cas_hooks_config(&HookConfig::default());

    for (event, expected_command) in &[
        ("SessionStart", "cas hook SessionStart"),
        ("SessionEnd", "cas hook SessionEnd"),
        ("Stop", "cas hook Stop"),
        ("SubagentStart", "cas hook SubagentStart"),
        ("SubagentStop", "cas hook SubagentStop"),
        ("PostToolUse", "cas hook PostToolUse"),
        ("PostToolUseFailure", "cas hook PostToolUseFailure"),
        ("PreToolUse", "cas hook PreToolUse"),
        ("UserPromptSubmit", "cas hook UserPromptSubmit"),
        ("PermissionRequest", "cas hook PermissionRequest"),
        ("PermissionDenied", "cas hook PermissionDenied"),
        ("Notification", "cas hook Notification"),
        ("PreCompact", "cas hook PreCompact"),
    ] {
        assert_eq!(
            first_hook_command(&config, event),
            Some(*expected_command),
            "{event} hook must carry shell-form command string"
        );
    }
}

/// AC#2 — no event hook leaks an `"args"` array. The malformed cas-9a60
/// exec-form put the executable in args[0] with no top-level command; this
/// guards against any regression back to that shape.
#[test]
fn hook_entries_do_not_emit_args_array() {
    let config = get_cas_hooks_config(&HookConfig::default());

    for event in &[
        "SessionStart",
        "SessionEnd",
        "Stop",
        "SubagentStart",
        "SubagentStop",
        "PostToolUse",
        "PostToolUseFailure",
        "PreToolUse",
        "UserPromptSubmit",
        "PermissionRequest",
        "PermissionDenied",
        "Notification",
        "PreCompact",
    ] {
        assert_eq!(
            first_hook_args(&config, event),
            None,
            "{event} hook must not carry an exec-form args array"
        );
    }
}

/// AC#2 — exhaustive shape check: walk EVERY hook object under
/// `hooks.*[].hooks[]` and assert each has a string `command` and NO `args`
/// key. This catches any future entry that forgets `command` or reintroduces
/// `args`, including ones the per-event helpers above don't cover.
#[test]
fn every_emitted_hook_object_has_command_and_no_args() {
    let config = get_cas_hooks_config(&HookConfig::default());
    let hooks = config
        .get("hooks")
        .and_then(|h| h.as_object())
        .expect("hooks object missing");

    let mut hook_objects = 0usize;
    for (event, entries) in hooks {
        let entries = entries
            .as_array()
            .unwrap_or_else(|| panic!("{event} entries is not an array"));
        for entry in entries {
            let hook_list = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .unwrap_or_else(|| panic!("{event} entry missing hooks array"));
            for hook in hook_list {
                hook_objects += 1;
                let cmd = hook.get("command").and_then(|c| c.as_str());
                assert!(
                    cmd.is_some(),
                    "{event} hook object lacks string command: {hook}"
                );
                assert!(
                    hook.get("args").is_none(),
                    "{event} hook object must not carry args key: {hook}"
                );
                assert_eq!(hook.get("type").and_then(|t| t.as_str()), Some("command"));
            }
        }
    }

    // 14 events x 1 hook object (including sealed-handoff terminal cleanup),
    // plus the extra SessionStart staleness entry = 15.
    assert_eq!(
        hook_objects, 15,
        "expected exactly 15 hook objects (14 events + factory check-staleness)"
    );
}

#[test]
fn verifier_subagent_start_binding_is_synchronous() {
    let config = get_cas_hooks_config(&HookConfig::default());
    let hook = config["hooks"]["SubagentStart"][0]["hooks"][0]
        .as_object()
        .expect("SubagentStart hook object");
    assert!(
        hook.get("async").is_none(),
        "sealed handoff must bind before the verifier child's first turn"
    );
}

/// AC#2 — the second SessionStart entry is the factory staleness check, and it
/// emits exactly `cas factory check-staleness` in shell-form with no args.
#[test]
fn session_start_check_staleness_emits_shell_form() {
    let config = get_cas_hooks_config(&HookConfig::default());
    let staleness_cmd = nth_hook_command(&config, "SessionStart", 1);
    assert_eq!(
        staleness_cmd,
        Some("cas factory check-staleness"),
        "check-staleness entry under SessionStart must be shell-form command"
    );
    // And no exec-form args leak on the staleness entry.
    let staleness_args = nth_hook_args(&config, "SessionStart", 1);
    assert!(
        staleness_args.is_none(),
        "check-staleness entry must not carry an exec-form args array"
    );
}

/// AC#4 — round-trip: the freshly-emitted shell-form config is detected by
/// has_cas_hook_entries, fully stripped by strip_cas_hooks (hooks key gone),
/// and a second strip is a no-op (idempotent re-`cas init`).
#[test]
fn emitted_config_round_trips_through_detect_and_strip() {
    let mut config = get_cas_hooks_config(&HookConfig::default());

    assert!(
        has_cas_hook_entries(&config),
        "freshly-emitted shell-form config must be detected as CAS hooks"
    );

    let stripped = strip_cas_hooks(&mut config);
    assert!(stripped, "strip_cas_hooks must report removal of CAS hooks");
    assert!(
        config.get("hooks").is_none(),
        "hooks key must be gone after stripping an all-CAS config"
    );

    // Idempotent: detection now false, second strip is a no-op.
    assert!(
        !has_cas_hook_entries(&config),
        "no CAS hooks should remain after stripping"
    );
    assert!(
        !strip_cas_hooks(&mut config),
        "a second strip must be a no-op (idempotent re-init)"
    );
}

/// AC#6 — regression: both legacy on-disk forms must still be detected and
/// removed on the next `cas init`. Covers the malformed cas-9a60 exec-form
/// (`args[0]=="cas"`, no command) and the cas-c17b shell-form.
#[test]
fn legacy_forms_still_detected_and_stripped() {
    // Malformed exec-form: shape CAS actually wrote on cas-9a60 / cas-7ecd era,
    // including matcher, timeout, and async fields. No top-level command.
    let mut exec_form = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Read|Write|Edit|Glob|Grep|Bash|NotebookEdit",
                "hooks": [{
                    "type": "command",
                    "args": ["cas", "hook", "PreToolUse"],
                    "timeout": 2000
                }]
            }],
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "args": ["cas", "factory", "check-staleness"],
                    "timeout": 5000
                }]
            }]
        }
    });
    assert!(
        has_cas_hook_entries(&exec_form),
        "malformed exec-form settings from pre-cas-c17b CAS must still be detected"
    );
    assert!(
        strip_cas_hooks(&mut exec_form),
        "malformed exec-form CAS hooks must be stripped on re-init"
    );
    assert!(
        exec_form.get("hooks").is_none(),
        "all exec-form CAS hooks should be removed, leaving no hooks key"
    );

    // Shell-form: shape generated by CAS after cas-c17b.
    let mut shell_form = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        }
    });
    assert!(
        has_cas_hook_entries(&shell_form),
        "shell-form settings must also be detected as CAS hooks"
    );
    assert!(
        strip_cas_hooks(&mut shell_form),
        "shell-form CAS hooks must be stripped on re-init"
    );
    assert!(
        shell_form.get("hooks").is_none(),
        "all shell-form CAS hooks should be removed, leaving no hooks key"
    );
}

// ---------------------------------------------------------------------------
// cas-fda1 — sealed verifier handoff hook installation is an ATOMIC
// configuration invariant.
//
// PreToolUse is the only route that can issue a sealed task-verifier handoff.
// SubagentStart binds it; PostToolUseFailure and PermissionDenied are the two
// terminal cleanup routes for a still-unbound handoff. Before this fix those
// three hung off `stop.enabled`, `post_tool_use.enabled`, and
// `permission_request.enabled` — flags independent of `pre_tool_use.enabled` —
// so valid configurations could issue a handoff that could never bind or be
// cleaned up, wedging every later verifier spawn until expiry.
// ---------------------------------------------------------------------------

/// Build a HookConfig with the four independent flags that used to gate the
/// sealed-handoff lifecycle set explicitly. Everything else stays at default.
fn matrix_config(
    pre_tool_use: bool,
    stop: bool,
    post_tool_use: bool,
    permission_request: bool,
) -> HookConfig {
    let mut config = HookConfig::default();
    config.pre_tool_use.enabled = pre_tool_use;
    config.stop.enabled = stop;
    config.post_tool_use.enabled = post_tool_use;
    config.permission_request.enabled = permission_request;
    config
}

fn event_installed(config: &serde_json::Value, event: &str) -> bool {
    config
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .is_some()
}

/// Every one of the 2^4 independent flag combinations must satisfy the
/// invariant: a configuration that installs handoff issuance also installs
/// synchronous SubagentStart binding AND both cleanup routes — or installs no
/// issuance at all (fail-closed, since no handoff can then exist).
#[test]
fn sealed_handoff_lifecycle_is_atomic_across_full_flag_matrix() {
    let mut checked = 0usize;
    for pre_tool_use in [false, true] {
        for stop in [false, true] {
            for post_tool_use in [false, true] {
                for permission_request in [false, true] {
                    let hook_config =
                        matrix_config(pre_tool_use, stop, post_tool_use, permission_request);
                    let config = get_cas_hooks_config(&hook_config);
                    let combo = format!(
                        "pre_tool_use={pre_tool_use} stop={stop} \
                         post_tool_use={post_tool_use} permission_request={permission_request}"
                    );

                    let issuance = event_installed(&config, "PreToolUse");
                    assert_eq!(
                        issuance, pre_tool_use,
                        "[{combo}] PreToolUse issuance must track pre_tool_use.enabled"
                    );

                    for terminal in ["SubagentStart", "PostToolUseFailure", "PermissionDenied"] {
                        assert_eq!(
                            event_installed(&config, terminal),
                            issuance,
                            "[{combo}] {terminal} must be installed exactly when handoff \
                             issuance is installed — an unbindable/uncleanable handoff \
                             wedges verifier spawn until expiry"
                        );
                    }

                    if issuance {
                        // Binding must stay synchronous: verification must not
                        // race the bind of the child's authority.
                        assert!(
                            config["hooks"]["SubagentStart"][0]["hooks"][0]
                                .get("async")
                                .is_none(),
                            "[{combo}] SubagentStart binding must remain synchronous"
                        );
                        // Both cleanup routes must target the Agent spawn tools.
                        for cleanup in ["PostToolUseFailure", "PermissionDenied"] {
                            assert_eq!(
                                config["hooks"][cleanup][0]["matcher"].as_str(),
                                Some("Task|Agent"),
                                "[{combo}] {cleanup} must match the Agent spawn tools"
                            );
                        }
                        // SubagentStart binds only the verifier child.
                        assert_eq!(
                            config["hooks"]["SubagentStart"][0]["matcher"].as_str(),
                            Some("task-verifier"),
                            "[{combo}] SubagentStart must bind only task-verifier children"
                        );
                    }

                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 16, "all 2^4 flag combinations must be covered");
}

/// The precise pre-fix wedge: issuance on, `stop` off. This configuration used
/// to emit PreToolUse without SubagentStart, minting handoffs that could never
/// bind.
#[test]
fn issuance_without_stop_still_installs_binding_and_cleanup() {
    let config = get_cas_hooks_config(&matrix_config(true, false, false, false));

    assert!(event_installed(&config, "PreToolUse"));
    assert!(
        event_installed(&config, "SubagentStart"),
        "stop.enabled=false must no longer strip sealed-handoff binding"
    );
    assert!(
        event_installed(&config, "PostToolUseFailure"),
        "post_tool_use.enabled=false must no longer strip failed-spawn cleanup"
    );
    assert!(
        event_installed(&config, "PermissionDenied"),
        "permission_request.enabled=false must no longer strip denied-spawn cleanup"
    );

    // The co-gated group must not drag in the unrelated hooks owned by those
    // other flags.
    assert!(!event_installed(&config, "Stop"));
    assert!(!event_installed(&config, "SubagentStop"));
    assert!(!event_installed(&config, "PostToolUse"));
    assert!(!event_installed(&config, "PermissionRequest"));
}

/// With issuance disabled the whole lifecycle group is absent — fail-closed,
/// not half-installed.
#[test]
fn disabled_issuance_installs_no_handoff_lifecycle_hooks() {
    let config = get_cas_hooks_config(&matrix_config(false, true, true, true));

    for event in [
        "PreToolUse",
        "SubagentStart",
        "PostToolUseFailure",
        "PermissionDenied",
    ] {
        assert!(
            !event_installed(&config, event),
            "{event} must be absent when no handoff can be issued"
        );
    }

    // Hooks genuinely owned by the still-enabled flags remain installed.
    assert!(event_installed(&config, "Stop"));
    assert!(event_installed(&config, "SubagentStop"));
    assert!(event_installed(&config, "PostToolUse"));
    assert!(event_installed(&config, "PermissionRequest"));
}

/// Non-lifecycle hooks must keep tracking their own independent flags — the
/// co-gating must not have coupled anything beyond the handoff lifecycle.
#[test]
fn unrelated_hooks_still_track_their_own_flags() {
    for stop in [false, true] {
        for post_tool_use in [false, true] {
            for permission_request in [false, true] {
                let config =
                    get_cas_hooks_config(&matrix_config(true, stop, post_tool_use, permission_request));
                let combo =
                    format!("stop={stop} post_tool_use={post_tool_use} permission_request={permission_request}");

                assert_eq!(event_installed(&config, "Stop"), stop, "[{combo}] Stop");
                assert_eq!(
                    event_installed(&config, "SubagentStop"),
                    stop,
                    "[{combo}] SubagentStop"
                );
                assert_eq!(
                    event_installed(&config, "PostToolUse"),
                    post_tool_use,
                    "[{combo}] PostToolUse"
                );
                assert_eq!(
                    event_installed(&config, "PermissionRequest"),
                    permission_request,
                    "[{combo}] PermissionRequest"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// cas-5b96: multi-config-dir awareness
//
// Claude Code reads `$CLAUDE_CONFIG_DIR/settings.json` when the variable is set
// and `~/.claude/settings.json` otherwise. Treating hooks in `~/.claude` as
// covering *every* session silently stripped project hooks and left alt-dir
// sessions with zero CAS hooks.
// ---------------------------------------------------------------------------

use crate::cli::hook::config_gen::{
    all_config_dirs_have_cas_hooks, config_dir_has_cas_hooks, config_dirs_missing_cas_hooks,
    known_claude_config_dirs_from,
};
use crate::cli::hook::configure_claude_hooks_with_config_dirs;
use std::path::PathBuf;

/// Write a settings.json containing CAS hooks into `config_dir`.
fn write_global_hooks(config_dir: &std::path::Path) {
    std::fs::create_dir_all(config_dir).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "cas hook SessionStart"}]
            }]
        }
    });
    std::fs::write(
        config_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
}

fn project_has_cas_hooks(project_root: &std::path::Path) -> bool {
    let content =
        std::fs::read_to_string(project_root.join(".claude/settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
    has_cas_hook_entries(&settings)
}

#[test]
fn known_config_dirs_defaults_to_home_claude() {
    let home = PathBuf::from("/fake/home");
    let dirs = known_claude_config_dirs_from(Some(&home), None);
    assert_eq!(dirs, vec![home.join(".claude")]);
}

#[test]
fn known_config_dirs_adds_env_dir_and_expands_tilde() {
    let home = PathBuf::from("/fake/home");

    let dirs = known_claude_config_dirs_from(Some(&home), Some("~/.claude-alt"));
    assert_eq!(
        dirs,
        vec![home.join(".claude"), home.join(".claude-alt")],
        "CLAUDE_CONFIG_DIR must be expanded against home and added"
    );

    let absolute = known_claude_config_dirs_from(Some(&home), Some("/elsewhere/claude"));
    assert_eq!(
        absolute,
        vec![home.join(".claude"), PathBuf::from("/elsewhere/claude")]
    );
}

#[test]
fn known_config_dirs_dedupes_and_ignores_blank_env() {
    let home = PathBuf::from("/fake/home");

    // Env pointing at the default dir must not duplicate it.
    let dirs = known_claude_config_dirs_from(Some(&home), Some("~/.claude"));
    assert_eq!(dirs, vec![home.join(".claude")]);

    // Empty / whitespace env is treated as unset.
    assert_eq!(
        known_claude_config_dirs_from(Some(&home), Some("   ")),
        vec![home.join(".claude")]
    );
}

#[test]
fn coverage_requires_every_config_dir() {
    let home = isolated_home();
    let default_dir = home.path().join(".claude");
    let alt_dir = home.path().join(".claude-alt");
    let dirs = vec![default_dir.clone(), alt_dir.clone()];

    // Neither populated → not covered.
    assert!(!all_config_dirs_have_cas_hooks(&dirs));

    // Default dir only → the alt-dir sessions are still uncovered.
    write_global_hooks(&default_dir);
    assert!(config_dir_has_cas_hooks(&default_dir));
    assert!(!all_config_dirs_have_cas_hooks(&dirs));
    assert_eq!(config_dirs_missing_cas_hooks(&dirs), vec![alt_dir.clone()]);

    // Both populated → covered, and nothing is missing.
    write_global_hooks(&alt_dir);
    assert!(all_config_dirs_have_cas_hooks(&dirs));
    assert!(config_dirs_missing_cas_hooks(&dirs).is_empty());

    // An empty dir list can never prove coverage.
    assert!(!all_config_dirs_have_cas_hooks(&[]));
}

/// AC1: hooks only in ~/.claude while sessions run under CLAUDE_CONFIG_DIR=alt
/// must NOT leave the project hookless.
#[test]
fn configure_keeps_project_hooks_when_alt_config_dir_lacks_them() {
    let home = isolated_home();
    let default_dir = home.path().join(".claude");
    let alt_dir = home.path().join(".claude-alt");
    write_global_hooks(&default_dir);
    std::fs::create_dir_all(&alt_dir).unwrap();

    let project = TempDir::new().unwrap();
    configure_claude_hooks_with_config_dirs(project.path(), false, &[default_dir, alt_dir])
        .unwrap();

    assert!(
        project_has_cas_hooks(project.path()),
        "project hooks must be kept while a known config dir has no global hooks"
    );
}

/// Regression for the reported failure: an existing project settings file with
/// CAS hooks must not be stripped by a re-run (`cas update`) when the alt config
/// dir is hookless.
#[test]
fn configure_does_not_strip_existing_project_hooks_for_alt_config_dir() {
    let home = isolated_home();
    let default_dir = home.path().join(".claude");
    let alt_dir = home.path().join(".claude-alt");
    write_global_hooks(&default_dir);

    let project = TempDir::new().unwrap();
    // First run with no global hooks anywhere writes project hooks.
    configure_claude_hooks_with_config_dirs(project.path(), false, &[alt_dir.clone()]).unwrap();
    assert!(project_has_cas_hooks(project.path()));

    // Second run (cas update) with hooks in ~/.claude only must keep them.
    configure_claude_hooks_with_config_dirs(project.path(), false, &[default_dir, alt_dir])
        .unwrap();
    assert!(
        project_has_cas_hooks(project.path()),
        "cas update must not strip project hooks that alt-dir sessions depend on"
    );
}

/// AC3/AC4: when every known config dir has global hooks, dedup still strips the
/// project-level copies (single-config-dir behavior unchanged).
#[test]
fn configure_strips_project_hooks_when_all_config_dirs_covered() {
    let home = isolated_home();
    let default_dir = home.path().join(".claude");
    let alt_dir = home.path().join(".claude-alt");

    let project = TempDir::new().unwrap();
    configure_claude_hooks_with_config_dirs(project.path(), false, &[default_dir.clone()]).unwrap();
    assert!(project_has_cas_hooks(project.path()));

    write_global_hooks(&default_dir);
    write_global_hooks(&alt_dir);
    configure_claude_hooks_with_config_dirs(project.path(), false, &[default_dir.clone(), alt_dir])
        .unwrap();
    assert!(
        !project_has_cas_hooks(project.path()),
        "with every config dir covered, project hooks are duplicates and must be stripped"
    );

    // Single config dir populated → same strip behavior as before cas-5b96.
    let project2 = TempDir::new().unwrap();
    configure_claude_hooks_with_config_dirs(project2.path(), false, &[default_dir]).unwrap();
    assert!(!project_has_cas_hooks(project2.path()));
}
