use crate::hooks::handlers::*;

pub fn add_loop_completion_note(
    cas_root: &std::path::Path,
    task_id: &str,
    loop_state: &crate::types::Loop,
    reason: &str,
) {
    if let Ok(task_store) = crate::store::open_task_store(cas_root) {
        if let Ok(mut task) = task_store.get(task_id) {
            let note = format!(
                "\n[{}] Loop {} - {} after {} iterations",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                loop_state.id,
                reason,
                loop_state.iteration
            );
            task.notes.push_str(&note);
            let _ = task_store.update(&task);
        }
    }
}

/// Handle loop iteration - blocks exit and feeds prompt back
///
/// This is the core of the iteration loop functionality. When an active loop
/// exists for the session, this function:
/// 1. Checks if the completion promise was found in the transcript
/// 2. Checks if max iterations were reached
/// 3. If neither, blocks exit and injects the prompt for another iteration
pub fn handle_loop_iteration(
    input: &HookInput,
    cas_root: &std::path::Path,
    loop_store: std::sync::Arc<dyn crate::store::LoopStore>,
    active_loop: &mut crate::types::Loop,
) -> Result<HookOutput, MemError> {
    // Check for completion promise in transcript
    if let Some(ref promise) = active_loop.completion_promise {
        if let Some(ref transcript_path) = input.transcript_path {
            let path = std::path::Path::new(transcript_path);
            if check_promise_in_transcript(path, promise).unwrap_or(false) {
                // Promise found - complete the loop
                active_loop.complete(&format!("Promise '{promise}' detected"));
                let _ = loop_store.update(active_loop);
                eprintln!(
                    "cas: Loop {} completed after {} iterations (promise detected)",
                    active_loop.id, active_loop.iteration
                );

                // Add final task note if linked
                if let Some(ref task_id) = active_loop.task_id {
                    add_loop_completion_note(cas_root, task_id, active_loop, "completed");
                }

                return Ok(HookOutput::empty());
            }
        }
    }

    // Check if max iterations reached
    if active_loop.is_max_reached() {
        active_loop.max_iterations_reached();
        let _ = loop_store.update(active_loop);
        eprintln!(
            "cas: Loop {} stopped after {} iterations (max reached)",
            active_loop.id, active_loop.max_iterations
        );

        // Add final task note if linked
        if let Some(ref task_id) = active_loop.task_id {
            add_loop_completion_note(cas_root, task_id, active_loop, "max iterations reached");
        }

        return Ok(HookOutput::empty());
    }

    // Increment iteration and continue
    active_loop.increment();
    let _ = loop_store.update(active_loop);

    // Build the iteration message with honesty enforcement
    let promise_info = active_loop
        .completion_promise
        .as_ref()
        .map(|p| format!(
            "\n\n---\n⚠️ **Completion**: Output `<promise>{p}</promise>` ONLY when the task is truly complete.\n\
            The statement MUST be completely and unequivocally TRUE. Do NOT lie to exit the loop.\n\
            If the loop should stop, the promise will become true naturally when work is done.\n\
            Use `/cancel-loop` to stop early if needed."
        ))
        .unwrap_or_default();

    let iteration_msg = format!(
        "🔄 Loop iteration {}{}\n\n{}{}",
        active_loop.iteration,
        active_loop
            .max_iterations
            .checked_sub(0)
            .filter(|&m| m > 0)
            .map(|m| format!("/{m}"))
            .unwrap_or_default(),
        active_loop.prompt,
        promise_info
    );

    eprintln!(
        "cas: Loop {} iteration {} starting",
        active_loop.id, active_loop.iteration
    );

    // If linked to a task, add a progress note
    if let Some(ref task_id) = active_loop.task_id {
        if let Ok(task_store) = crate::store::open_task_store(cas_root) {
            if let Ok(mut task) = task_store.get(task_id) {
                let note = format!(
                    "\n[{}] Loop iteration {} started",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                    active_loop.iteration
                );
                task.notes.push_str(&note);
                let _ = task_store.update(&task);
            }
        }
    }

    // Block exit (decision=block + reason for Claude) and surface user-visible
    // status via systemMessage. The named constructor enforces the Stop-family
    // schema invariant — hookSpecificOutput is unrepresentable here by type.
    Ok(HookOutput::block_stop_with_context(
        iteration_msg,
        format!("Loop iteration {} continuing", active_loop.iteration),
    ))
}

/// Default limit for learnings shown in review context
const LEARNING_REVIEW_LIMIT: usize = 20;

/// Build context for learning review if there are unreviewed learnings above threshold
///
/// Returns Some(context) if learning review should be triggered, None otherwise.
/// The context instructs the agent to spawn a learning-reviewer subagent.
/// Returns error context if store operations fail (fail explicitly, no silent skip).
pub fn build_learning_review_context(store: &dyn Store, config: &Config) -> Option<String> {
    // Check if learning review is enabled via hooks.stop.learning_review_enabled
    let stop_config = config.hooks.as_ref().map(|h| &h.stop);

    let enabled = stop_config
        .map(|s| s.learning_review_enabled)
        .unwrap_or(false);

    if !enabled {
        return None;
    }

    let threshold = stop_config
        .map(|s| s.learning_review_threshold)
        .unwrap_or(5);

    // Validate threshold > 0 when enabled
    if threshold == 0 {
        eprintln!(
            "cas: ERROR - learning_review_threshold must be > 0 when learning_review_enabled is true"
        );
        let mut error_context = String::new();
        error_context.push_str("<learning-review-error>\n");
        error_context.push_str("## Learning Review Configuration Error\n\n");
        error_context.push_str("The `learning_review_threshold` is set to 0, which is invalid.\n");
        error_context
            .push_str("Please set `hooks.stop.learning_review_threshold` to a value > 0.\n");
        error_context.push_str("</learning-review-error>\n");
        return Some(error_context);
    }

    // Get unreviewed learnings
    // Fail explicitly if store query fails (no silent skip)
    let unreviewed = match store.list_unreviewed_learnings(LEARNING_REVIEW_LIMIT) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cas: ERROR - Failed to get unreviewed learnings: {e}");
            // Return error context to block stop and surface the error
            let mut error_context = String::new();
            error_context.push_str("<learning-review-error>\n");
            error_context.push_str("## Learning Review Error\n\n");
            error_context.push_str(&format!(
                "Failed to check for unreviewed learnings: {e}\n\n"
            ));
            error_context.push_str(
                "The `learning_review_enabled` config option is enabled but an error occurred.\n",
            );
            error_context.push_str("Please investigate the store error before stopping.\n");
            error_context.push_str("</learning-review-error>\n");
            return Some(error_context);
        }
    };

    // Check threshold
    if unreviewed.len() < threshold {
        return None;
    }

    // Build learning review context
    let mut context = String::new();
    context.push_str("<learning-review required=\"true\">\n");
    context.push_str(&format!(
        "## Unreviewed Learnings ({} entries)\n\n",
        unreviewed.len()
    ));
    context.push_str("Before closing this session, review these learnings and determine if any should be promoted to rules or skills:\n\n");
    context.push_str("| ID | Created | Content Preview |\n");
    context.push_str("|-----|---------|----------------|\n");

    for entry in unreviewed.iter().take(10) {
        let preview = entry.preview(60);
        let created = entry.created.format("%Y-%m-%d").to_string();
        context.push_str(&format!("| {} | {} | {} |\n", entry.id, created, preview));
    }

    if unreviewed.len() > 10 {
        context.push_str(&format!(
            "\n*...and {} more learnings*\n",
            unreviewed.len() - 10
        ));
    }

    let unreviewed_ids = unreviewed
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    context.push_str("\n**Instructions:**\n");
    context.push_str(
        "1. Use the Task tool to spawn a `learning-reviewer` subagent with this prompt:\n",
    );
    context.push_str(&format!(
        "   \"Review these unreviewed learning IDs: {unreviewed_ids}. For each:\n"
    ));
    context.push_str("   - If it describes a pattern/convention → create a draft rule\n");
    context.push_str("   - If it describes a workflow/procedure → create a draft skill\n");
    context.push_str("   - If it's project-specific context → leave as learning\n");
    context.push_str("   Mark each learning as reviewed after processing.\"\n");
    context.push_str("2. After the subagent completes, you may stop.\n");
    context.push_str("</learning-review>\n");

    Some(context)
}

/// Build context for rule review when draft rules exceed threshold
///
/// Returns Some(context) if rule review should be triggered, None otherwise.
/// The context instructs the agent to spawn a rule-reviewer subagent.
/// Returns error context if store operations fail (fail explicitly, no silent skip).
pub fn build_rule_review_context(rule_store: &dyn RuleStore, config: &Config) -> Option<String> {
    // Check if rule review is enabled
    let stop_config = config.hooks.as_ref().map(|h| &h.stop);

    // An omitted [hooks] section means default configuration, so preserve the
    // enabled-by-default behavior even before callers materialize HookConfig.
    let enabled = stop_config.map(|s| s.rule_review_enabled).unwrap_or(true);

    if !enabled {
        return None;
    }

    let threshold = stop_config.map(|s| s.rule_review_threshold).unwrap_or(5);

    // Validate threshold > 0 when enabled
    if threshold == 0 {
        eprintln!(
            "cas: ERROR - rule_review_threshold must be > 0 when rule_review_enabled is true"
        );
        let mut error_context = String::new();
        error_context.push_str("<rule-review-error>\n");
        error_context.push_str("## Rule Review Configuration Error\n\n");
        error_context.push_str("The `rule_review_threshold` is set to 0, which is invalid.\n");
        error_context.push_str("Please set `hooks.stop.rule_review_threshold` to a value > 0.\n");
        error_context.push_str("</rule-review-error>\n");
        return Some(error_context);
    }

    // Get draft rules
    // Fail explicitly if store query fails (no silent skip)
    let all_rules = match rule_store.list() {
        Ok(rules) => rules,
        Err(e) => {
            eprintln!("cas: ERROR - Failed to get rules: {e}");
            // Return error context to block stop and surface the error
            let mut error_context = String::new();
            error_context.push_str("<rule-review-error>\n");
            error_context.push_str("## Rule Review Error\n\n");
            error_context.push_str(&format!("Failed to check for draft rules: {e}\n\n"));
            error_context.push_str(
                "The `rule_review_enabled` config option is enabled but an error occurred.\n",
            );
            error_context.push_str("Please investigate the store error before stopping.\n");
            error_context.push_str("</rule-review-error>\n");
            return Some(error_context);
        }
    };

    let draft_rules: Vec<_> = all_rules
        .iter()
        .filter(|r| r.status == RuleStatus::Draft)
        .collect();

    if draft_rules.len() < threshold {
        return None;
    }

    // Build rule review context
    let mut context = String::new();
    context.push_str("<rule-review required=\"true\">\n");
    context.push_str(&format!(
        "## Draft Rules Pending Review ({} rules)\n\n",
        draft_rules.len()
    ));
    context.push_str(
        "Review these draft rules and determine which should be promoted, merged, or archived:\n\n",
    );
    context.push_str("| ID | Content Preview | Helpful |\n");
    context.push_str("|----|-----------------|----------|\n");

    for rule in draft_rules.iter().take(10) {
        let preview: String = rule.content.chars().take(50).collect();
        let preview = preview.replace('\n', " ");
        context.push_str(&format!(
            "| {} | {}... | {} |\n",
            rule.id, preview, rule.helpful_count
        ));
    }

    if draft_rules.len() > 10 {
        context.push_str(&format!(
            "\n*...and {} more draft rules*\n",
            draft_rules.len() - 10
        ));
    }

    context.push_str("\n**Instructions:**\n");
    context
        .push_str("1. Use the Task tool to spawn a `rule-reviewer` subagent with this prompt:\n");
    context.push_str("   \"Review the draft rules in Cassy. For each:\n");
    context.push_str("   - If clear and validated → promote to proven\n");
    context.push_str("   - If similar to another → merge them\n");
    context.push_str("   - If vague or outdated → archive it\"\n");
    context.push_str("2. After the subagent completes, you may stop.\n");
    context.push_str("</rule-review>\n");

    Some(context)
}

/// Build context for duplicate detection when entries exceed threshold
///
/// Returns Some(context) if duplicate detection should be triggered, None otherwise.
/// The context instructs the agent to spawn a duplicate-detector subagent.
/// Returns error context if store operations fail (fail explicitly, no silent skip).
pub fn build_duplicate_detection_context(store: &dyn Store, config: &Config) -> Option<String> {
    // Check if duplicate detection is enabled
    let stop_config = config.hooks.as_ref().map(|h| &h.stop);

    let enabled = stop_config
        .map(|s| s.duplicate_detection_enabled)
        .unwrap_or(false);

    if !enabled {
        return None;
    }

    let threshold = stop_config
        .map(|s| s.duplicate_detection_threshold)
        .unwrap_or(20);

    // Validate threshold > 0 when enabled
    if threshold == 0 {
        eprintln!(
            "cas: ERROR - duplicate_detection_threshold must be > 0 when duplicate_detection_enabled is true"
        );
        let mut error_context = String::new();
        error_context.push_str("<duplicate-detection-error>\n");
        error_context.push_str("## Duplicate Detection Configuration Error\n\n");
        error_context
            .push_str("The `duplicate_detection_threshold` is set to 0, which is invalid.\n");
        error_context
            .push_str("Please set `hooks.stop.duplicate_detection_threshold` to a value > 0.\n");
        error_context.push_str("</duplicate-detection-error>\n");
        return Some(error_context);
    }

    // Count recent entries
    // Fail explicitly if store query fails (no silent skip)
    let recent = match store.list() {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cas: ERROR - Failed to get entries: {e}");
            // Return error context to block stop and surface the error
            let mut error_context = String::new();
            error_context.push_str("<duplicate-detection-error>\n");
            error_context.push_str("## Duplicate Detection Error\n\n");
            error_context.push_str(&format!("Failed to check for entries: {e}\n\n"));
            error_context.push_str(
                "The `duplicate_detection_enabled` config option is enabled but an error occurred.\n",
            );
            error_context.push_str("Please investigate the store error before stopping.\n");
            error_context.push_str("</duplicate-detection-error>\n");
            return Some(error_context);
        }
    };

    if recent.len() < threshold {
        return None;
    }

    // Build duplicate detection context
    let mut context = String::new();
    context.push_str("<duplicate-detection required=\"true\">\n");
    context.push_str(&format!(
        "## Memory Cleanup Recommended ({} entries)\n\n",
        recent.len()
    ));
    context.push_str("Check for duplicate or near-duplicate entries that can be consolidated:\n\n");
    context.push_str("| ID | Type | Content Preview |\n");
    context.push_str("|----|------|----------------|\n");

    for entry in recent.iter().take(15) {
        let preview = entry.preview(40);
        let entry_type = format!("{:?}", entry.entry_type);
        context.push_str(&format!(
            "| {} | {} | {} |\n",
            entry.id, entry_type, preview
        ));
    }

    context.push_str("\n**Instructions:**\n");
    context.push_str(
        "1. Use the Task tool to spawn a `duplicate-detector` subagent with this prompt:\n",
    );
    context.push_str("   \"Scan Cassy memories for duplicates. For each duplicate pair:\n");
    context.push_str("   - Merge content into the more complete entry\n");
    context.push_str("   - Archive the redundant entry\n");
    context.push_str("   - Report statistics on space saved\"\n");
    context.push_str("2. After the subagent completes, you may stop.\n");
    context.push_str("</duplicate-detection>\n");

    Some(context)
}

/// Build context for session summary generation
///
/// Returns Some(context) if generate_summary is enabled and no summary exists yet.
/// The context instructs the agent to spawn a session-summarizer subagent.
/// Returns an error context if store operations fail (fail explicitly, no silent skip).
pub fn build_session_summary_context(
    store: &dyn Store,
    config: &Config,
    session_id: &str,
) -> Option<String> {
    // Check if summary generation is enabled via hooks.stop.generate_summary
    let stop_config = config.hooks.as_ref().map(|h| &h.stop);

    let enabled = stop_config.map(|s| s.generate_summary).unwrap_or(false);

    if !enabled {
        return None;
    }

    // Check if a summary already exists for this session
    // Fail explicitly if store query fails (no silent skip)
    let session_entries = match store.list_by_session(session_id) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cas: ERROR - Failed to check for existing summaries: {e}");
            // Return error context to block stop and surface the error
            let mut error_context = String::new();
            error_context.push_str("<session-summary-error>\n");
            error_context.push_str("## Session Summary Error\n\n");
            error_context.push_str(&format!(
                "Failed to check for existing session summaries: {e}\n\n"
            ));
            error_context.push_str(
                "The `generate_summary` config option is enabled but an error occurred.\n",
            );
            error_context.push_str("Please investigate the store error before stopping.\n");
            error_context.push_str("</session-summary-error>\n");
            return Some(error_context);
        }
    };

    let has_summary = session_entries.iter().any(|e| {
        e.tags
            .iter()
            .any(|t| t == "session-summary" || t == "summary")
    });

    if has_summary {
        // Summary already exists, don't re-trigger
        return None;
    }

    // Build session summary context
    let mut context = String::new();
    context.push_str("<session-summary required=\"true\">\n");
    context.push_str("## Session Summary Required\n\n");
    context.push_str(
        "Before ending this session, generate a summary of the work done, decisions made, and learnings captured.\n\n",
    );
    context.push_str("**Instructions:**\n");
    context.push_str(
        "1. Use the Task tool to spawn a `session-summarizer` subagent with this prompt:\n",
    );
    // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — the summarizer subagent
    // inherits the same process's MCP registration as its parent, so it
    // needs the parent's own tool prefix here.
    let prefix = crate::harness_policy::own_tool_prefix();
    context.push_str("   \"Generate a session summary. Steps:\n");
    context.push_str(&format!(
        "   - Get session context using {prefix}search action=context\n"
    ));
    context.push_str(&format!(
        "   - List tasks worked on using {prefix}task action=mine\n"
    ));
    context.push_str(&format!(
        "   - Get recent memories using {prefix}memory action=recent limit=20\n"
    ));
    context.push_str("   - Create a structured summary covering: completed work, decisions, files changed, learnings, blockers, next steps\n");
    context.push_str(&format!(
        "   - Store the summary using {prefix}memory action=remember with tags session,summary\"\n"
    ));
    context.push_str("2. After the subagent completes, you may stop.\n");
    context.push_str("</session-summary>\n");

    Some(context)
}

// =============================================================================
// B3 (cas-5c0a): Worker-stop hook — emit final git-state event
// =============================================================================

/// Emit a `WorkerGitCommit` daemon event carrying the worker's final git state
/// when a factory worker session ends.
///
/// Reuses `collect_worker_git_status` from B1 (`factory_ops`) so there is no
/// divergent duplicate collector.  The event appears in `coordination
/// action=worker_activity` and lets the supervisor see branch/HEAD/ahead-behind/
/// pushed-ref/PR without any manual `git log` forensics.
///
/// Failure is best-effort: if the event store is unreachable or the git
/// commands fail, the function logs to stderr and returns without panicking.
pub(crate) fn emit_worker_final_git_state(
    cas_root: &std::path::Path,
    worker_name: &str,
    cwd: &std::path::Path,
    session_id: &str,
) {
    use crate::mcp::tools::service::factory_ops::collect_worker_git_status;
    use cas_store::{EventStore, SqliteEventStore};
    use cas_types::{Event, EventEntityType, EventType};

    let gs = collect_worker_git_status(cwd);

    // Build a compact one-line summary for the activity feed.
    // cas-86c5: use "checkpoint" not "session-stop" — these are routine
    // per-turn Stop-hook snapshots, not terminal events. "session-stop"
    // implied the worker died; supervisors were incorrectly using it as a
    // liveness signal and resetting tasks on active workers.
    let dirty_tag = if gs.dirty { "dirty" } else { "clean" };
    let pushed_tag = if gs.pushed_ref == "none" {
        "not-pushed"
    } else {
        &gs.pushed_ref
    };
    let summary = format!(
        "checkpoint git-state: {} @ {} [{}] [{}] ahead:{} behind:{} PR: {}",
        gs.branch,
        crate::mcp::tools::service::factory_ops::head_sha_for_display(&gs.head_sha),
        dirty_tag,
        pushed_tag,
        gs.ahead,
        gs.behind,
        gs.pr_url,
    );

    // Shell-form emission: echo to stderr so it appears in the session JSONL
    // (Claude Code captures stderr in the hook output stream, cas-5c0a AC3).
    eprintln!("cas: worker {} turn-checkpoint: {summary}", worker_name);

    // Structured metadata for machine-readable consumption.
    let metadata = serde_json::json!({
        "branch": gs.branch,
        "head_sha": gs.head_sha,
        "ahead": gs.ahead,
        "behind": gs.behind,
        "base_branch": gs.base_branch,
        "dirty": gs.dirty,
        "pushed_ref": gs.pushed_ref,
        "pr_url": gs.pr_url.to_string(),
    });

    let event = Event::new(
        EventType::WorkerGitCommit,
        EventEntityType::Agent,
        worker_name,
        &summary,
    )
    .with_session(session_id)
    .with_metadata(metadata);

    // Best-effort write: if the store is unreachable, log and continue.
    if let Ok(store) = SqliteEventStore::open(cas_root) {
        if let Err(e) = store.record(&event) {
            eprintln!("cas: worker {worker_name} stop: failed to record git-state event: {e}");
        }
    }
}

// =============================================================================

/// Handle Stop hook - generate session summary when agent finishes
///
/// This is triggered when the agent explicitly finishes its task,
/// making it ideal for generating a comprehensive session summary.
///
/// Implements "compaction" pattern from context engineering:
/// - Summarizes session activities
/// - Preserves architectural decisions
/// - Captures unresolved issues for future sessions
mod stop_flow;
mod synthesis;

pub use stop_flow::handle_stop;
pub use synthesis::{
    synthesize_buffered_observations, synthesize_buffered_observations_with_sources,
};

// =============================================================================
// B3 tests
// =============================================================================

#[cfg(test)]
mod tests_b3 {
    use super::emit_worker_final_git_state;
    use cas_store::{EventStore, SqliteEventStore};
    use cas_types::EventType;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_git_repo_with_factory_branch(tmp: &std::path::Path) {
        Command::new("git").args(["init", "-b", "main"]).current_dir(tmp).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@cas"]).current_dir(tmp).output().unwrap();
        Command::new("git").args(["config", "user.name", "Cassy Test"]).current_dir(tmp).output().unwrap();
        std::fs::write(tmp.join("README"), "init").unwrap();
        Command::new("git").args(["add", "README"]).current_dir(tmp).output().unwrap();
        Command::new("git").args(["commit", "-m", "init"]).current_dir(tmp).output().unwrap();
        Command::new("git")
            .args(["checkout", "-b", "factory/stop-worker"])
            .current_dir(tmp)
            .output()
            .unwrap();
        std::fs::write(tmp.join("work.rs"), "// task").unwrap();
        Command::new("git").args(["add", "work.rs"]).current_dir(tmp).output().unwrap();
        Command::new("git")
            .args(["commit", "-m", "feat: worker task done"])
            .current_dir(tmp)
            .output()
            .unwrap();
    }

    /// AC1 + AC4 (cas-5c0a): emit_worker_final_git_state writes a WorkerGitCommit
    /// event to the event store carrying the worker's branch and session ID.
    ///
    /// FAILS with the stub (stub does nothing; store has 0 events).
    /// PASSES once the real implementation calls the event store.
    #[test]
    fn emit_final_git_state_writes_worker_git_commit_event() {
        let git_dir = TempDir::new().expect("git tempdir");
        setup_git_repo_with_factory_branch(git_dir.path());

        let cas_dir = TempDir::new().expect("cas tempdir");

        emit_worker_final_git_state(
            cas_dir.path(),
            "stop-worker",
            git_dir.path(),
            "ses-b3-test-001",
        );

        let store = SqliteEventStore::open(cas_dir.path())
            .expect("event store must open");
        let events = store.list_recent(10).expect("list_recent must succeed");

        assert!(
            !events.is_empty(),
            "emit must write at least one event; got 0 — stub was not replaced"
        );

        let evt = &events[0];
        assert_eq!(
            evt.event_type,
            EventType::WorkerGitCommit,
            "event_type must be WorkerGitCommit; got {:?}",
            evt.event_type
        );
        assert_eq!(
            evt.entity_id, "stop-worker",
            "entity_id must be the worker name"
        );
        assert!(
            evt.summary.contains("factory/stop-worker"),
            "summary must include branch name: {}",
            evt.summary
        );
        assert_eq!(
            evt.session_id.as_deref(),
            Some("ses-b3-test-001"),
            "session_id must be set on the event"
        );
    }

    /// cas-86c5: per-turn git-state snapshots must use "checkpoint" not
    /// "session-stop" in their activity-feed summary. "session-stop" implied
    /// the worker died; supervisors were resetting tasks on alive workers as a
    /// result. This test is a mutation guard — if the format! call reverts to
    /// "session-stop", it fails here before any supervisor sees the misleading text.
    #[test]
    fn git_state_summary_uses_checkpoint_not_session_stop() {
        let git_dir = TempDir::new().expect("git tempdir");
        setup_git_repo_with_factory_branch(git_dir.path());

        let cas_dir = TempDir::new().expect("cas tempdir");

        emit_worker_final_git_state(
            cas_dir.path(),
            "guard-worker",
            git_dir.path(),
            "ses-86c5-label-guard",
        );

        let store = SqliteEventStore::open(cas_dir.path()).expect("event store must open");
        let events = store.list_recent(5).expect("list_recent must succeed");

        assert!(!events.is_empty(), "must write at least one event");
        let summary = &events[0].summary;

        assert!(
            !summary.starts_with("session-stop"),
            "summary must NOT start with 'session-stop' — use 'checkpoint' instead \
             (cas-86c5: misleading label causes supervisors to reset alive workers). \
             Got: {summary}"
        );
        assert!(
            summary.starts_with("checkpoint"),
            "summary must start with 'checkpoint'. Got: {summary}"
        );
        assert!(
            summary.contains("PR: unknown (branch not on origin locally)"),
            "a missing local remote ref must remain distinguishable from no PR: {summary}"
        );
    }

    /// AC2 (cas-5c0a): WorkerGitStatus is pub(crate) from factory_ops — reuse contract.
    /// This is a compile-time assertion; if it compiles, the contract holds.
    #[test]
    fn worker_git_status_struct_is_pub_crate_from_factory_ops() {
        use crate::mcp::tools::service::factory_ops::{WorkerGitStatus, WorkerPrUrl};
        // Constructing it proves visibility — no runtime behavior needed.
        let _ = WorkerGitStatus {
            branch: "factory/x".to_string(),
            head_sha: "abc1234".to_string(),
            ahead: 1,
            behind: 0,
            base_branch: "main".to_string(),
            dirty: false,
            pushed_ref: "none".to_string(),
            pr_url: WorkerPrUrl::None,
            is_shared_checkout: false,
        };
    }
}
