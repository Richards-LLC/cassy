use std::path::Path;

use toml::map::Map;

/// Check if a specific home directory's ~/.claude/settings.json has CAS hooks
/// configured.
///
/// Extracted for test-isolation: callers can pass a fake home TempDir to avoid
/// reading the real `~/.claude/settings.json` and eliminate the parallel-test
/// race documented in cas-1888.
pub(crate) fn global_has_cas_hooks_in(home_dir: &std::path::Path) -> bool {
    let global_settings_path = home_dir.join(".claude").join("settings.json");
    let Ok(content) = std::fs::read_to_string(&global_settings_path) else {
        return false;
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    has_cas_hook_entries(&settings)
}

/// Check if the global ~/.claude/settings.json already has CAS hooks configured.
///
/// Returns true if the global settings contain at least one hook entry whose
/// command starts with "cas hook". When this is true, project-level settings
/// should NOT add hooks (only permissions/statusLine) to avoid duplication.
///
/// Wraps [`global_has_cas_hooks_in`] with `dirs::home_dir()`. Tests should use
/// `global_has_cas_hooks_in` with a controlled TempDir to avoid reading real
/// user state (cas-1888).
pub fn global_has_cas_hooks() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    global_has_cas_hooks_in(&home)
}

/// Check if a settings JSON value contains any CAS hook entries.
///
/// Recognises both the shell-form (`"command": "cas hook ..."`) — emitted by
/// CAS since cas-c17b — and the exec-form (`"args": ["cas", "hook", ...]`)
/// emitted by the cas-7ecd era, so that detection is backwards-compatible
/// with settings.json files generated before cas-c17b.  Emitters were
/// reverted to shell-form because Claude Code 2.1.139's /doctor validator
/// rejects exec-form hooks before the agent loads
/// (upstream: anthropics/claude-code#58441).
pub fn has_cas_hook_entries(settings: &serde_json::Value) -> bool {
    let Some(hooks) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    for (_event, entries) in hooks {
        let Some(entries_arr) = entries.as_array() else {
            continue;
        };
        for entry in entries_arr {
            let Some(hook_list) = entry.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for hook in hook_list {
                // Legacy shell-string form
                if let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) {
                    if cmd.starts_with("cas hook ") {
                        return true;
                    }
                }
                // Exec form: ["cas", "hook", ...]
                if is_cas_hook_args(hook) {
                    return true;
                }
            }
        }
    }
    false
}

/// Returns true when a hook JSON object uses exec-form `"args"` whose first two
/// elements are `"cas"` and `"hook"`.
fn is_cas_hook_args(hook: &serde_json::Value) -> bool {
    let Some(args) = hook.get("args").and_then(|a| a.as_array()) else {
        return false;
    };
    args.first().and_then(|v| v.as_str()) == Some("cas")
        && args.get(1).and_then(|v| v.as_str()) == Some("hook")
}

/// Returns true when a hook JSON object is a CAS exec-form or shell-form factory
/// hook (`"args": ["cas", "factory", ...]` or `"command": "cas factory ..."`).
fn is_cas_factory_args(hook: &serde_json::Value) -> bool {
    let Some(args) = hook.get("args").and_then(|a| a.as_array()) else {
        return false;
    };
    args.first().and_then(|v| v.as_str()) == Some("cas")
        && args.get(1).and_then(|v| v.as_str()) == Some("factory")
}

/// Strip CAS hook entries from a settings JSON value.
///
/// Removes hook event keys where ALL entries are CAS hooks. If an event has
/// a mix of CAS and non-CAS hooks, only the CAS entries are removed.
/// Returns true if any hooks were removed.
pub fn strip_cas_hooks(settings: &mut serde_json::Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };

    let mut events_to_remove = Vec::new();
    let mut modified = false;

    for (event, entries) in hooks.iter_mut() {
        let Some(entries_arr) = entries.as_array_mut() else {
            continue;
        };

        let original_len = entries_arr.len();
        entries_arr.retain(|entry| {
            let Some(hook_list) = entry.get("hooks").and_then(|h| h.as_array()) else {
                return true; // keep non-standard entries
            };
            // Remove entry if ALL its hooks are CAS hooks (shell form OR exec form)
            let all_cas = hook_list.iter().all(|hook| {
                // Legacy shell-string form
                let cmd_match = hook
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|cmd| {
                        cmd.starts_with("cas hook ") || cmd.starts_with("cas factory ")
                    })
                    .unwrap_or(false);
                // Exec-form (Claude Code 2.1.139+)
                let args_match = is_cas_hook_args(hook) || is_cas_factory_args(hook);
                cmd_match || args_match
            });
            !all_cas
        });

        if entries_arr.len() != original_len {
            modified = true;
        }
        if entries_arr.is_empty() {
            events_to_remove.push(event.clone());
        }
    }

    for event in events_to_remove {
        hooks.remove(&event);
    }

    // Remove empty hooks object
    if hooks.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }

    modified
}

/// Get the CAS hooks configuration JSON
///
/// Emits shell-form `"command": "cas hook <Event>"` entries (and
/// `"command": "cas factory check-staleness"` for the staleness check),
/// converging on the same shape as
/// `ui/factory/daemon/runtime/teams.rs::factory_hooks_block` (cas-c17b).
///
/// The earlier exec-form attempt (cas-9a60) emitted
/// `{"type": "command", "args": ["cas", "hook", "<Event>"]}` with NO top-level
/// `command` string. That is malformed: Claude Code's `/doctor` validator
/// requires a string `command` for every `type: "command"` hook, so it
/// rejected all 12 entries and the harness silently disabled every CAS hook.
/// (The cas-9a60 docstring's claim that exec-form is accepted once
/// anthropics/claude-code#58441 closed was a red herring — valid exec-form is
/// `{"command": "cas", "args": [...]}`, and `command` is required regardless.)
/// Shell-form sidesteps this and keeps detection working without changes.
///
/// Both legacy on-disk forms remain detectable: `has_cas_hook_entries` /
/// `strip_cas_hooks` match shell-form (`command` starts with "cas hook " /
/// "cas factory ") AND the malformed exec-form (`args[0] == "cas"`), so users
/// upgrade cleanly from either form on the next `cas init`.
///
/// Note: Claude Code 2.1.0+ supports `once: true` for hooks that should only run once
/// per session, even if resumed. CAS hooks intentionally do NOT use `once: true` because:
/// - SessionStart should inject context on every session start/resume
/// - PostToolUse and Stop should run on every matching event
///
/// Users can manually add `"once": true` to specific hooks if desired.
pub(crate) fn get_cas_hooks_config(config: &crate::config::HookConfig) -> serde_json::Value {
    // Build hooks config, only including enabled hooks
    let mut hooks = serde_json::Map::new();

    if config.session_start.enabled {
        hooks.insert(
            "SessionStart".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook SessionStart",
                            "timeout": config.session_start.timeout
                        }
                    ]
                },
                {
                    // Factory worktree staleness check - warns workers if behind remote
                    // Silent when up-to-date, so safe to run for all agents
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas factory check-staleness",
                            "timeout": 5000
                        }
                    ]
                }
            ]),
        );
    }

    // SessionEnd always uses session_start timeout (no separate config needed)
    // async: true - pure cleanup, no context injection
    if config.session_start.enabled {
        hooks.insert(
            "SessionEnd".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook SessionEnd",
                            "timeout": config.session_start.timeout,
                            "async": true
                        }
                    ]
                }
            ]),
        );
    }

    if config.stop.enabled {
        hooks.insert(
            "Stop".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook Stop",
                            "timeout": config.stop.timeout
                        }
                    ]
                }
            ]),
        );
    }

    // SubagentStop uses stop timeout
    // async: true - marker file cleanup only
    if config.stop.enabled {
        hooks.insert(
            "SubagentStop".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook SubagentStop",
                            "timeout": config.stop.timeout / 2, // Subagent cleanup is quicker
                            "async": true
                        }
                    ]
                }
            ]),
        );
    }

    // async: true - observation recording, doesn't affect execution
    if config.post_tool_use.enabled {
        let matcher = config.post_tool_use.matcher.join("|");
        hooks.insert(
            "PostToolUse".to_string(),
            serde_json::json!([
                {
                    "matcher": matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PostToolUse",
                            "timeout": config.post_tool_use.timeout,
                            "async": true
                        }
                    ]
                }
            ]),
        );
    }

    if config.pre_tool_use.enabled {
        let matcher = config.pre_tool_use.matcher.join("|");
        hooks.insert(
            "PreToolUse".to_string(),
            serde_json::json!([
                {
                    "matcher": matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PreToolUse",
                            "timeout": config.pre_tool_use.timeout
                        }
                    ]
                }
            ]),
        );

        // Sealed verifier handoff lifecycle — ONE atomic installation unit
        // (cas-fda1). PreToolUse (immediately above) is the only route that can
        // issue a sealed task-verifier handoff, so every terminal route for that
        // handoff must be installed by the same condition:
        //
        //   * SubagentStart          — binds the sole unbound handoff to the
        //                              official distinct child agent_id.
        //   * PostToolUseFailure     — cancels the exact still-unbound handoff
        //                              when the Agent spawn fails.
        //   * PermissionDenied       — same cancellation for a denied spawn.
        //
        // These previously hung off `stop.enabled`, `post_tool_use.enabled`, and
        // `permission_request.enabled` respectively. Because those flags are
        // independent of `pre_tool_use.enabled`, valid configurations could issue
        // a handoff that could never bind or be cleaned up; `pre_tool.rs` then
        // denies every later spawn with "already awaiting SubagentStart" until
        // the handoff expires, wedging verification. Co-gating here makes the
        // invariant structural: issuance installed => binding + both cleanup
        // routes installed; issuance absent => the whole group is absent, which
        // is fail-closed because no handoff can exist to bind or leak.
        //
        // All three handlers are pure handoff-lifecycle: each returns an empty
        // output unless the event is a `task-verifier` Task/Agent spawn, so
        // co-gating adds no behavior outside this lifecycle.

        // Must remain synchronous so verification cannot race binding.
        hooks.insert(
            "SubagentStart".to_string(),
            serde_json::json!([
                {
                    "matcher": "task-verifier",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook SubagentStart",
                            "timeout": 2000
                        }
                    ]
                }
            ]),
        );
        hooks.insert(
            "PostToolUseFailure".to_string(),
            serde_json::json!([
                {
                    "matcher": "Task|Agent",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PostToolUseFailure",
                            "timeout": 2000,
                            "async": true
                        }
                    ]
                }
            ]),
        );
        hooks.insert(
            "PermissionDenied".to_string(),
            serde_json::json!([
                {
                    "matcher": "Task|Agent",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PermissionDenied",
                            "timeout": 2000,
                            "async": true
                        }
                    ]
                }
            ]),
        );
    }

    if config.user_prompt_submit.enabled {
        hooks.insert(
            "UserPromptSubmit".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook UserPromptSubmit",
                            "timeout": config.user_prompt_submit.timeout
                        }
                    ]
                }
            ]),
        );
    }

    if config.permission_request.enabled {
        hooks.insert(
            "PermissionRequest".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PermissionRequest",
                            "timeout": config.permission_request.timeout
                        }
                    ]
                }
            ]),
        );
    }

    // async: true - external notifications, already spawns threads for webhooks
    if config.notification.enabled {
        let matcher = config.notification.matcher.join("|");
        hooks.insert(
            "Notification".to_string(),
            serde_json::json!([
                {
                    "matcher": matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook Notification",
                            "timeout": config.notification.timeout,
                            "async": true
                        }
                    ]
                }
            ]),
        );
    }

    if config.pre_compact.enabled {
        hooks.insert(
            "PreCompact".to_string(),
            serde_json::json!([
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PreCompact",
                            "timeout": config.pre_compact.timeout
                        }
                    ]
                }
            ]),
        );
    }

    // MessageDisplay hook (CC 2.1.152+) — Ink-crash guard + assistant-text
    // redaction. Registered unconditionally (guard is opt-in via config flag
    // `[hooks] message_display_guard = true`; the handler itself is a
    // zero-cost passthrough when the flag is absent or false).
    //
    // Uses a short timeout (1 s) since the handler does no I/O when the
    // guard is off and only regex + string operations when it's on.
    // Not async: MessageDisplay may block rendering until we respond, so we
    // must not defer into the background.
    hooks.insert(
        "MessageDisplay".to_string(),
        serde_json::json!([
            {
                "hooks": [
                    {
                        "type": "command",
                        "command": "cas hook MessageDisplay",
                        "timeout": 1000
                    }
                ]
            }
        ]),
    );

    let mut allow_permissions = get_cas_bash_permissions();
    allow_permissions.extend(get_cas_mcp_permissions());

    serde_json::json!({
        "permissions": {
            "allow": allow_permissions
        },
        "hooks": hooks,
        "statusLine": {
            "type": "command",
            "command": "cas statusline"
        }
    })
}

/// Get suggested Bash permission patterns for CAS commands
///
/// Claude Code 2.1.0+ supports wildcard patterns like `Bash(cas :*)` to allow
/// all CAS CLI commands without individual prompts.
pub fn get_cas_bash_permissions() -> Vec<String> {
    vec![
        "Bash(cas :*)".to_string(),       // All CAS commands
        "Bash(cas task:*)".to_string(),   // Task operations
        "Bash(cas search:*)".to_string(), // Search operations
        "Bash(cas add:*)".to_string(),    // Memory operations
    ]
}

/// Get MCP tool permission patterns for CAS tools
///
/// Workers need these permissions to call mcp__cas__* tools without prompts.
pub fn get_cas_mcp_permissions() -> Vec<String> {
    vec![
        "mcp__cas__task".to_string(),
        "mcp__cas__coordination".to_string(),
        "mcp__cas__memory".to_string(),
        "mcp__cas__search".to_string(),
        "mcp__cas__rule".to_string(),
        "mcp__cas__skill".to_string(),
        "mcp__cas__spec".to_string(),
        "mcp__cas__verification".to_string(),
        "mcp__cas__system".to_string(),
        "mcp__cas__pattern".to_string(),
    ]
}

/// Configure CAS as an MCP server via .mcp.json
///
/// Creates or updates .mcp.json in the project root to register CAS.
/// This follows the Claude Code convention for project-level MCP configuration.
/// Returns Ok(true) if file was modified, Ok(false) if no changes needed.
pub fn configure_mcp_server(project_root: &Path) -> anyhow::Result<bool> {
    let mcp_json_path = project_root.join(".mcp.json");

    // Read existing content for comparison
    let existing_content = if mcp_json_path.exists() {
        std::fs::read_to_string(&mcp_json_path).ok()
    } else {
        None
    };

    // Read existing config or create new
    let mut config: serde_json::Value = existing_content
        .as_ref()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // Ensure mcpServers object exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }

    // Add or update CAS server config
    config["mcpServers"]["cas"] = serde_json::json!({
        "command": "cas",
        "args": ["serve"]
    });

    // Write back with pretty formatting
    let formatted = serde_json::to_string_pretty(&config)?;

    // Check if content actually changed
    if existing_content.as_ref() == Some(&formatted) {
        return Ok(false);
    }

    std::fs::write(&mcp_json_path, formatted)?;
    Ok(true)
}

/// Configure CAS for Codex via `.codex/config.toml` and `.codex/hooks.json`.
///
/// Registers the MCP server and installs a native Codex `PostToolUse` hook for
/// shell/unified-exec calls. Codex documents both shell commands and
/// `exec_command` under the canonical `Bash` matcher:
/// <https://developers.openai.com/codex/config-advanced#hooks>
///
/// Returns `Ok(true)` if either file was modified, `Ok(false)` if no changes
/// were needed.
pub fn configure_codex_mcp_server(project_root: &Path) -> anyhow::Result<bool> {
    let codex_dir = project_root.join(".codex");
    let config_path = codex_dir.join("config.toml");

    if !codex_dir.exists() {
        std::fs::create_dir_all(&codex_dir)?;
    }

    let existing_content = if config_path.exists() {
        Some(std::fs::read_to_string(&config_path)?)
    } else {
        None
    };

    let mut config: toml::Value = match existing_content.as_ref() {
        Some(content) => toml::from_str(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse .codex/config.toml: {e}"))?,
        None => toml::Value::Table(Map::new()),
    };

    let root = config
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml is not a table"))?;

    let mcp_servers = root
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(Map::new()));
    let mcp_servers = mcp_servers
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("mcp_servers is not a table"))?;

    let mut target_key = None;
    if mcp_servers.contains_key("cas") {
        target_key = Some("cas".to_string());
    } else {
        for (key, value) in mcp_servers.iter() {
            if let Some(entry) = value.as_table() {
                if entry.get("command") == Some(&toml::Value::String("cas".to_string())) {
                    target_key = Some(key.clone());
                    break;
                }
            }
        }
    }

    let key = target_key.unwrap_or_else(|| "cas".to_string());
    let mut changed = false;

    match mcp_servers.get_mut(&key) {
        Some(entry) => {
            let entry = entry
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("mcp_servers.{key} is not a table"))?;

            let desired_command = toml::Value::String("cas".to_string());
            if entry.get("command") != Some(&desired_command) {
                entry.insert("command".to_string(), desired_command);
                changed = true;
            }

            let desired_args = toml::Value::Array(vec![toml::Value::String("serve".to_string())]);
            if entry.get("args") != Some(&desired_args) {
                entry.insert("args".to_string(), desired_args);
                changed = true;
            }

            // Ensure Codex fallback session is enabled by default
            let env = entry
                .entry("env")
                .or_insert_with(|| toml::Value::Table(Map::new()));
            let env = env
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("mcp_servers.{key}.env is not a table"))?;
            if !env.contains_key("CAS_CODEX_FALLBACK_SESSION") {
                env.insert(
                    "CAS_CODEX_FALLBACK_SESSION".to_string(),
                    toml::Value::String("1".to_string()),
                );
                changed = true;
            }
        }
        None => {
            let mut entry = Map::new();
            entry.insert(
                "command".to_string(),
                toml::Value::String("cas".to_string()),
            );
            entry.insert(
                "args".to_string(),
                toml::Value::Array(vec![toml::Value::String("serve".to_string())]),
            );
            let mut env = Map::new();
            env.insert(
                "CAS_CODEX_FALLBACK_SESSION".to_string(),
                toml::Value::String("1".to_string()),
            );
            entry.insert("env".to_string(), toml::Value::Table(env));
            mcp_servers.insert(key, toml::Value::Table(entry));
            changed = true;
        }
    }

    let hooks_changed = configure_codex_post_tool_use_hook(&codex_dir)?;

    if changed {
        let formatted = toml::to_string_pretty(&config)?;
        if existing_content.as_ref() != Some(&formatted) {
            std::fs::write(&config_path, formatted)?;
        }
    }

    Ok(changed || hooks_changed)
}

/// Install the task-anchor hook using Codex's native `hooks.json` schema.
///
/// The timeout is three seconds (Codex timeout units are seconds, unlike the
/// millisecond values in Claude's settings). Existing non-CAS hook groups are
/// preserved, while an older CAS PostToolUse entry is converged to this exact
/// matcher and handler.
fn configure_codex_post_tool_use_hook(codex_dir: &Path) -> anyhow::Result<bool> {
    let hooks_path = codex_dir.join("hooks.json");
    let existing_content = if hooks_path.exists() {
        Some(std::fs::read_to_string(&hooks_path)?)
    } else {
        None
    };
    let mut config: serde_json::Value = match existing_content.as_ref() {
        Some(content) => serde_json::from_str(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse .codex/hooks.json: {e}"))?,
        None => serde_json::json!({}),
    };
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.json is not an object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.json `hooks` is not an object"))?;
    let post_tool_use = hooks
        .entry("PostToolUse")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.json `hooks.PostToolUse` is not an array"))?;

    post_tool_use.retain(|group| {
        !group
            .get("hooks")
            .and_then(|handlers| handlers.as_array())
            .is_some_and(|handlers| {
                handlers.iter().any(|handler| {
                    handler.get("command").and_then(|value| value.as_str())
                        == Some("cas hook PostToolUse")
                })
            })
    });
    post_tool_use.push(serde_json::json!({
        "matcher": "^Bash$",
        "hooks": [{
            "type": "command",
            "command": "cas hook PostToolUse",
            "timeout": 3
        }]
    }));

    let formatted = serde_json::to_string_pretty(&config)?;
    if existing_content.as_ref() == Some(&formatted) {
        return Ok(false);
    }
    std::fs::write(hooks_path, formatted)?;
    Ok(true)
}
