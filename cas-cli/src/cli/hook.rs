//! Hook command - handle Claude Code hook events
//!
//! Reads JSON from stdin, processes the hook event, and outputs JSON to stdout.

use clap::{Parser, Subcommand};
use std::io::{self, Read};
use std::path::Path;

use crate::config::Config;
use crate::hooks::{HookInput, handle_hook};
use crate::store;
use crate::ui::components::{Formatter, Header, KeyValue, Renderable, StatusLine};
use crate::ui::theme::ActiveTheme;

use crate::cli::Cli;
use crate::cli::hook::config_gen::{get_cas_hooks_config, has_cas_hook_entries};

mod config_gen;
pub use crate::cli::hook::config_gen::{
    configure_codex_mcp_server, configure_mcp_server, global_has_cas_hooks, strip_cas_hooks,
};

/// Arguments for the hook command
#[derive(Parser)]
pub struct HookArgs {
    /// Hook subcommand or event name
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Subcommand)]
#[command(rename_all = "verbatim")]
pub enum HookCommand {
    /// Configure Claude Code hooks in .claude/settings.json
    #[command(name = "configure")]
    Configure {
        /// Force overwrite existing hooks configuration
        #[arg(short, long)]
        force: bool,
    },
    /// Show current hooks configuration status
    #[command(name = "status")]
    Status,
    /// Handle SessionStart hook event
    SessionStart,
    /// Handle SessionEnd hook event
    SessionEnd,
    /// Handle Stop hook event
    Stop,
    /// Handle SubagentStart hook event (verification jail unjailing)
    SubagentStart,
    /// Handle SubagentStop hook event
    SubagentStop,
    /// Handle PostToolUse hook event
    PostToolUse,
    /// Handle PostToolUseFailure hook event (sealed verifier handoff cleanup)
    PostToolUseFailure,
    /// Handle PreToolUse hook event (auto-approval)
    PreToolUse,
    /// Handle UserPromptSubmit hook event
    UserPromptSubmit,
    /// Handle PermissionRequest hook event (smart auto-approve)
    PermissionRequest,
    /// Handle PermissionDenied hook event (sealed verifier handoff cleanup)
    PermissionDenied,
    /// Handle Notification hook event (external alerts)
    Notification,
    /// Handle PreCompact hook event (context preservation)
    PreCompact,
    /// Handle MessageDisplay hook event (Ink-crash guard + assistant-text redaction, opt-in)
    MessageDisplay,
    /// Remove duplicate CAS hooks from project-level .claude/settings.json files
    ///
    /// When CAS hooks are configured globally in ~/.claude/settings.json,
    /// project-level hooks cause duplicates (each hook runs twice per tool call).
    /// This command strips CAS hook entries from project settings while preserving
    /// non-hook settings like permissions and statusLine.
    #[command(name = "cleanup")]
    Cleanup {
        /// Dry run - show what would be changed without modifying files
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
}

/// Execute the hook command
pub fn execute(args: &HookArgs, cli: &Cli) -> anyhow::Result<()> {
    match &args.command {
        HookCommand::Configure { force } => execute_configure(*force, cli),
        HookCommand::Cleanup { dry_run } => execute_cleanup(*dry_run, cli),
        HookCommand::Status => execute_status(cli),
        HookCommand::SessionStart => execute_event("SessionStart", cli),
        HookCommand::SessionEnd => execute_event("SessionEnd", cli),
        HookCommand::Stop => execute_event("Stop", cli),
        HookCommand::SubagentStart => execute_event("SubagentStart", cli),
        HookCommand::SubagentStop => execute_event("SubagentStop", cli),
        HookCommand::PostToolUse => execute_event("PostToolUse", cli),
        HookCommand::PostToolUseFailure => execute_event("PostToolUseFailure", cli),
        HookCommand::PreToolUse => execute_event("PreToolUse", cli),
        HookCommand::UserPromptSubmit => execute_event("UserPromptSubmit", cli),
        HookCommand::PermissionRequest => execute_event("PermissionRequest", cli),
        HookCommand::PermissionDenied => execute_event("PermissionDenied", cli),
        HookCommand::Notification => execute_event("Notification", cli),
        HookCommand::PreCompact => execute_event("PreCompact", cli),
        HookCommand::MessageDisplay => execute_event("MessageDisplay", cli),
    }
}

/// Handle a hook event (reads JSON from stdin)
fn execute_event(event: &str, cli: &Cli) -> anyhow::Result<()> {
    // Initialize logging for hooks (best-effort, don't fail if it doesn't work)
    init_hook_logging(cli.verbose);

    // Read JSON from stdin
    let mut input_json = String::new();
    io::stdin().read_to_string(&mut input_json)?;

    // Parse the hook input
    let mut input: HookInput = serde_json::from_str(&input_json)
        .map_err(|e| anyhow::anyhow!("Failed to parse hook input: {e}"))?;

    // Harness-side population: snapshot CAS_AGENT_ROLE into the HookInput so
    // downstream handlers read the role explicitly from the request rather
    // than re-reading process-global env at call time. Makes the contract
    // robust against inline hook dispatch from long-lived MCP processes
    // where other tools mutate env concurrently.
    if input.agent_role.is_none() {
        input.agent_role = std::env::var("CAS_AGENT_ROLE").ok();
    }

    // Handle the hook event
    let output = handle_hook(event, input)?;

    // Output JSON to stdout
    println!("{}", serde_json::to_string(&output)?);

    Ok(())
}

/// Initialize logging for hook execution
fn init_hook_logging(verbose: bool) {
    // Try to find CAS root and load config
    let cas_root = store::find_cas_root().ok();
    let logging_config = cas_root
        .as_ref()
        .and_then(|root| Config::load(root).ok())
        .and_then(|c| c.logging)
        .unwrap_or_default();

    // Initialize logging - ignore errors (may already be initialized)
    let _ = crate::logging::init(cas_root.as_deref(), verbose, &logging_config);
}

/// Strip duplicate CAS hooks from project-level .claude/settings.json files
fn execute_cleanup(dry_run: bool, cli: &Cli) -> anyhow::Result<()> {
    if !global_has_cas_hooks() {
        if cli.json {
            println!(r#"{{"status":"skipped","reason":"no_global_hooks"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::info("No CAS hooks found in global ~/.claude/settings.json").render(&mut fmt)?;
            fmt.newline()?;
            fmt.info("Nothing to clean up. Run 'cas hook configure' in a project first, or add hooks to ~/.claude/settings.json.")?;
        }
        return Ok(());
    }

    // Find all project-level .claude/settings.json files with CAS hooks
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let global_path = home.join(".claude").join("settings.json");

    let mut candidates = Vec::new();
    find_settings_files_with_cas_hooks(&home, &global_path, &mut candidates);

    if candidates.is_empty() {
        if cli.json {
            println!(r#"{{"status":"clean","files_checked":0}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::success("No duplicate CAS hooks found in project settings").render(&mut fmt)?;
        }
        return Ok(());
    }

    let mut cleaned = 0u32;
    let mut deleted = 0u32;
    let mut errors = 0u32;

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    if !cli.json {
        Header::h1(&format!(
            "CAS Hook Cleanup{}",
            if dry_run { " (dry run)" } else { "" }
        ))
        .render(&mut fmt)?;
        fmt.newline()?;
    }

    for path in &candidates {
        match cleanup_single_file(path, dry_run) {
            Ok(CleanupAction::Stripped) => {
                cleaned += 1;
                if !cli.json {
                    fmt.bullet(&format!("Stripped hooks: {}", path.display()))?;
                }
            }
            Ok(CleanupAction::Deleted) => {
                deleted += 1;
                if !cli.json {
                    fmt.bullet(&format!("Deleted (hooks-only file): {}", path.display()))?;
                }
            }
            Ok(CleanupAction::Unchanged) => {}
            Err(e) => {
                errors += 1;
                if !cli.json {
                    eprintln!("  Error processing {}: {e}", path.display());
                }
            }
        }
    }

    if cli.json {
        println!(
            r#"{{"status":"done","dry_run":{dry_run},"stripped":{cleaned},"deleted":{deleted},"errors":{errors}}}"#
        );
    } else {
        fmt.newline()?;
        StatusLine::success(&format!(
            "{}: {} stripped, {} deleted, {} errors",
            if dry_run { "Would process" } else { "Processed" },
            cleaned,
            deleted,
            errors
        ))
        .render(&mut fmt)?;
    }

    Ok(())
}

enum CleanupAction {
    Stripped,
    Deleted,
    Unchanged,
}

/// Clean up a single project-level settings file by stripping CAS hooks.
fn cleanup_single_file(path: &Path, dry_run: bool) -> anyhow::Result<CleanupAction> {
    let content = std::fs::read_to_string(path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    if !strip_cas_hooks(&mut settings) {
        return Ok(CleanupAction::Unchanged);
    }

    // Also strip CAS statusLine if present (global provides it)
    if let Some(obj) = settings.as_object_mut() {
        obj.remove("statusLine");
    }

    // Check if the file is now empty or only has empty objects
    let is_empty = settings
        .as_object()
        .map(|obj| {
            obj.is_empty()
                || obj.iter().all(|(_, v)| {
                    v.as_object().map(|o| o.is_empty()).unwrap_or(false)
                        || v.as_array().map(|a| a.is_empty()).unwrap_or(false)
                })
        })
        .unwrap_or(false);

    if dry_run {
        return Ok(if is_empty {
            CleanupAction::Deleted
        } else {
            CleanupAction::Stripped
        });
    }

    if is_empty {
        std::fs::remove_file(path)?;
        // Also remove empty .claude directory if it's now empty
        if let Some(parent) = path.parent() {
            if parent.file_name().is_some_and(|n| n == ".claude") {
                if let Ok(mut entries) = std::fs::read_dir(parent) {
                    if entries.next().is_none() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
        Ok(CleanupAction::Deleted)
    } else {
        let output = serde_json::to_string_pretty(&settings)?;
        std::fs::write(path, output)?;
        Ok(CleanupAction::Stripped)
    }
}

/// Recursively find .claude/settings.json files containing CAS hooks.
fn find_settings_files_with_cas_hooks(
    dir: &Path,
    global_path: &Path,
    results: &mut Vec<std::path::PathBuf>,
) {
    // Check this directory for .claude/settings.json
    let settings_path = dir.join(".claude").join("settings.json");
    if settings_path.exists() && settings_path != *global_path {
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) {
                if has_cas_hook_entries(&settings) {
                    results.push(settings_path);
                }
            }
        }
    }

    // Recurse into subdirectories, but skip heavy/irrelevant paths
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Skip directories that won't have project settings
        if name.starts_with('.')
            && name != ".cas" // .cas/worktrees/ may have settings
        {
            continue;
        }
        if matches!(
            name.as_ref(),
            "node_modules" | "target" | "dist" | ".git" | "__pycache__" | "venv"
        ) {
            continue;
        }

        find_settings_files_with_cas_hooks(&path, global_path, results);
    }
}

/// Configure Claude Code hooks
fn execute_configure(force: bool, cli: &Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let hooks_in_global = global_has_cas_hooks();

    match configure_claude_hooks(&cwd, force) {
        Ok(created) => {
            if cli.json {
                println!(
                    r#"{{"status":"configured","created":{created},"hooks_skipped":{hooks_in_global}}}"#
                );
            } else {
                let theme = ActiveTheme::default();
                let mut stdout = io::stdout();
                let mut fmt = Formatter::stdout(&mut stdout, theme);

                if hooks_in_global {
                    StatusLine::success(if created {
                        "Created .claude/settings.json (permissions only — hooks are global)"
                    } else {
                        "Updated .claude/settings.json (permissions only — hooks are global)"
                    })
                    .render(&mut fmt)?;
                    fmt.newline()?;
                    StatusLine::info(
                        "CAS hooks already in ~/.claude/settings.json — skipped to avoid duplicates.",
                    )
                    .render(&mut fmt)?;
                } else if created {
                    StatusLine::success("Created .claude/settings.json with CAS hooks")
                        .render(&mut fmt)?;
                    fmt.newline()?;
                    Header::h2("Hooks configured").render(&mut fmt)?;
                    fmt.bullet("SessionStart - Injects context at session start")?;
                    fmt.bullet("Stop - Generates session summary")?;
                    fmt.bullet("PostToolUse - Captures Write/Edit/Bash activity")?;
                    fmt.bullet("StatusLine - Shows CAS status in Claude Code footer")?;
                    fmt.newline()?;
                    Header::h2("Permissions configured (Claude Code 2.1.0+)").render(&mut fmt)?;
                    fmt.bullet("Bash(cas :*) - All CAS commands auto-allowed")?;
                    fmt.newline()?;
                    StatusLine::info("Restart your Claude Code session for hooks to take effect.")
                        .render(&mut fmt)?;
                } else {
                    StatusLine::success("Updated .claude/settings.json with CAS hooks")
                        .render(&mut fmt)?;
                    fmt.newline()?;
                    StatusLine::info("Restart your Claude Code session for hooks to take effect.")
                        .render(&mut fmt)?;
                }
            }
        }
        Err(e) => {
            if cli.json {
                println!(r#"{{"status":"error","message":"{e}"}}"#);
            } else {
                anyhow::bail!("Failed to configure hooks: {e}");
            }
        }
    }

    Ok(())
}

/// Show hooks configuration status
fn execute_status(cli: &Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let claude_dir = cwd.join(".claude");
    let settings_path = claude_dir.join("settings.json");

    if !settings_path.exists() {
        if cli.json {
            println!(r#"{{"configured":false,"reason":"no_settings_file"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::error("No .claude/settings.json found").render(&mut fmt)?;
            fmt.newline()?;
            fmt.info("Run 'cas hook configure' to set up Claude Code hooks.")?;
        }
        return Ok(());
    }

    // Read and parse settings
    let content = std::fs::read_to_string(&settings_path)?;
    let settings: serde_json::Value = serde_json::from_str(&content)?;

    let has_hooks = settings.get("hooks").is_some();

    let check_hook = |name: &str| -> bool {
        settings
            .pointer(&format!("/hooks/{name}"))
            .map(|v| v.is_array() && !v.as_array().unwrap().is_empty())
            .unwrap_or(false)
    };

    let has_session_start = check_hook("SessionStart");
    let has_session_end = check_hook("SessionEnd");
    let has_stop = check_hook("Stop");
    let has_subagent_stop = check_hook("SubagentStop");
    let has_post_tool = check_hook("PostToolUse");
    let has_pre_tool = check_hook("PreToolUse");
    let has_user_prompt = check_hook("UserPromptSubmit");
    let has_permission_request = check_hook("PermissionRequest");
    let has_notification = check_hook("Notification");
    let has_pre_compact = check_hook("PreCompact");

    if cli.json {
        println!(
            r#"{{"configured":{has_hooks},"session_start":{has_session_start},"session_end":{has_session_end},"stop":{has_stop},"subagent_stop":{has_subagent_stop},"post_tool_use":{has_post_tool},"pre_tool_use":{has_pre_tool},"user_prompt_submit":{has_user_prompt},"permission_request":{has_permission_request},"notification":{has_notification},"pre_compact":{has_pre_compact}}}"#
        );
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);

        Header::h1("Claude Code Hooks Status").render(&mut fmt)?;

        let status_str = |configured: bool| -> &'static str {
            if configured {
                "configured"
            } else {
                "not configured"
            }
        };

        KeyValue::new()
            .add("SessionStart", status_str(has_session_start))
            .add("SessionEnd", status_str(has_session_end))
            .add("Stop", status_str(has_stop))
            .add("SubagentStop", status_str(has_subagent_stop))
            .add("PostToolUse", status_str(has_post_tool))
            .add("PreToolUse", status_str(has_pre_tool))
            .add("UserPromptSubmit", status_str(has_user_prompt))
            .add("PermissionRequest", status_str(has_permission_request))
            .add("Notification", status_str(has_notification))
            .add("PreCompact", status_str(has_pre_compact))
            .render(&mut fmt)?;

        let all_configured = has_session_start
            && has_session_end
            && has_stop
            && has_subagent_stop
            && has_post_tool
            && has_pre_tool
            && has_user_prompt
            && has_permission_request
            && has_notification
            && has_pre_compact;
        if !all_configured {
            fmt.newline()?;
            fmt.info("Run 'cas hook configure' to set up missing hooks.")?;
        }
    }

    Ok(())
}

/// Configure Claude Code hooks in .claude/settings.json
///
/// This function creates or updates the settings.json file to include
/// CAS hooks for SessionStart, Stop, and PostToolUse events.
///
/// When global ~/.claude/settings.json already has CAS hooks configured,
/// this function only writes permissions and statusLine to the project
/// settings — no hooks — to avoid duplicate hook execution.
///
/// Returns Ok(true) if file was created, Ok(false) if updated.
/// Configure CAS hooks, injecting an explicit home directory for the global-settings
/// check. Pass `None` for production (uses `dirs::home_dir()`); pass `Some(path)` in
/// tests to avoid reading the real `~/.claude/settings.json` (cas-1888).
pub(crate) fn configure_claude_hooks_with_home(
    project_root: &Path,
    force: bool,
    home_dir: Option<&Path>,
) -> anyhow::Result<bool> {
    use config_gen::global_has_cas_hooks_in;

    let claude_dir = project_root.join(".claude");
    let settings_path = claude_dir.join("settings.json");
    let cas_dir = project_root.join(".cas");

    // Create .claude directory if needed
    if !claude_dir.exists() {
        std::fs::create_dir_all(&claude_dir)?;
    }

    // Load hook config from .cas/config.toml (or defaults)
    let hook_config = if cas_dir.exists() {
        Config::load(&cas_dir)
            .map(|c| c.hooks())
            .unwrap_or_default()
    } else {
        crate::config::HookConfig::default()
    };

    let cas_hooks = get_cas_hooks_config(&hook_config);

    // Check if global settings already have CAS hooks — if so, skip project-level
    // hooks to avoid duplicate execution. Only write permissions and statusLine.
    let skip_hooks = match home_dir {
        Some(h) => global_has_cas_hooks_in(h),
        None => global_has_cas_hooks(),
    };

    let created = if settings_path.exists() && !force {
        // Merge with existing settings
        let content = std::fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&content)?;

        if !settings.is_object() {
            anyhow::bail!("settings.json is not an object");
        }

        if skip_hooks {
            // Global hooks exist — strip any existing CAS hooks from project settings
            strip_cas_hooks(&mut settings);
        } else {
            // No global hooks — add hooks to project settings
            let hooks = settings
                .as_object_mut()
                .unwrap()
                .entry("hooks")
                .or_insert_with(|| serde_json::json!({}));

            let hooks_obj = hooks
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("hooks is not an object"))?;

            let cas_hooks_obj = cas_hooks.as_object().unwrap();
            for (key, value) in cas_hooks_obj.get("hooks").unwrap().as_object().unwrap() {
                hooks_obj.insert(key.clone(), value.clone());
            }
        }

        // Add statusLine configuration (overwrite if exists - CAS owns this)
        let settings_obj = settings.as_object_mut().unwrap();
        if let Some(status_line) = cas_hooks.get("statusLine") {
            settings_obj.insert("statusLine".to_string(), status_line.clone());
        }

        // Merge CAS Bash permissions (Claude Code 2.1.0+ wildcard patterns)
        if let Some(cas_permissions) = cas_hooks.get("permissions") {
            let permissions = settings_obj
                .entry("permissions")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(permissions_obj) = permissions.as_object_mut() {
                let allow = permissions_obj
                    .entry("allow")
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(allow_arr) = allow.as_array_mut() {
                    // Add CAS permissions if not already present
                    if let Some(cas_allow) = cas_permissions.get("allow").and_then(|a| a.as_array())
                    {
                        for pattern in cas_allow {
                            if !allow_arr.contains(pattern) {
                                allow_arr.push(pattern.clone());
                            }
                        }
                    }
                }
            }
        }

        // Add worktree directory to additionalDirectories if worktrees are enabled
        merge_worktree_permissions(project_root, settings_obj);

        // Write back
        let output = serde_json::to_string_pretty(&settings)?;
        std::fs::write(&settings_path, output)?;
        false
    } else {
        // Create new settings file
        let mut settings = if skip_hooks {
            // Global hooks exist — only write permissions and statusLine
            let mut obj = serde_json::Map::new();
            if let Some(perms) = cas_hooks.get("permissions") {
                obj.insert("permissions".to_string(), perms.clone());
            }
            if let Some(sl) = cas_hooks.get("statusLine") {
                obj.insert("statusLine".to_string(), sl.clone());
            }
            serde_json::Value::Object(obj)
        } else {
            cas_hooks.clone()
        };

        // Add worktree directory if worktrees are enabled
        if let Some(settings_obj) = settings.as_object_mut() {
            merge_worktree_permissions(project_root, settings_obj);
        }

        let output = serde_json::to_string_pretty(&settings)?;
        std::fs::write(&settings_path, output)?;
        true
    };

    Ok(created)
}

/// Public API: configure CAS Claude hooks for a project.
///
/// Delegates to [`configure_claude_hooks_with_home`] with `home_dir = None`
/// (uses `dirs::home_dir()` to locate global settings). Call sites in
/// production (`init`, `update`, `hook configure`) use this wrapper.
/// Tests use `configure_claude_hooks_with_home` directly to inject an isolated
/// home directory (cas-1888).
pub fn configure_claude_hooks(project_root: &Path, force: bool) -> anyhow::Result<bool> {
    configure_claude_hooks_with_home(project_root, force, None)
}

/// Merge worktree additionalDirectories into settings permissions if enabled.
fn merge_worktree_permissions(
    project_root: &Path,
    settings_obj: &mut serde_json::Map<String, serde_json::Value>,
) {
    let cas_root = project_root.join(".cas");
    if let Ok(config) = Config::load(&cas_root) {
        let worktrees_config = config.worktrees();
        if worktrees_config.enabled {
            let base = worktrees_config.base_path.replace(
                "{project}",
                project_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("project"),
            );

            let worktree_path = if base.starts_with('/') {
                base
            } else {
                project_root
                    .parent()
                    .unwrap_or(project_root)
                    .join(&base)
                    .to_string_lossy()
                    .to_string()
            };

            let permissions = settings_obj
                .entry("permissions")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(permissions_obj) = permissions.as_object_mut() {
                let additional_dirs = permissions_obj
                    .entry("additionalDirectories")
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(dirs_arr) = additional_dirs.as_array_mut() {
                    let path_value = serde_json::Value::String(worktree_path.clone());
                    if !dirs_arr.contains(&path_value) {
                        dirs_arr.push(path_value);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "hook_tests/tests.rs"]
mod tests;
