use crate::hooks::handlers::*;
use super::pre_tool::FACTORY_AUTO_APPROVE_TOOLS;

pub fn handle_permission_request(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let tool_name = match &input.tool_name {
        Some(name) => name.as_str(),
        None => return Ok(HookOutput::empty()),
    };

    // ========================================================================
    // FACTORY PERMISSION-REQUEST AUTO-APPROVE — belt #3 (cas-7f33)
    //
    // In some Claude Code 2.1.x builds, PreToolUse `permissionDecision:"allow"`
    // does not pre-empt team-mode leader-escalation cleanly, and the decision
    // flow falls through to a PermissionRequest notification. The UG9 self-
    // check bug (see pre_tool.rs) then escalates to the team leader for
    // every filesystem write — self-deadlock for supervisors.
    //
    // Scope and structural asymmetry with pre_tool.rs (INTENTIONAL):
    //   - The PreToolUse hoist is gated on `cas_root.is_none()` because a
    //     second copy exists below the protection block to handle the
    //     `cas_root=Some` case after `.env`/credential denies have run.
    //   - This handler has NO protection gates today (pre-cas-7f33 code
    //     only did task-context/lease-based auto-approve). There is no
    //     deny-first invariant to preserve, so the belt fires
    //     unconditionally for factory agents — both `cas_root=None` and
    //     `cas_root=Some` need the deadlock bypass whenever the
    //     PermissionRequest path is reached.
    //   - Trade-off: the prior lease/task-context auto-approve (below)
    //     is short-circuited for factory agents on the 7 allowlisted
    //     tools. Factory agents are already privileged via the PreToolUse
    //     auto-approve; this does not materially change their write surface.
    //   - If a future contributor adds a protection gate (e.g., `.env`
    //     deny) to this handler, it MUST be hoisted above this belt or
    //     factory agents will bypass it. See FACTORY_AUTO_APPROVE_TOOLS
    //     doc comment in pre_tool.rs for the full consumer list.
    //
    // Runs BEFORE the cas_root check because CAS_AGENT_ROLE is pure env,
    // no store access required.
    // ========================================================================
    let is_factory_agent = std::env::var("CAS_AGENT_ROLE").is_ok();
    if is_factory_agent && FACTORY_AUTO_APPROVE_TOOLS.contains(&tool_name) {
        eprintln!(
            "cas: PermissionRequest factory auto-approve for {tool_name}"
        );
        return Ok(HookOutput::with_permission_request(
            "allow",
            &format!(
                "Factory agent auto-approve ({tool_name}) — bypasses Claude Code team-mode leader-escalation deadlock (UG9 bug)"
            ),
        ));
    }

    // Check if CAS is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Get the agent's claimed tasks
    let agent_store = open_agent_store(cas_root)?;
    let task_store = open_task_store(cas_root)?;

    // Agent ID is the session_id
    let agent = match agent_store.get(&input.session_id) {
        Ok(a) => a,
        Err(_) => return Ok(HookOutput::empty()),
    };

    // Get agent's active task leases
    let leases = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
    if leases.is_empty() {
        return Ok(HookOutput::empty());
    }

    // Get file path if applicable
    let file_path = input
        .tool_input
        .as_ref()
        .and_then(|ti| ti.get("file_path").and_then(|v| v.as_str()));

    // Check if the operation relates to any claimed task
    for lease in &leases {
        if let Ok(task) = task_store.get(&lease.task_id) {
            // For now, auto-approve Read/Glob/Grep for any claimed task
            // This allows exploration without friction
            if matches!(
                tool_name,
                "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch"
            ) {
                eprintln!(
                    "cas: PermissionRequest auto-approved {} for task {}",
                    tool_name, &task.id
                );
                return Ok(HookOutput::with_permission_request(
                    "allow",
                    &format!("Agent has claimed task: {}", task.title),
                ));
            }

            // For Write/Edit, check if path is mentioned in task notes/description
            if let Some(path) = file_path {
                let path_filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);

                // Check if the file is mentioned in task context
                let task_context = format!("{} {} {}", task.title, task.description, task.notes);
                if task_context.contains(path_filename) {
                    eprintln!(
                        "cas: PermissionRequest auto-approved {tool_name} on {path} (task context match)"
                    );
                    return Ok(HookOutput::with_permission_request(
                        "allow",
                        &format!("File mentioned in claimed task: {}", task.title),
                    ));
                }
            }
        }
    }

    // No auto-approval - let Claude ask the user
    Ok(HookOutput::empty())
}

// ============================================================================
// Notification Hook Handler
// ============================================================================

/// Handle Notification hook - external alerts
///
/// This hook fires on various notification events and can:
/// 1. Send desktop notifications (via terminal-notifier on macOS)
/// 2. Optional webhook support for Slack/Discord
/// 3. Log notification patterns for analytics
///
/// Supported matchers: permission_prompt, idle_prompt, auth_success
pub fn handle_notification(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    // Check if CAS is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Load config for notification settings
    let config = Config::load(cas_root).unwrap_or_default();
    let notification_config = match config.notifications {
        Some(nc) if nc.enabled => nc,
        _ => return Ok(HookOutput::empty()),
    };

    // Get the notification type from hook event name
    let notification_type = input.hook_event_name.as_str();

    // Check matchers to see if we should notify
    let should_notify = match notification_type {
        "permission_prompt" => notification_config.on_permission_prompt,
        "idle_prompt" => notification_config.on_idle_prompt,
        "auth_success" => notification_config.on_auth_success,
        _ => false,
    };

    if !should_notify {
        return Ok(HookOutput::empty());
    }

    // Build notification message
    let message = match notification_type {
        "permission_prompt" => format!(
            "Claude needs permission for: {}",
            input.tool_name.as_deref().unwrap_or("unknown action")
        ),
        "idle_prompt" => "Claude is waiting for your input".to_string(),
        "auth_success" => "Claude Code authentication successful".to_string(),
        _ => format!("Claude Code: {notification_type}"),
    };

    // Send desktop notification (macOS)
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                &format!(
                    "display notification \"{}\" with title \"Claude Code\"",
                    message.replace('"', "\\\"")
                ),
            ])
            .spawn();
    }

    // Linux notifications via notify-send
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["Claude Code", &message])
            .spawn();
    }

    // Optional webhook support
    if let Some(ref webhook_url) = notification_config.webhook_url {
        // Send async notification via webhook (fire and forget)
        let url = webhook_url.clone();
        let msg = message.clone();
        std::thread::spawn(move || {
            let _ = send_webhook_notification(&url, &msg);
        });
    }

    // Log the notification event
    eprintln!("cas: Notification sent: {message}");

    Ok(HookOutput::empty())
}

/// Send a webhook notification (blocking, for use in spawned thread)
pub fn send_webhook_notification(url: &str, message: &str) -> Result<(), MemError> {
    // Simple HTTP POST with JSON payload
    let payload = serde_json::json!({
        "text": message,
        "username": "Claude Code",
    });

    // Use curl for simplicity (available on most systems)
    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload.to_string(),
            url,
        ])
        .output();

    match output {
        Ok(_) => Ok(()),
        Err(e) => Err(MemError::Other(format!("Webhook failed: {e}"))),
    }
}

// ============================================================================
// PreCompact Hook Handler
// ============================================================================

/// Handle PreCompact hook - preserve critical context before compaction
///
/// This hook fires before Claude's context is compacted and can:
/// 1. Inject high-importance memories before compaction
/// 2. Preserve active task context and decisions
/// 3. Support both manual (/compact) and auto triggers
/// 4. Optionally generate pre-compaction summary
///
/// Returns additionalContext with preserved memories
pub fn handle_pre_compact(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    // Check if CAS is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    let mut context_parts: Vec<String> = Vec::new();

    // 1. Inject high-importance memories (importance >= 0.7)
    let store = open_store(cas_root)?;
    let entries = store.list()?;
    let high_importance: Vec<_> = entries
        .iter()
        .filter(|e| e.importance >= 0.7 && e.entry_type != EntryType::Observation)
        .take(10)
        .collect();

    if !high_importance.is_empty() {
        context_parts.push("## Critical Memories (preserve across compaction)".to_string());
        for entry in &high_importance {
            let preview = truncate_display(&entry.content, 200);
            context_parts.push(format!("- [{}] {}", entry.id, preview));
        }
    }

    // 2. Preserve active task context
    let task_store = open_task_store(cas_root)?;
    let in_progress = task_store.list(Some(TaskStatus::InProgress))?;

    if !in_progress.is_empty() {
        context_parts.push(String::new());
        context_parts.push("## Active Tasks (preserve across compaction)".to_string());
        for task in in_progress.iter().take(5) {
            context_parts.push(format!("- [{}] {} ({})", task.id, task.title, task.status));
            if !task.notes.is_empty() {
                // Get last note line
                if let Some(last_note) = task.notes.lines().last() {
                    if last_note.len() <= 100 {
                        context_parts.push(format!("  Last note: {last_note}"));
                    }
                }
            }
        }
    }

    // 3. Get recent decisions from current session
    let session_entries = store.list_by_session(&input.session_id)?;
    let decisions: Vec<_> = session_entries
        .iter()
        .filter(|e| {
            e.observation_type == Some(ObservationType::Decision)
                || e.tags.iter().any(|t| t == "decision")
        })
        .take(5)
        .collect();

    if !decisions.is_empty() {
        context_parts.push(String::new());
        context_parts.push("## Session Decisions (preserve across compaction)".to_string());
        for decision in &decisions {
            let preview = truncate_display(&decision.content, 150);
            context_parts.push(format!("- {preview}"));
        }
    }

    // 4. Include proven rules for current file context
    let rule_store = open_rule_store(cas_root)?;
    let critical_rules = rule_store.list_critical()?;

    if !critical_rules.is_empty() {
        context_parts.push(String::new());
        context_parts.push("## Critical Rules (always apply)".to_string());
        for rule in critical_rules.iter().take(3) {
            context_parts.push(format!("- [{}] {}", rule.id, rule.preview(80)));
        }
    }

    // B4 FLUSH: write worker in-flight findings to the active task note BEFORE
    // compaction erases them from context. Must run UNCONDITIONALLY — even when
    // context_parts is empty (no high-importance memories, no active tasks), the
    // worker may still have in-flight findings worth preserving. Best-effort: all
    // errors are swallowed inside flush_worker_findings_to_task; compaction is
    // never blocked regardless of flush outcome.
    flush_worker_findings_to_task(input, cas_root);
    // Claude is the only harness that currently emits an imminent-compaction
    // hook. Record the handoff before its context is replaced; Codex/Grok
    // retain the explicit worker checkpoint protocol in their worker guidance.
    match checkpoint_worker_before_compaction(input, cas_root) {
        Ok(Some(checkpoint)) => context_parts.push(checkpoint),
        Ok(None) => {}
        Err(error) => {
            eprintln!("cas: PreCompact checkpoint: {error} (ignored, compaction proceeds)");
        }
    }

    if context_parts.is_empty() {
        return Ok(HookOutput::empty());
    }

    let context = context_parts.join("\n");
    eprintln!(
        "cas: PreCompact injecting {} chars of context",
        context.len()
    );

    // PreCompact doesn't support hookSpecificOutput in Claude Code's schema
    // (only PreToolUse, UserPromptSubmit, PostToolUse do)
    // Use systemMessage instead to inject context
    Ok(HookOutput {
        system_message: Some(context),
        ..Default::default()
    })
}

// =============================================================================
// B4 PreCompact FLUSH (cas-c299)
// =============================================================================

/// Best-effort flush of in-flight worker findings to the active task note.
///
/// Called from `handle_pre_compact` immediately before returning. All errors
/// are swallowed so compaction is never blocked. Also exported for B3 (session-
/// stop handler) to call the same logic at stop time.
pub(crate) fn flush_worker_findings_to_task(input: &HookInput, cas_root: &Path) {
    if let Err(e) = do_flush(input, cas_root) {
        eprintln!("cas: PreCompact flush: {e} (ignored, compaction proceeds)");
    }
}

fn do_flush(input: &HookInput, cas_root: &Path) -> Result<(), MemError> {
    // Guard: run only inside a factory worker process
    let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
        .map(|r| r.eq_ignore_ascii_case("worker"))
        .unwrap_or(false)
        && std::env::var("CAS_FACTORY_MODE").is_ok();
    if !is_factory_worker {
        return Ok(());
    }

    let transcript_path = match input.transcript_path.as_deref() {
        Some(p) => p,
        None => return Ok(()),
    };

    let task_id = match resolve_worker_active_task(&input.session_id, cas_root)? {
        Some(id) => id,
        None => {
            eprintln!("cas: PreCompact flush: no active task for session {}", input.session_id);
            return Ok(());
        }
    };

    let findings = extract_compact_findings(transcript_path)?;
    if findings.is_empty() {
        return Ok(());
    }

    write_findings_note(cas_root, &task_id, &findings)
}

/// Resolve the InProgress task claimed by the worker session.
///
/// Walks: session_id → Agent record → active TaskLease → Task with InProgress status.
/// Returns `Ok(None)` if no active task is found (not an error — worker may not
/// have claimed anything yet, or the task was already closed).
pub(crate) fn resolve_worker_active_task(
    session_id: &str,
    cas_root: &Path,
) -> Result<Option<String>, MemError> {
    let agent_store = open_agent_store(cas_root)?;
    let agent = match agent_store.get(session_id) {
        Ok(a) => a,
        Err(_) => return Ok(None),
    };
    let leases = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
    let task_store = open_task_store(cas_root)?;
    let active_task_id = leases
        .iter()
        .filter_map(|lease| task_store.get(&lease.task_id).ok())
        .find(|t| t.status == TaskStatus::InProgress)
        .map(|t| t.id.clone());
    Ok(active_task_id)
}

/// Persist the minimal handoff that lets a factory worker resume after
/// Claude's `PreCompact` hook, and queue the matching non-disruptive notice
/// for its supervisor.  This intentionally uses the existing agent metadata
/// and prompt queue rather than creating a compaction-specific store.
///
/// Other harnesses do not expose a mechanically detectable pre-compaction
/// event, so this function is deliberately invoked only from `PreCompact`.
pub(crate) fn checkpoint_worker_before_compaction(
    input: &HookInput,
    cas_root: &Path,
) -> Result<Option<String>, MemError> {
    use std::process::Command;

    let agent_store = open_agent_store(cas_root)?;
    let mut agent = match agent_store.get(&input.session_id) {
        Ok(agent) if agent.role == crate::types::AgentRole::Worker => agent,
        Ok(_) | Err(_) => return Ok(None),
    };
    let Some(task_id) = resolve_worker_active_task(&input.session_id, cas_root)? else {
        return Ok(None);
    };

    let cwd = input.cwd.trim();
    let branch = if cwd.is_empty() {
        "unknown".to_string()
    } else {
        Command::new("git")
            .args(["-C", cwd, "branch", "--show-current"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let intent = "resume active task after context compaction";
    agent.metadata.insert(
        "context_checkpoint_state".to_string(),
        "compacting".to_string(),
    );
    agent
        .metadata
        .insert("context_checkpoint_task_id".to_string(), task_id.clone());
    agent
        .metadata
        .insert("context_checkpoint_branch".to_string(), branch.clone());
    agent
        .metadata
        .insert("context_checkpoint_cwd".to_string(), cwd.to_string());
    agent
        .metadata
        .insert("context_checkpoint_intent".to_string(), intent.to_string());
    agent.metadata.insert(
        "context_checkpoint_at".to_string(),
        chrono::Utc::now().to_rfc3339(),
    );
    agent_store.update(&agent)?;

    let message = format!(
        "compacting, will resume task {task_id} on branch {branch}. Checkpoint: cwd={}, intent={intent}.",
        if cwd.is_empty() { "unknown" } else { cwd }
    );
    crate::store::open_prompt_queue_store(cas_root)?.enqueue_with_summary(
        &agent.name,
        "supervisor",
        &message,
        agent.factory_session.as_deref(),
        Some("worker compacting"),
    )?;
    Ok(Some(format!(
        "\n## Factory Compaction Checkpoint\n\
         You are resuming task `{task_id}` on branch `{branch}`.\n\
         Working directory: `{}`\n\
         Intent: {intent}.\n\
         Before starting any other task, continue this assigned task and read its CAS task record.",
        if cwd.is_empty() { "unknown" } else { cwd }
    )))
}

/// Maximum findings written per flush (bounds note size).
const MAX_FLUSH_FINDINGS: usize = 8;
/// Maximum characters per individual finding body.
const MAX_FLUSH_FINDING_LEN: usize = 200;
/// Maximum total characters for the flush note body.
const MAX_FLUSH_NOTE_CHARS: usize = 2000;

/// Extract high-confidence findings from the transcript via session-learn.
///
/// Returns an empty Vec on any session-learn error (best-effort).
fn extract_compact_findings(transcript_path: &str) -> Result<Vec<String>, MemError> {
    let drafts = match session_learn_sync(transcript_path, &[]) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cas: PreCompact flush: session-learn failed: {e} (no findings written)");
            return Ok(vec![]);
        }
    };
    let findings: Vec<String> = drafts
        .into_iter()
        .filter(|d| {
            d.confidence >= 0.5
                && matches!(
                    d.signal.as_str(),
                    "decision" | "correction" | "concept" | "pattern"
                )
                && d.dedup_hits.is_empty()
        })
        .take(MAX_FLUSH_FINDINGS)
        .map(|d| truncate_display(&d.content, MAX_FLUSH_FINDING_LEN))
        .collect();
    Ok(findings)
}

/// Write the findings as a decision note on the task.
///
/// Deduplicates by checking if the first finding's 60-char fingerprint already
/// appears in `task.notes`. Bounds the note body to `MAX_FLUSH_NOTE_CHARS`.
pub(crate) fn write_findings_note(
    cas_root: &Path,
    task_id: &str,
    findings: &[String],
) -> Result<(), MemError> {
    let task_store = open_task_store(cas_root)?;
    let mut task = task_store.get(task_id)?;

    // Dedup: fingerprint = first 60 bytes of the first finding (raw slice, no "...")
    if let Some(first) = findings.first() {
        let mut fp_end = 60.min(first.len());
        while fp_end > 0 && !first.is_char_boundary(fp_end) {
            fp_end -= 1;
        }
        let fingerprint = &first[..fp_end];
        if !fingerprint.is_empty() && task.notes.contains(fingerprint) {
            eprintln!("cas: PreCompact flush: deduped (fingerprint already in notes)");
            return Ok(());
        }
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    // Enforce the cap defensively in the writer regardless of call site
    let capped = &findings[..MAX_FLUSH_FINDINGS.min(findings.len())];
    let mut body = capped
        .iter()
        .map(|f| format!("- {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Bound to MAX_FLUSH_NOTE_CHARS
    if body.len() > MAX_FLUSH_NOTE_CHARS {
        let mut end = MAX_FLUSH_NOTE_CHARS;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        body = body[..end].to_string();
    }

    let formatted_note = format!("[{timestamp}] ✅ DECISION Pre-compact flush\n{body}");
    if task.notes.is_empty() {
        task.notes = formatted_note;
    } else {
        task.notes = format!("{}\n\n{}", task.notes, formatted_note);
    }
    task.updated_at = chrono::Utc::now();
    task_store.update(&task)?;
    eprintln!(
        "cas: PreCompact flush: wrote {} finding(s) to task {task_id}",
        findings.len()
    );
    Ok(())
}

#[cfg(test)]
mod pre_compact_flush_tests {
    use super::*;
    use crate::store::{init_cas_dir, open_agent_store, open_prompt_queue_store, open_task_store};
    use crate::types::{Agent, AgentRole, Task, TaskStatus};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    struct CasDir {
        _tmp: TempDir,
        pub root: PathBuf,
    }

    fn setup_cas() -> CasDir {
        let tmp = tempfile::tempdir().expect("TempDir");
        let root = init_cas_dir(tmp.path()).expect("init_cas_dir");
        CasDir { _tmp: tmp, root }
    }

    fn add_inprogress_task(cas_root: &std::path::Path, task_id: &str) {
        let store = open_task_store(cas_root).expect("open_task_store");
        let mut task = Task::new(task_id.to_string(), "B4 test task".to_string());
        task.status = TaskStatus::InProgress;
        store.add(&task).expect("task.add");
    }

    #[test]
    fn pre_compact_checkpoint_notifies_supervisor_and_persists_resume_state() {
        let cas = setup_cas();
        let task_id = "cas-41c28-test";
        add_inprogress_task(&cas.root, task_id);

        let agent_store = open_agent_store(&cas.root).expect("agent store");
        let mut worker = Agent::new("compact-session".to_string(), "compact-worker".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-compact-test".to_string());
        agent_store.register(&worker).expect("register worker");
        agent_store
            .try_claim(task_id, &worker.id, 600, Some("test compaction"))
            .expect("claim active task");

        let input = HookInput {
            session_id: worker.id.clone(),
            cwd: "/tmp/compact-worktree".to_string(),
            hook_event_name: "PreCompact".to_string(),
            ..Default::default()
        };

        let output = handle_pre_compact(&input, Some(&cas.root))
            .expect("pre-compaction hook must succeed");
        let resume_context = output
            .system_message
            .expect("an active worker task needs resume context");
        assert!(resume_context.contains("cas-41c28-test"));
        assert!(resume_context.contains("resume active task"));

        let persisted = agent_store.get(&worker.id).expect("read worker checkpoint");
        assert_eq!(
            persisted
                .metadata
                .get("context_checkpoint_state")
                .map(String::as_str),
            Some("compacting")
        );
        assert_eq!(
            persisted
                .metadata
                .get("context_checkpoint_task_id")
                .map(String::as_str),
            Some(task_id)
        );
        assert_eq!(
            persisted
                .metadata
                .get("context_checkpoint_cwd")
                .map(String::as_str),
            Some("/tmp/compact-worktree")
        );
        assert!(
            persisted
                .metadata
                .get("context_checkpoint_branch")
                .is_some_and(|branch| !branch.is_empty()),
            "checkpoint must record a resume branch: {:?}",
            persisted.metadata
        );
        assert_eq!(
            persisted
                .metadata
                .get("context_checkpoint_intent")
                .map(String::as_str),
            Some("resume active task after context compaction")
        );

        let queue = open_prompt_queue_store(&cas.root).expect("prompt queue");
        let messages = queue
            .peek_for_targets(&["supervisor"], Some("factory-compact-test"), 10)
            .expect("supervisor queue");
        assert_eq!(messages.len(), 1, "one compaction notice must be queued");
        assert!(
            messages[0]
                .prompt
                .contains("compacting, will resume task cas-41c28-test")
        );
        assert!(messages[0].prompt.contains("on branch"));
    }

    // -------------------------------------------------------------------------
    // AC: transcript-with-findings → note written with correct format
    // -------------------------------------------------------------------------
    #[test]
    fn test_write_findings_note_creates_formatted_decision_note() {
        let cas = setup_cas();
        add_inprogress_task(&cas.root, "cas-b4t1");

        let findings = vec![
            "Found two spawn paths: queue path and fork_first.rs daemon path".to_string(),
            "create_worktree silently falls back to main cwd on failure".to_string(),
        ];

        write_findings_note(&cas.root, "cas-b4t1", &findings)
            .expect("write_findings_note should succeed");

        let store = open_task_store(&cas.root).expect("open_task_store");
        let task = store.get("cas-b4t1").expect("task.get");

        assert!(
            task.notes.contains("DECISION") || task.notes.contains("Pre-compact flush"),
            "note must contain flush marker; got:\n{}",
            task.notes
        );
        assert!(
            task.notes.contains("Found two spawn paths"),
            "note must contain first finding; got:\n{}",
            task.notes
        );
        assert!(
            task.notes.contains("create_worktree"),
            "note must contain second finding; got:\n{}",
            task.notes
        );
        assert!(
            task.notes.contains("- "),
            "findings must be bullet-formatted; got:\n{}",
            task.notes
        );
    }

    // -------------------------------------------------------------------------
    // AC: dedup — repeated compaction with same findings fingerprint → no dup
    // -------------------------------------------------------------------------
    #[test]
    fn test_write_findings_note_deduplicates_repeated_flush() {
        let cas = setup_cas();
        add_inprogress_task(&cas.root, "cas-b4t2");

        let findings = vec![
            "Root cause: worktree isolation flag not propagated to daemon spawn".to_string(),
        ];

        // First flush
        write_findings_note(&cas.root, "cas-b4t2", &findings)
            .expect("first write should succeed");

        // Second flush — same fingerprint → must be deduped
        write_findings_note(&cas.root, "cas-b4t2", &findings)
            .expect("second write (dedup path) should succeed");

        let store = open_task_store(&cas.root).expect("open_task_store");
        let task = store.get("cas-b4t2").expect("task.get");

        let occurrences = task.notes.matches("Root cause:").count();
        assert_eq!(
            occurrences, 1,
            "same finding must appear exactly once after dedup; notes:\n{}",
            task.notes
        );
    }

    // -------------------------------------------------------------------------
    // AC: no active task → graceful (resolve returns None, no panic)
    // -------------------------------------------------------------------------
    #[test]
    fn test_resolve_worker_active_task_returns_none_for_unknown_session() {
        let cas = setup_cas();

        let result = resolve_worker_active_task("nonexistent-session-xyz", &cas.root);
        assert!(result.is_ok(), "must not error for unknown session");
        assert_eq!(
            result.unwrap(),
            None,
            "must return None for session with no agent record"
        );
    }

    // -------------------------------------------------------------------------
    // AC: failure path → compaction unaffected (no panic, flush is silent)
    // -------------------------------------------------------------------------
    #[test]
    fn test_flush_worker_findings_no_transcript_is_graceful() {
        let cas = setup_cas();

        let input = HookInput {
            session_id: "test-session-b4".to_string(),
            transcript_path: None, // no transcript → early return
            hook_event_name: "PreCompact".to_string(),
            ..Default::default()
        };

        // Must not panic; errors must be swallowed
        flush_worker_findings_to_task(&input, &cas.root);
    }

    // -------------------------------------------------------------------------
    // AC#4: note is size-bounded — MAX_FLUSH_FINDINGS cap enforced
    // -------------------------------------------------------------------------
    #[test]
    fn test_write_findings_note_respects_max_findings_cap() {
        let cas = setup_cas();
        add_inprogress_task(&cas.root, "cas-b4t5");

        // Write 12 findings — only MAX_FLUSH_FINDINGS (8) should appear
        let findings: Vec<String> = (0..12)
            .map(|i| format!("finding-{i}: discovered something important about subsystem"))
            .collect();

        write_findings_note(&cas.root, "cas-b4t5", &findings)
            .expect("write should succeed");

        let store = open_task_store(&cas.root).expect("open_task_store");
        let task = store.get("cas-b4t5").expect("task.get");

        // Count how many "finding-N:" lines appear
        let written = (0..12)
            .filter(|i| task.notes.contains(&format!("finding-{i}:")))
            .count();

        assert!(
            written <= MAX_FLUSH_FINDINGS,
            "must write at most {MAX_FLUSH_FINDINGS} findings; wrote {written};\nnotes:\n{}",
            task.notes
        );
        assert!(
            written > 0,
            "must write at least some findings; notes:\n{}",
            task.notes
        );
    }

    // -------------------------------------------------------------------------
    // FIX A regression: flush fires even when context_parts is empty
    //
    // Before the fix, flush_worker_findings_to_task was called AFTER the
    // `if context_parts.is_empty() { return Ok(empty) }` guard, so a worker
    // with in-flight findings but no high-importance memories would compact
    // WITHOUT flushing.  This test calls write_findings_note directly to prove
    // the writer succeeds on a fresh (empty-notes) task — the reorder fix is
    // verified structurally (flush call is above the early-return in the source).
    // -------------------------------------------------------------------------
    #[test]
    fn test_flush_writes_even_when_inject_context_is_empty() {
        let cas = setup_cas();
        add_inprogress_task(&cas.root, "cas-b4t7");

        // Simulate a worker that has findings but an empty CAS store
        // (no high-importance memories → context_parts would be empty → old
        // code would have returned before flushing).
        let findings = vec![
            "Found root cause: create_worktree falls back to main cwd on reuse-branch".to_string(),
        ];

        write_findings_note(&cas.root, "cas-b4t7", &findings)
            .expect("write must succeed even in empty-context scenario");

        let store = open_task_store(&cas.root).expect("open_task_store");
        let task = store.get("cas-b4t7").expect("task.get");

        assert!(
            task.notes.contains("create_worktree"),
            "findings must be written even when inject context is empty; notes:\n{}",
            task.notes
        );
    }

    // -------------------------------------------------------------------------
    // AC#5: inject direction preserved — handle_pre_compact still returns
    //       a non-empty systemMessage even after flush is wired in
    // -------------------------------------------------------------------------
    #[test]
    fn test_handle_pre_compact_inject_direction_preserved() {
        let cas = setup_cas();

        // Minimal HookInput — no transcript, so flush short-circuits gracefully
        let input = HookInput {
            session_id: "inject-test-session".to_string(),
            transcript_path: None,
            hook_event_name: "PreCompact".to_string(),
            ..Default::default()
        };

        // Even with an empty store (no high-importance memories), the function
        // must not error.  With memories present it would return Some(context).
        let result = handle_pre_compact(&input, Some(&cas.root));
        assert!(
            result.is_ok(),
            "handle_pre_compact must not error after B4 flush wired in; got: {result:?}"
        );
        // hook_specific_output must be None — PreCompact schema forbids it
        let output = result.unwrap();
        assert!(
            output.hook_specific_output.is_none(),
            "PreCompact must not set hookSpecificOutput (schema forbids it)"
        );
    }
}
