use crate::hooks::handlers::handlers_middle::tmpfs_guardrail::maybe_tmpfs_guardrail_warning;
use crate::hooks::handlers::*;
use crate::hooks::types::HookSpecificOutput;
use std::borrow::Cow;

pub fn handle_post_tool_use(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    handle_post_tool_use_with_guardrail(input, cas_root, |cas_root, input, stores| {
        maybe_tmpfs_guardrail_warning(cas_root, input, stores.config())
    })
}

fn handle_post_tool_use_with_guardrail(
    input: &HookInput,
    cas_root: Option<&Path>,
    guardrail: impl FnOnce(&Path, &HookInput, &mut ToolHookStores<'_>) -> Option<HookOutput>,
) -> Result<HookOutput, MemError> {
    let input = normalize_post_tool_input(input);
    let input = input.as_ref();
    let tool_name = match &input.tool_name {
        Some(name) => name.as_str(),
        None => return Ok(HookOutput::empty()),
    };

    // Check if CAS is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // If Agent/Task returned without a successful SubagentStart bind, remove
    // only its exact still-unbound sealed handoff. Bound/consumed audit rows
    // are immutable and this is therefore a no-op after a legitimate spawn.
    let _ = handle_verifier_spawn_cleanup(input, Some(cas_root));

    // Create shared store cache — Config and stores opened once, reused across checks
    let mut stores = ToolHookStores::new(cas_root);

    // === WORKER ACTIVITY TRACKING (for supervisor visibility) ===
    // Send activity events for significant tools to the daemon
    if let Some((event_type, description)) = detect_significant_activity(tool_name, input) {
        let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
            session_id: input.session_id.clone(),
            event_type,
            description,
            entity_id: extract_activity_entity_id(tool_name, input),
        };
        // Best-effort send - don't block hook on daemon availability
        let _ = crate::mcp::socket::send_event(cas_root, &event);
    }

    let guardrail_tool = should_run_tmpfs_guardrail(tool_name);

    // Read-like tools do not produce PostToolUse observations today. Return
    // before config/mount/state work so non-Write/Edit/Bash hot paths stay
    // free of guardrail I/O.
    if !guardrail_tool && !should_buffer_observation(input, tool_name) {
        return Ok(HookOutput::empty());
    }

    // === TMPFS/RAMFS LARGE-WRITE GUARDRAIL ===
    // Warning-only: never deny a tool. Gate before config and mount/state I/O
    // so unrelated PostToolUse calls do not pay the tmpfs guardrail cost.
    let tmpfs_guardrail_warning = if guardrail_tool {
        guardrail(cas_root, input, &mut stores)
    } else {
        None
    };

    // === RIPPLE CHECK (file-to-task/spec consistency reminder) ===
    // When a file linked to a task or spec is modified, remind agent to check consistency
    if tool_name == "Write" || tool_name == "Edit" {
        if let Some(reminder) = check_ripple_consistency(&mut stores, input) {
            return Ok(HookOutput::with_post_tool_context(
                merge_post_tool_contexts(tmpfs_guardrail_warning.as_ref(), reminder),
            ));
        }
    }

    // Check if this tool is worth capturing for observations
    if !CAPTURE_TOOLS.contains(&tool_name) {
        if let Some(output) = tmpfs_guardrail_warning {
            return Ok(output);
        }
        return Ok(HookOutput::empty());
    }

    // Initialize dev tracer for rich tool tracing
    let _ = DevTracer::init_global(cas_root);

    // Check config for capture settings (uses cached Config)
    let config = stores.config();
    if let Some(ref hooks_config) = config.hooks {
        if !hooks_config.capture_enabled {
            if let Some(output) = tmpfs_guardrail_warning {
                return Ok(output);
            }
            return Ok(HookOutput::empty());
        }

        // Check if this specific tool should be captured
        if !hooks_config.capture_tools.is_empty()
            && !hooks_config.capture_tools.iter().any(|t| t == tool_name)
        {
            if let Some(output) = tmpfs_guardrail_warning {
                return Ok(output);
            }
            return Ok(HookOutput::empty());
        }
    }

    // Record rich tool trace for learning loop detection (dev mode only)
    if config.dev.as_ref().map(|d| d.dev_mode).unwrap_or(false) {
        record_rich_tool_trace(input, tool_name);
    }

    // === ATTRIBUTION CAPTURE (must happen before smart filtering) ===
    // Attribution must be captured for ALL file changes, not just "significant" ones.
    // This enables git blame-style attribution for every line of AI-generated code.
    if tool_name == "Write" || tool_name == "Edit" {
        crate::hooks::handlers::handlers_events::capture_file_change_for_attribution(
            cas_root, input, tool_name,
        );
    }

    // Detect git commit and link file changes (Bash tool only)
    if tool_name == "Bash" {
        crate::hooks::handlers::handlers_events::detect_and_link_git_commit(cas_root, input);
        crate::hooks::handlers::handlers_events::detect_codemap_structural_changes(cas_root, input);

        // Project-overview pending-file update. Only fires on successful git commits
        // so detect_structural_changes (which always diffs HEAD~1..HEAD) sees a
        // fresh commit, not the same previous one on every Bash.
        //
        // Advisory flock on a sentinel file serializes concurrent hook processes
        // so the JSONL append in detect_structural_changes cannot interleave
        // bytes past PIPE_BUF — addresses the P1 append-race carry-forward from
        // cas-dcba review. Best-effort: if the lock fails (FS without lock
        // support, e.g. NTFS3 — see MEMORY.md), we fall through and accept the
        // pre-existing race rather than dropping the pending write entirely.
        if is_successful_git_commit(input) {
            if let Some(repo_root) = cas_root.parent() {
                with_pending_file_lock(cas_root, || {
                    if let Err(e) = crate::hooks::handlers::handlers_events::project_overview::detect_structural_changes(
                        repo_root,
                    ) {
                        eprintln!("cas: project-overview detect error: {e}");
                    }
                });
            }
        }
    }

    if let Some(output) = tmpfs_guardrail_warning {
        return Ok(output);
    }

    // === SMART FILTERING ===
    // Only capture significant observations to reduce noise
    let should_buffer = should_buffer_observation(input, tool_name);

    if !should_buffer {
        return Ok(HookOutput::empty());
    }

    // Format observation content (use configurable skip lists)
    let skip_config = config.hooks.as_ref().map(|h| &h.post_tool_use.skip);
    let content = format_observation(input, skip_config);

    if content.is_empty() {
        return Ok(HookOutput::empty());
    }

    // Extract common fields for both storage paths
    let file_path = input
        .tool_input
        .as_ref()
        .and_then(|ti| ti.get("file_path").and_then(|v| v.as_str()));

    // Track file for session-aware context boosting
    // This enables "streaming observations" - context adapts to files being worked on
    if let Some(fp) = file_path {
        track_session_file(cas_root, fp);
    }

    let exit_code = input
        .tool_response
        .as_ref()
        .and_then(|tr| tr.get("exitCode").and_then(|v| v.as_i64()))
        .map(|v| v as i32);

    let is_error = exit_code.map(|c| c != 0).unwrap_or(false)
        || input
            .tool_response
            .as_ref()
            .and_then(|tr| tr.get("stderr").and_then(|v| v.as_str()))
            .map(|s| !s.is_empty())
            .unwrap_or(false);

    // Buffer for session-end synthesis (only when DevTracer is available)
    // Raw observations without synthesis are noise — skip if no tracer
    if let Some(tracer) = DevTracer::get() {
        let _ = tracer.buffer_observation(tool_name, file_path, &content, exit_code, is_error);
    }

    // Silent success - don't clutter Claude's output
    Ok(HookOutput::empty())
}

/// Normalize harness-specific tool names before any shared PostToolUse logic.
///
/// Grok sends the real terminal tool name (`run_terminal_command`) even
/// though its hook matcher calls that tool `Bash`. Reuse the established Bash
/// path for Grok hooks only. Factory-spawned hooks take the cheap env fast
/// path; standalone Grok sessions are recognized by fields unique to Grok's
/// camelCase hook envelope. An identically named tool from Claude or Codex
/// without those markers retains its original meaning.
fn normalize_post_tool_input(input: &HookInput) -> Cow<'_, HookInput> {
    if input.tool_name.as_deref() == Some("run_terminal_command")
        && (crate::harness_policy::own_harness_from_env() == cas_mux::SupervisorCli::Grok
            || input.workspace_root.is_some()
            || input.tool_input_truncated.is_some())
    {
        let mut normalized = input.clone();
        normalized.tool_name = Some("Bash".to_string());
        Cow::Owned(normalized)
    } else {
        Cow::Borrowed(input)
    }
}

fn should_run_tmpfs_guardrail(tool_name: &str) -> bool {
    matches!(tool_name, "Write" | "Edit" | "Bash")
}

fn post_tool_context(output: &HookOutput) -> Option<&str> {
    match output.hook_specific_output.as_ref()? {
        HookSpecificOutput::PostToolUse { additional_context } => additional_context.as_deref(),
        _ => None,
    }
}

fn merge_post_tool_contexts(warning: Option<&HookOutput>, reminder: String) -> String {
    match warning.and_then(post_tool_context) {
        Some(warning) if !warning.trim().is_empty() => format!("{warning}\n{reminder}"),
        _ => reminder,
    }
}

/// Determine if an observation is significant enough to buffer
///
/// We only buffer observations that are likely to yield useful learnings:
/// - Errors (always capture - failures are learning opportunities)
/// - New file creation (architectural decisions)
/// - Large edits (significant changes)
/// - Commands with interesting output
pub fn should_buffer_observation(input: &HookInput, tool_name: &str) -> bool {
    match tool_name {
        "Bash" => {
            // Always capture errors
            let exit_code = input
                .tool_response
                .as_ref()
                .and_then(|tr| tr.get("exitCode").and_then(|v| v.as_i64()))
                .unwrap_or(0);

            if exit_code != 0 {
                return true;
            }

            // Capture build/test commands (even successful ones - patterns matter)
            if let Some(ti) = &input.tool_input {
                if let Some(cmd) = ti.get("command").and_then(|v| v.as_str()) {
                    let cmd_lower = cmd.to_lowercase();
                    let significant_commands = [
                        "cargo build",
                        "cargo test",
                        "cargo check",
                        "mix compile",
                        "mix test",
                        "mix credo",
                        "npm build",
                        "npm test",
                        "bun test",
                        "git commit",
                        "git push",
                    ];
                    if significant_commands.iter().any(|sc| cmd_lower.contains(sc)) {
                        return true;
                    }
                }
            }

            false
        }
        "Write" => {
            // New file creation is always significant
            true
        }
        "Edit" => {
            // Only capture large edits (significant changes)
            if let Some(ti) = &input.tool_input {
                let old_string = ti.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new_string = ti.get("new_string").and_then(|v| v.as_str()).unwrap_or("");

                let old_lines = old_string.lines().count();
                let new_lines = new_string.lines().count();
                let line_diff = (new_lines as i32 - old_lines as i32).abs();

                // Capture edits that add/remove 10+ lines or involve 50+ line changes
                line_diff >= 10 || old_lines + new_lines >= 50
            } else {
                false
            }
        }
        "Read" => {
            // Don't buffer reads - they're just information gathering
            false
        }
        _ => false,
    }
}

pub fn record_rich_tool_trace(input: &HookInput, tool_name: &str) {
    let tracer = match DevTracer::get() {
        Some(t) => t,
        None => return,
    };

    // Get last trace to compute sequence info
    let last_trace = tracer.get_last_tool_trace(&input.session_id).ok().flatten();

    let sequence_pos = last_trace.as_ref().map(|t| t.sequence_pos + 1).unwrap_or(0);
    let prev_tool = last_trace.as_ref().map(|t| t.tool_name.clone());
    let prev_failed = last_trace.as_ref().map(|t| !t.success).unwrap_or(false);
    let time_since_prev_ms = last_trace.as_ref().map(|t| {
        let now = chrono::Utc::now();
        (now - t.timestamp).num_milliseconds().max(0) as u64
    });

    let mut trace = ToolTrace::new(
        input.session_id.clone(),
        tool_name.to_string(),
        sequence_pos,
    );

    trace.prev_tool = prev_tool;
    trace.prev_failed = prev_failed;
    trace.time_since_prev_ms = time_since_prev_ms;

    // Extract tool-specific information
    match tool_name {
        "Edit" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str()) {
                    trace.file_path = Some(path.to_string());
                    trace.is_dependency = ToolTrace::is_dep_path(path);
                }
                // Capture edit content for semantic analysis
                if let Some(old) = tool_input.get("old_string").and_then(|v| v.as_str()) {
                    trace.old_content = Some(old.chars().take(1000).collect());
                    trace.old_content_hash = Some(ToolTrace::hash_content(old));
                }
                if let Some(new) = tool_input.get("new_string").and_then(|v| v.as_str()) {
                    trace.new_content = Some(new.chars().take(1000).collect());
                    trace.new_content_hash = Some(ToolTrace::hash_content(new));
                }
                // Compute line changes
                if let (Some(old), Some(new)) = (
                    tool_input.get("old_string").and_then(|v| v.as_str()),
                    tool_input.get("new_string").and_then(|v| v.as_str()),
                ) {
                    let old_lines = old.lines().count() as u32;
                    let new_lines = new.lines().count() as u32;
                    if new_lines > old_lines {
                        trace.lines_added = Some(new_lines - old_lines);
                    } else if old_lines > new_lines {
                        trace.lines_removed = Some(old_lines - new_lines);
                    }
                }
            }
            // Check tool response for success
            if let Some(ref response) = input.tool_response {
                trace.success = response.get("error").is_none();
                if let Some(err) = response.get("error").and_then(|v| v.as_str()) {
                    trace.error_snippet = Some(err.chars().take(500).collect());
                    trace.error_type = Some(ToolTrace::classify_error(err).to_string());
                }
            }
        }
        "Write" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str()) {
                    trace.file_path = Some(path.to_string());
                    trace.is_dependency = ToolTrace::is_dep_path(path);
                }
                // Capture written content
                if let Some(content) = tool_input.get("content").and_then(|v| v.as_str()) {
                    trace.new_content = Some(content.chars().take(1000).collect());
                    trace.new_content_hash = Some(ToolTrace::hash_content(content));
                    trace.lines_added = Some(content.lines().count() as u32);
                }
            }
            if let Some(ref response) = input.tool_response {
                trace.success = response.get("error").is_none();
                if let Some(err) = response.get("error").and_then(|v| v.as_str()) {
                    trace.error_snippet = Some(err.chars().take(500).collect());
                    trace.error_type = Some(ToolTrace::classify_error(err).to_string());
                }
            }
        }
        "Bash" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) {
                    trace.command = Some(cmd.chars().take(500).collect());
                    trace.command_type = Some(ToolTrace::classify_command(cmd).to_string());
                }
            }
            if let Some(ref response) = input.tool_response {
                // Try to get exit code from response
                if let Some(exit_code) = response.get("exit_code").and_then(|v| v.as_i64()) {
                    trace.exit_code = Some(exit_code as i32);
                    trace.success = exit_code == 0;
                }
                // Extract output/error snippets
                if let Some(output) = response.get("stdout").and_then(|v| v.as_str()) {
                    trace.output_snippet = Some(output.chars().take(500).collect());
                }
                if let Some(stderr) = response.get("stderr").and_then(|v| v.as_str()) {
                    if !stderr.is_empty() {
                        trace.error_snippet = Some(stderr.chars().take(500).collect());
                        trace.error_type = Some(ToolTrace::classify_error(stderr).to_string());
                    }
                }
                // Also check for error in result field
                if let Some(result) = response.get("result").and_then(|v| v.as_str()) {
                    if trace.output_snippet.is_none() && !result.is_empty() {
                        trace.output_snippet = Some(result.chars().take(500).collect());
                    }
                    // If still no error type, try to classify from result
                    if trace.error_type.is_none() && !trace.success {
                        trace.error_type = Some(ToolTrace::classify_error(result).to_string());
                    }
                }
            }
        }
        "Read" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str()) {
                    trace.file_path = Some(path.to_string());
                    trace.is_dependency = ToolTrace::is_dep_path(path);
                }
            }
        }
        "Grep" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(pattern) = tool_input.get("pattern").and_then(|v| v.as_str()) {
                    trace.search_pattern = Some(pattern.chars().take(200).collect());
                }
                if let Some(path) = tool_input.get("path").and_then(|v| v.as_str()) {
                    trace.file_path = Some(path.to_string());
                }
            }
            if let Some(ref response) = input.tool_response {
                // Try to count matches from response
                if let Some(matches) = response.get("matches").and_then(|v| v.as_array()) {
                    trace.search_results_count = Some(matches.len() as u32);
                }
                // Or count from files_with_matches
                if let Some(files) = response.get("files").and_then(|v| v.as_array()) {
                    trace.search_results_count = Some(files.len() as u32);
                }
            }
        }
        "Glob" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(pattern) = tool_input.get("pattern").and_then(|v| v.as_str()) {
                    trace.search_pattern = Some(pattern.chars().take(200).collect());
                }
                if let Some(path) = tool_input.get("path").and_then(|v| v.as_str()) {
                    trace.file_path = Some(path.to_string());
                }
            }
            if let Some(ref response) = input.tool_response {
                if let Some(files) = response.get("files").and_then(|v| v.as_array()) {
                    trace.search_results_count = Some(files.len() as u32);
                }
            }
        }
        "WebFetch" => {
            if let Some(ref tool_input) = input.tool_input {
                if let Some(url) = tool_input.get("url").and_then(|v| v.as_str()) {
                    trace.url = Some(url.chars().take(500).collect());
                }
            }
            if let Some(ref response) = input.tool_response {
                trace.success = response.get("error").is_none();
                if let Some(err) = response.get("error").and_then(|v| v.as_str()) {
                    trace.error_snippet = Some(err.chars().take(500).collect());
                    trace.error_type = Some(ToolTrace::classify_error(err).to_string());
                }
            }
        }
        "Task" => {
            // Task tool is used for sub-agents, capture the prompt/description
            if let Some(ref tool_input) = input.tool_input {
                if let Some(prompt) = tool_input.get("prompt").and_then(|v| v.as_str()) {
                    // Store prompt snippet as search_pattern (repurposed for context)
                    trace.search_pattern = Some(prompt.chars().take(200).collect());
                }
            }
        }
        _ => {}
    }

    // Record the trace
    let _ = tracer.record_tool_trace(&trace);
}

/// Format an observation from tool usage
pub fn format_observation(
    input: &HookInput,
    skip_config: Option<&crate::config::PostToolUseSkipConfig>,
) -> String {
    let tool_name = input.tool_name.as_deref().unwrap_or("unknown");

    match tool_name {
        "Write" | "Edit" => format_file_change(input),
        "Bash" => format_bash_command(input, skip_config),
        "Read" => format_file_read(input, skip_config),
        _ => String::new(),
    }
}

/// Format a file write/edit observation
pub fn format_file_change(input: &HookInput) -> String {
    let tool_input = match &input.tool_input {
        Some(v) => v,
        None => return String::new(),
    };

    let file_path = tool_input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let tool_name = input.tool_name.as_deref().unwrap_or("Write");

    // Don't capture the full content, just the fact that it was modified
    format!("{tool_name}: {file_path}")
}

/// Format a bash command observation
pub fn format_bash_command(
    input: &HookInput,
    skip_config: Option<&crate::config::PostToolUseSkipConfig>,
) -> String {
    let tool_input = match &input.tool_input {
        Some(v) => v,
        None => return String::new(),
    };

    let command = tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Use config skip lists or defaults
    let default_skip = crate::config::PostToolUseSkipConfig::default();
    let skip = skip_config.unwrap_or(&default_skip);

    // Default commands to always skip (hardcoded for safety)
    let always_skip = [
        "which", "type", "command", "whereis", "file", "wc", "sort", "uniq", "tr", "cut", "awk",
        "sed", "grep", "rg", "find", "fd", "ag", "true", "false", "test", "[",
    ];

    // CAS dev build prefixes (always skip, not configurable)
    let dev_prefixes = ["./target/release/cas ", "./target/debug/cas "];

    // Git readonly commands with extended set (always skip)
    let always_skip_git = [
        "git remote",
        "git tag",
        "git stash list",
        "git config --get",
        "git rev-parse",
        "git ls-files",
        "git describe",
    ];

    let first_word = command.split_whitespace().next().unwrap_or("");

    // Check configurable skip commands
    if skip.commands.iter().any(|c| c == first_word) {
        return String::new();
    }

    // Check always-skip commands
    if always_skip.contains(&first_word) {
        return String::new();
    }

    // Check configurable skip prefixes (cas commands)
    if skip.prefixes.iter().any(|p| command.starts_with(p)) {
        return String::new();
    }

    // Check dev build prefixes (always skip)
    if dev_prefixes.iter().any(|p| command.starts_with(p)) {
        return String::new();
    }

    // Check configurable read-only git commands
    if skip.git_readonly.iter().any(|gc| command.starts_with(gc)) {
        return String::new();
    }

    // Check always-skip git commands
    if always_skip_git.iter().any(|gc| command.starts_with(gc)) {
        return String::new();
    }

    // Truncate long commands
    let cmd = truncate_display(command, 200);

    format!("Bash: {cmd}")
}

/// Format a file read observation
pub fn format_file_read(
    input: &HookInput,
    skip_config: Option<&crate::config::PostToolUseSkipConfig>,
) -> String {
    let tool_input = match &input.tool_input {
        Some(v) => v,
        None => return String::new(),
    };

    let file_path = tool_input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Use config skip extensions or defaults
    let default_skip = crate::config::PostToolUseSkipConfig::default();
    let skip = skip_config.unwrap_or(&default_skip);

    // Only note important file reads, skip common ones (configurable)
    if skip
        .read_extensions
        .iter()
        .any(|ext| file_path.ends_with(ext))
    {
        return String::new();
    }

    format!("Read: {file_path}")
}

/// Check if a modified file is referenced by any active task or spec.
/// Returns a reminder string if matches are found, None otherwise.
/// Designed to be lightweight (< 50ms) — fails silently on any error.
fn check_ripple_consistency(stores: &mut ToolHookStores, input: &HookInput) -> Option<String> {
    let file_path = input.tool_input.as_ref()?.get("file_path")?.as_str()?;

    let path = std::path::Path::new(file_path);

    // Scope ripple-check to the current project only.
    //
    // Cross-project edits (editing a file outside the project root) produce
    // false positives because common filenames like CLAUDE.md, README.md, or
    // Cargo.toml match task bodies across unrelated projects.  If the edited
    // file is not under the current project's root, suppress the check silently.
    //
    // The project root is cas_root.parent() (e.g. ~/Petrastella/cas-src when
    // cas_root = ~/Petrastella/cas-src/.cas).  Only absolute paths are checked
    // here; relative paths are assumed to live inside the current project.
    if path.is_absolute() {
        let project_root = stores.cas_root.parent()?;
        if !is_file_within_project(path, project_root) {
            return None;
        }
    }

    // Compute relative path from cwd for matching against task descriptions
    let cwd = std::env::current_dir().ok()?;
    let relative = path
        .strip_prefix(&cwd)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(file_path);

    // Build a short path (parent_dir/filename) for partial matching
    let short_path = path.file_name().and_then(|f| {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|parent| format!("{}/{}", parent.to_string_lossy(), f.to_string_lossy()))
    });

    // Check tasks using cached store
    let mut matching_task_ids = Vec::new();

    if let Some(task_store) = stores.tasks().cloned() {
        for status in [TaskStatus::Open, TaskStatus::InProgress] {
            if let Ok(tasks) = task_store.list(Some(status)) {
                for task in &tasks {
                    if task_references_file(task, relative, short_path.as_deref()) {
                        matching_task_ids.push(task.id.clone());
                        if matching_task_ids.len() >= 5 {
                            break;
                        }
                    }
                }
            }
            if matching_task_ids.len() >= 5 {
                break;
            }
        }
    }

    // Also check active specs using cached store
    let mut matching_spec_ids = Vec::new();
    if let Some(spec_store) = stores.specs().cloned() {
        if let Ok(specs) = spec_store.list(None) {
            for spec in &specs {
                // Skip superseded/rejected specs
                if spec.status == crate::types::SpecStatus::Superseded
                    || spec.status == crate::types::SpecStatus::Rejected
                {
                    continue;
                }
                if spec_references_file(spec, relative, short_path.as_deref()) {
                    matching_spec_ids.push(spec.id.clone());
                    if matching_spec_ids.len() >= 3 {
                        break;
                    }
                }
            }
        }
    }

    if matching_task_ids.is_empty() && matching_spec_ids.is_empty() {
        return None;
    }

    // Build reminder with matched IDs
    let mut parts = Vec::new();
    if !matching_task_ids.is_empty() {
        let refs = matching_task_ids
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("task(s) {refs}"));
    }
    if !matching_spec_ids.is_empty() {
        let refs = matching_spec_ids
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("spec(s) {refs}"));
    }

    Some(format!(
        "<system-reminder>Ripple check: `{}` is referenced in {}. \
         Verify the parent epic/spec descriptions are still consistent with your changes.</system-reminder>",
        relative,
        parts.join(" and "),
    ))
}

/// Returns `true` if `file_path` is located inside `project_root`.
///
/// Uses `std::fs::canonicalize` to resolve symlinks before comparison so that
/// symlinked project roots don't produce false negatives (and to avoid
/// accidental infinite loops through circular symlinks). When canonicalization
/// fails — e.g. the file does not exist yet — falls back to a lexical
/// `starts_with` check on the original paths.
///
/// Exported for unit testing; not part of the public CAS API.
pub(crate) fn is_file_within_project(
    file_path: &std::path::Path,
    project_root: &std::path::Path,
) -> bool {
    let canon_root =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let canon_file = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    canon_file.starts_with(&canon_root)
}

/// Check if a task's text fields reference a given file path.
fn task_references_file(task: &Task, relative: &str, short_path: Option<&str>) -> bool {
    let haystack = format!(
        "{}\n{}\n{}",
        task.description, task.acceptance_criteria, task.design
    );

    if haystack.contains(relative) {
        return true;
    }

    // Fall back to short path (parent/filename) for less specific references,
    // but only if the short path is distinctive enough (> 5 chars)
    if let Some(short) = short_path {
        if short.len() > 5 && haystack.contains(short) {
            return true;
        }
    }

    false
}

/// Run `f` while holding an exclusive advisory flock on
/// `<cas_root>/project-overview-pending.lock`.
///
/// Prevents two concurrent PostToolUse hook processes from interleaving appends
/// to `project-overview-pending.json`. Unlock is RAII — even if `f` panics the
/// guard's Drop releases the lock on scope exit. Lock acquisition failure
/// (filesystems without advisory-lock support, e.g. NTFS3 per MEMORY.md) falls
/// through and runs the closure unprotected; preserving the write is more
/// important than strict serialization on those platforms.
fn with_pending_file_lock(cas_root: &Path, f: impl FnOnce()) {
    use fs2::FileExt;

    struct LockGuard(std::fs::File, bool);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            if self.1 {
                let _ = FileExt::unlock(&self.0);
            }
        }
    }

    let lock_path = cas_root.join("project-overview-pending.lock");
    let _guard = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok()
        .map(|file| {
            let locked = file.lock_exclusive().is_ok();
            LockGuard(file, locked)
        });
    f();
}

/// True when the PostToolUse input represents a successful `git commit` Bash
/// call that creates a new commit.
///
/// `--dry-run` is excluded (mirrors `attribution::is_git_commit_command`).
/// `--amend` is excluded because it rewrites HEAD in place — firing
/// `detect_structural_changes` on an amended commit would re-record the same
/// logical change set and inflate pending-file totals past threshold.
fn is_successful_git_commit(input: &HookInput) -> bool {
    let Some(tool_input) = input.tool_input.as_ref() else {
        return false;
    };
    let Some(command) = tool_input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let cmd_lower = command.to_lowercase();
    if !cmd_lower.contains("git commit")
        || cmd_lower.contains("--dry-run")
        || cmd_lower.contains("--amend")
    {
        return false;
    }
    let exit_code = input
        .tool_response
        .as_ref()
        .and_then(|r| r.get("exitCode").and_then(|v| v.as_i64()))
        .unwrap_or(1);
    exit_code == 0
}

/// Check if a spec's text fields reference a given file path.
fn spec_references_file(
    spec: &crate::types::Spec,
    relative: &str,
    short_path: Option<&str>,
) -> bool {
    let haystack = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        spec.summary,
        spec.design_notes,
        spec.additional_notes,
        spec.goals.join(" "),
        spec.technical_requirements.join(" "),
        spec.in_scope.join(" "),
    );

    if haystack.contains(relative) {
        return true;
    }

    if let Some(short) = short_path {
        if short.len() > 5 && haystack.contains(short) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod post_tool_wiring_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;
    use std::cell::Cell;

    fn bash_input(command: &str, exit_code: i64) -> HookInput {
        HookInput {
            session_id: "post-tool-test".to_string(),
            cwd: "/tmp".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": command })),
            tool_response: Some(json!({ "exitCode": exit_code })),
            ..Default::default()
        }
    }

    fn write_input() -> HookInput {
        HookInput {
            session_id: "post-tool-test".to_string(),
            cwd: "/tmp".to_string(),
            hook_event_name: "PostToolUse".to_string(),
            tool_name: Some("Write".to_string()),
            tool_input: Some(json!({
                "file_path": "/tmp/out.bin",
                "content": "large content"
            })),
            ..Default::default()
        }
    }

    fn context(output: &HookOutput) -> String {
        post_tool_context(output).unwrap_or_default().to_string()
    }

    fn warning_output() -> HookOutput {
        HookOutput::with_post_tool_context("TMPFS WARNING".to_string())
    }

    #[test]
    fn grok_terminal_tool_normalizes_to_bash() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_WORKER_CLI", Some("grok")),
        ]);
        let input = HookInput {
            tool_name: Some("run_terminal_command".to_string()),
            ..Default::default()
        };

        let normalized = normalize_post_tool_input(&input);

        assert_eq!(normalized.tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn standalone_grok_payload_shape_normalizes_to_bash_without_factory_env() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_FACTORY_WORKER_CLI", None),
            ("CAS_FACTORY_SUPERVISOR_CLI", None),
            ("CAS_FACTORY_MODE", None),
            ("CAS_FACTORY_SESSION", None),
        ]);
        let input: HookInput = serde_json::from_value(json!({
            "hookEventName": "post_tool_use",
            "sessionId": "standalone-grok",
            "cwd": "/tmp/project",
            "workspaceRoot": "/tmp/project",
            "toolName": "run_terminal_command",
            "toolInput": {"command": "git status"},
            "toolResult": {"exitCode": 0},
            "toolInputTruncated": false
        }))
        .expect("standalone Grok fixture");

        let normalized = normalize_post_tool_input(&input);

        assert_eq!(normalized.tool_name.as_deref(), Some("Bash"));
        assert!(matches!(normalized, Cow::Owned(_)));
    }

    #[test]
    fn claude_and_codex_terminal_tool_name_is_unchanged() {
        let mut env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_WORKER_CLI", None),
        ]);
        let input = HookInput {
            tool_name: Some("run_terminal_command".to_string()),
            ..Default::default()
        };

        for harness in ["claude", "codex"] {
            env.set("CAS_FACTORY_WORKER_CLI", harness);
            let normalized = normalize_post_tool_input(&input);

            assert_eq!(
                normalized.tool_name.as_deref(),
                Some("run_terminal_command")
            );
            assert!(matches!(normalized, Cow::Borrowed(_)));
        }
    }

    #[test]
    fn post_tool_does_not_run_guardrail_for_read_hot_path() {
        let tmp = tempfile::tempdir().unwrap();
        let called = Cell::new(false);
        let input = HookInput {
            session_id: "post-tool-test".to_string(),
            cwd: tmp.path().to_string_lossy().to_string(),
            hook_event_name: "PostToolUse".to_string(),
            tool_name: Some("Read".to_string()),
            tool_input: Some(json!({ "file_path": "src/lib.rs" })),
            ..Default::default()
        };

        let output = handle_post_tool_use_with_guardrail(
            &input,
            Some(tmp.path()),
            |_cas_root, _input, _stores| {
                called.set(true);
                Some(warning_output())
            },
        )
        .unwrap();

        assert!(!called.get(), "Read must not enter tmpfs guardrail");
        assert!(post_tool_context(&output).is_none());
        assert!(
            !tmp.path().join("tmpfs_guardrail").exists(),
            "Read hot path must not touch guardrail state"
        );
    }

    #[test]
    fn post_tool_guardrail_warning_survives_capture_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[hooks]\ncapture_enabled = false\n",
        )
        .unwrap();

        let output = handle_post_tool_use_with_guardrail(
            &write_input(),
            Some(tmp.path()),
            |_cas_root, _input, _stores| Some(warning_output()),
        )
        .unwrap();

        assert_eq!(context(&output), "TMPFS WARNING");
    }

    #[test]
    fn post_tool_guardrail_warning_survives_capture_allowlist_skip() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[hooks]\ncapture_tools = [\"Bash\"]\n",
        )
        .unwrap();

        let output = handle_post_tool_use_with_guardrail(
            &write_input(),
            Some(tmp.path()),
            |_cas_root, _input, _stores| Some(warning_output()),
        )
        .unwrap();

        assert_eq!(context(&output), "TMPFS WARNING");
    }

    #[test]
    fn post_tool_guardrail_warning_returns_after_normal_bash_side_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let input = HookInput {
            session_id: "post-tool-test".to_string(),
            cwd: tmp.path().to_string_lossy().to_string(),
            hook_event_name: "PostToolUse".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": "dd if=/dev/zero of=/tmp/blob" })),
            tool_response: Some(json!({ "exitCode": 0 })),
            ..Default::default()
        };

        let output = handle_post_tool_use_with_guardrail(
            &input,
            Some(tmp.path()),
            |_cas_root, _input, _stores| Some(warning_output()),
        )
        .unwrap();

        assert_eq!(context(&output), "TMPFS WARNING");
    }

    #[test]
    fn ripple_context_merges_guardrail_warning() {
        let warning = warning_output();
        let merged = merge_post_tool_contexts(Some(&warning), "Ripple reminder".to_string());
        assert_eq!(merged, "TMPFS WARNING\nRipple reminder");
    }

    #[test]
    fn is_successful_git_commit_matches_git_commit_exit_0() {
        assert!(is_successful_git_commit(&bash_input(
            "git commit -m 'test'",
            0
        )));
    }

    #[test]
    fn is_successful_git_commit_rejects_non_zero_exit() {
        assert!(!is_successful_git_commit(&bash_input(
            "git commit -m 'test'",
            1
        )));
    }

    #[test]
    fn is_successful_git_commit_rejects_non_commit_commands() {
        assert!(!is_successful_git_commit(&bash_input("git status", 0)));
        assert!(!is_successful_git_commit(&bash_input("ls -la", 0)));
    }

    #[test]
    fn is_successful_git_commit_rejects_missing_tool_response() {
        let input = HookInput {
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": "git commit" })),
            tool_response: None,
            ..Default::default()
        };
        assert!(!is_successful_git_commit(&input));
    }

    #[test]
    fn is_successful_git_commit_rejects_amend() {
        assert!(!is_successful_git_commit(&bash_input(
            "git commit --amend -m x",
            0
        )));
    }

    #[test]
    fn is_successful_git_commit_rejects_dry_run() {
        assert!(!is_successful_git_commit(&bash_input(
            "git commit --dry-run",
            0
        )));
    }

    #[test]
    fn is_successful_git_commit_rejects_missing_exitcode_key() {
        let input = HookInput {
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": "git commit" })),
            tool_response: Some(json!({})),
            ..Default::default()
        };
        assert!(!is_successful_git_commit(&input));
    }

    #[test]
    fn with_pending_file_lock_runs_closure_and_creates_sentinel() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let tmp = std::env::temp_dir().join(format!(
            "po_lock_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let called = std::cell::Cell::new(false);
        with_pending_file_lock(&tmp, || called.set(true));

        assert!(called.get(), "closure must run");
        assert!(
            tmp.join("project-overview-pending.lock").exists(),
            "sentinel file must be created"
        );
        // Second call on same dir must not panic and must re-run closure.
        let called2 = std::cell::Cell::new(false);
        with_pending_file_lock(&tmp, || called2.set(true));
        assert!(called2.get());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn with_pending_file_lock_falls_through_when_cas_root_missing() {
        // Non-existent parent dir — OpenOptions::create will fail. Closure must
        // still run (fallthrough behavior), no panic.
        let tmp = std::env::temp_dir()
            .join(format!("po_missing_{}", std::process::id()))
            .join("does-not-exist");
        let called = std::cell::Cell::new(false);
        with_pending_file_lock(&tmp, || called.set(true));
        assert!(called.get(), "fallthrough closure must still run");
    }
}
