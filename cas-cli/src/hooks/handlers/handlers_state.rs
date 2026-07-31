use crate::hooks::handlers::*;

pub(crate) fn detect_significant_activity(
    tool_name: &str,
    input: &HookInput,
) -> Option<(String, String)> {
    let tool_input = input.tool_input.as_ref()?;

    match tool_name {
        "Edit" | "Write" => {
            let path = tool_input.get("file_path")?.as_str()?;
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            Some((
                "worker_file_edited".to_string(),
                format!("Edited {filename}"),
            ))
        }
        "Bash" => {
            let cmd = tool_input.get("command")?.as_str()?;
            if cmd.contains("git commit") {
                Some((
                    "worker_git_commit".to_string(),
                    "Committed changes".to_string(),
                ))
            } else {
                None // Skip other bash commands
            }
        }
        "Task" => {
            let subagent = tool_input.get("subagent_type")?.as_str()?;
            Some((
                "worker_subagent_spawned".to_string(),
                format!("Running {subagent}"),
            ))
        }
        _ => None,
    }
}

/// Extract entity ID for activity tracking
#[allow(dead_code)]
pub(crate) fn extract_activity_entity_id(tool_name: &str, input: &HookInput) -> Option<String> {
    let tool_input = input.tool_input.as_ref()?;

    match tool_name {
        "Edit" | "Write" => tool_input.get("file_path")?.as_str().map(String::from),
        "Task" => tool_input.get("subagent_type")?.as_str().map(String::from),
        _ => None,
    }
}

/// Track a file access for session-aware context boosting
///
/// Records files being worked on so they can influence context selection.
/// Uses a simple JSON file in the CAS directory.
pub(crate) fn track_session_file(cas_root: &std::path::Path, file_path: &str) {
    let session_files_path = cas_root.join("session_files.json");

    // Read existing files
    let mut files: Vec<String> = std::fs::read_to_string(&session_files_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // Add new file if not already present
    if !files.contains(&file_path.to_string()) {
        files.insert(0, file_path.to_string());
        // Keep only recent files
        files.truncate(MAX_RECENT_FILES);

        // Write back
        let _ = std::fs::write(
            &session_files_path,
            serde_json::to_string(&files).unwrap_or_default(),
        );
    }
}

/// Read recent files being worked on in this session
pub fn get_session_files(cas_root: &std::path::Path) -> Vec<String> {
    let session_files_path = cas_root.join("session_files.json");
    std::fs::read_to_string(&session_files_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Determine the current agent ID in hook context.
///
/// Prefer the session_id (canonical agent ID) and fall back to PPID-based
/// computation when the session_id is missing.
pub(crate) fn current_agent_id(input: &HookInput) -> String {
    // Codex's native hook `session_id` is its thread UUID, not CAS's
    // registered factory-agent ID. Factory hook processes inherit the
    // canonical CAS session ID from the worker environment.
    if std::env::var("CAS_FACTORY_MODE").as_deref() == Ok("1") {
        if let Ok(session_id) = std::env::var("CAS_SESSION_ID") {
            if !session_id.is_empty() {
                return session_id;
            }
        }
    }
    if !input.session_id.is_empty() {
        input.session_id.clone()
    } else {
        crate::agent_id::compute_agent_id_for_hook()
    }
}

/// Clear session files (called on session end)
pub(crate) fn clear_session_files(cas_root: &std::path::Path) {
    let session_files_path = cas_root.join("session_files.json");
    let _ = std::fs::remove_file(&session_files_path);
}

/// Add an interruption note to a task (instead of resetting status)
///
/// Preserves the InProgress status but adds a system note indicating the work was interrupted.
/// This allows the next agent to see that work was attempted and decide whether to resume or reset.
pub(crate) fn add_interruption_note(task: &mut crate::types::Task, agent_id: &str) {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let note = format!(
        "[{}] ⚠️ INTERRUPTED Agent {} stopped/timed out while task was in progress",
        timestamp,
        &agent_id[..12.min(agent_id.len())]
    );

    if task.notes.is_empty() {
        task.notes = note;
    } else {
        task.notes = format!("{}\n\n{}", task.notes, note);
    }
    task.updated_at = chrono::Utc::now();
}

/// Clean up agent leases for a session
///
/// Called during Stop/SubagentStop/SessionEnd to:
/// 1. Release all leases held by the agent
/// 2. Clear working epics tracking
/// 3. Add interruption note to tasks that were in progress (preserves status)
///
/// Note: Agents are registered with their session_id, so we use session_id
/// as the agent_id for lookup and cleanup.
/// PID → session mapping is handled by the daemon via socket events.
pub(crate) fn cleanup_agent_leases(
    cas_root: &std::path::Path,
    session_id: &str,
) -> Option<Vec<String>> {
    let agent_store = open_agent_store(cas_root).ok()?;

    // Use session_id as agent_id (agents are registered with their session_id)
    let agent_id = session_id;

    let agent = match agent_store.get(agent_id) {
        Ok(a) => a,
        Err(_) => {
            return Some(Vec::new());
        }
    };

    // Gracefully shutdown the agent and get list of released task IDs
    let released_task_ids = agent_store.graceful_shutdown(&agent.id).unwrap_or_default();

    // Clear working epics for this agent (session is ending)
    let _ = agent_store.clear_working_epics(&agent.id);

    // Unregister the agent (delete from database)
    let _ = agent_store.unregister(&agent.id);

    if released_task_ids.is_empty() {
        return Some(released_task_ids);
    }

    // Add interruption note to tasks that were in progress (don't reset status)
    if let Ok(task_store) = open_task_store(cas_root) {
        for task_id in &released_task_ids {
            if let Ok(mut task) = task_store.get(task_id) {
                // Only add note if task was in progress
                if task.status == TaskStatus::InProgress {
                    add_interruption_note(&mut task, &agent.id);
                    let _ = task_store.update(&task);
                }
            }
        }
    }

    eprintln!("cas: Released {} task lease(s)", released_task_ids.len());

    Some(released_task_ids)
}

/// Cleanup orphaned tasks on session start
///
/// Finds in_progress tasks that have no active lease (from crashed/interrupted sessions)
/// and resets them to Open status so they can be worked on again.
///
/// cas-85d9: this is the most consequential consumer of `list_active_leases`
/// found while auditing the "task leases are never renewed" gap (cas-d165).
/// Before task leases renewed on heartbeat, ANY task held past its ~30min
/// claim duration would have "no active lease" here even though its worker
/// was alive, heartbeating, and actively working — and this function would
/// silently reopen it to `Open` on the very next `SessionStart` of ANY
/// agent in the factory (not just the task's own worker), letting a second
/// worker pick up and duplicate work the first was still doing. Task-lease
/// heartbeat renewal (`agent_heartbeat` in
/// `crates/cas-store/src/agent_store/ops_agent.rs`) is what keeps this
/// function's "no active lease ⇒ orphaned" assumption true in practice now.
pub(crate) fn cleanup_orphaned_tasks(cas_root: &std::path::Path) -> usize {
    let task_store = match open_task_store(cas_root) {
        Ok(store) => store,
        Err(_) => return 0,
    };

    let agent_store = match open_agent_store(cas_root) {
        Ok(store) => store,
        Err(_) => return 0,
    };

    // Get all in_progress tasks
    let in_progress = match task_store.list(Some(TaskStatus::InProgress)) {
        Ok(tasks) => tasks,
        Err(_) => return 0,
    };

    if in_progress.is_empty() {
        return 0;
    }

    // Get all active leases
    let active_leases = agent_store.list_active_leases().unwrap_or_default();
    let claimed_task_ids: std::collections::HashSet<_> =
        active_leases.iter().map(|l| l.task_id.as_str()).collect();

    // Find orphaned tasks (in_progress but no active lease)
    let mut reopened = 0;
    for task in in_progress {
        if !claimed_task_ids.contains(task.id.as_str()) {
            // Reopen the task by setting status back to Open
            if let Ok(mut t) = task_store.get(&task.id) {
                t.status = TaskStatus::Open;
                t.updated_at = chrono::Utc::now();
                if task_store.update(&t).is_ok() {
                    reopened += 1;
                }
            }
        }
    }

    reopened
}

/// Exit blockers preventing agent from stopping
#[derive(Debug, Default)]
pub struct ExitBlockers {
    /// Active child agents that must complete first
    pub active_children: Vec<Agent>,
    /// Tasks with active lease that must be closed
    pub claimed_tasks: Vec<Task>,
    /// Subtasks of claimed epics that must be closed
    pub epic_subtasks: Vec<Task>,
}

impl ExitBlockers {
    /// Check if there are any blockers preventing exit
    pub fn has_blockers(&self) -> bool {
        !self.active_children.is_empty()
            || !self.claimed_tasks.is_empty()
            || !self.epic_subtasks.is_empty()
    }

    /// Format a message describing the blockers
    pub fn format_message(&self) -> String {
        let mut lines = vec!["⚠️ Cannot exit - you have remaining work:".to_string()];

        if !self.active_children.is_empty() {
            lines.push(String::new());
            lines.push("Active Child Agents:".to_string());
            for agent in &self.active_children {
                let claimed_info = if agent.active_tasks > 0 {
                    format!(" ({} tasks)", agent.active_tasks)
                } else {
                    String::new()
                };
                lines.push(format!(
                    "  🤖 [{}] {}{}",
                    &agent.id[..8.min(agent.id.len())],
                    agent.name,
                    claimed_info
                ));
            }
        }

        if !self.claimed_tasks.is_empty() {
            lines.push(String::new());
            lines.push("Claimed Tasks:".to_string());
            for task in &self.claimed_tasks {
                let type_str = if task.task_type == TaskType::Epic {
                    " (epic)"
                } else {
                    ""
                };
                lines.push(format!("  ○ [{}] {}{}", task.id, task.title, type_str));
            }
        }

        if !self.epic_subtasks.is_empty() {
            lines.push(String::new());
            lines.push("Epic Subtasks:".to_string());
            for task in &self.epic_subtasks {
                lines.push(format!(
                    "  ○ [{}] {} [{}]",
                    task.id, task.title, task.status
                ));
            }
        }

        lines.push(String::new());
        if !self.active_children.is_empty() {
            lines.push(
                "Wait for child agents to complete, then finish remaining tasks.".to_string(),
            );
        } else {
            // Count tasks by status
            let open_count = self
                .claimed_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Open)
                .count()
                + self
                    .epic_subtasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Open)
                    .count();
            let in_progress_count = self
                .claimed_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::InProgress)
                .count()
                + self
                    .epic_subtasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .count();

            lines.push("To exit, complete all remaining tasks:".to_string());

            if open_count > 0 {
                // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — reminder
                // text describing what THIS agent should call.
                let prefix = crate::harness_policy::own_tool_prefix();
                lines.push(format!("  {open_count} open task(s): Start with {prefix}task action: start, implement, then close"));
            }
            if in_progress_count > 0 {
                lines.push(format!("  {in_progress_count} in_progress task(s): Verify before close (spawn 'task-verifier' directly, or ask supervisor if workers are Codex)"));
            }
        }

        lines.join("\n")
    }
}

/// Check for blockers that would prevent agent from exiting
///
/// Returns exit blockers if there are open tasks/children that must be handled first.
///
/// Exit blocker logic:
/// 1. Check for active child agents (subagents still running)
/// 2. Check for claimed tasks that aren't closed (active leases)
/// 3. Check working_epics for open subtasks (epics the agent is working on)
///
/// Note: `session_id` is the canonical agent ID; PPID-based ID is a fallback
/// when the session ID is missing.
pub(crate) fn get_exit_blockers(
    cas_root: &std::path::Path,
    session_id: &str,
) -> Result<ExitBlockers, MemError> {
    let agent_store = open_agent_store(cas_root)?;
    let task_store = open_task_store(cas_root)?;

    // Prefer session_id as canonical agent ID; fall back to PPID-based ID if missing.
    let agent_id = if !session_id.is_empty() {
        session_id.to_string()
    } else {
        crate::agent_id::compute_agent_id_for_hook()
    };
    let agent = agent_store.get(&agent_id).ok();

    let mut blockers = ExitBlockers::default();
    let mut epic_ids = std::collections::HashSet::new();

    if let Some(ref agent) = agent {
        // 1. Check for active child agents
        blockers
            .active_children
            .extend(agent_store.get_active_children(&agent.id)?);

        // 2. Get claimed tasks (active leases)
        if let Ok(leases) = agent_store.list_agent_leases(&agent.id) {
            for lease in &leases {
                if let Ok(task) = task_store.get(&lease.task_id) {
                    // Only include open tasks as blockers
                    if task.status != TaskStatus::Closed {
                        // Track epics for subtask check (directly claimed epics)
                        if task.task_type == TaskType::Epic {
                            epic_ids.insert(task.id.clone());
                        }

                        blockers.claimed_tasks.push(task);
                    }
                }
            }
        }

        // 3. Get working_epics - epics the agent is actively working on
        if let Ok(working_epics) = agent_store.get_working_epics(&agent.id) {
            for epic_id in working_epics {
                epic_ids.insert(epic_id);
            }
        }
    }

    // 4. NOTE: We only check working_epics for THIS agent (session_id).
    // The MCP server now fails early if no session ID exists, so the agent ID
    // used for working_epics will always match the session_id in Stop hook.
    // No need to check other agents' working_epics.

    // 5. Get subtasks of all relevant epics
    let claimed_ids: std::collections::HashSet<_> = blockers
        .claimed_tasks
        .iter()
        .map(|t| t.id.as_str())
        .collect();

    for epic_id in &epic_ids {
        // Skip epics that are already closed
        if let Ok(epic) = task_store.get(epic_id) {
            if epic.status == TaskStatus::Closed {
                // Clean up stale working_epics entry
                if let Some(ref agent) = agent {
                    let _ = agent_store.remove_working_epic(&agent.id, epic_id);
                }
                continue;
            }
        }

        if let Ok(subtasks) = task_store.get_subtasks(epic_id) {
            for subtask in subtasks {
                // Include all non-closed subtasks - agent must complete the entire epic
                if subtask.status != TaskStatus::Closed
                    && !claimed_ids.contains(subtask.id.as_str())
                {
                    blockers.epic_subtasks.push(subtask);
                }
            }
        }
    }

    Ok(blockers)
}

/// Handle SubagentStop hook - minimal cleanup for subagent completion
///
/// Called when a Claude Code subagent (Task tool call) finishes.
///
/// IMPORTANT: The session_id in SubagentStop is the PARENT's session_id, not the
/// subagent's. We do NOT have the subagent's agent ID, and subagents spawned via
/// Task tool may not even be registered as CAS agents. Therefore, we do NOT
/// perform any agent cleanup here - that would incorrectly shut down the parent!
///
/// Only the parent's Stop hook should clean up agents and PID mappings.
pub fn handle_subagent_stop(
    _input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    if cas_root.is_none() {
        return Ok(HookOutput::empty());
    }

    // NOTE: Do NOT call cleanup_subagent_leases or any agent cleanup here!
    // The session_id is the parent's, not the subagent's.

    Ok(HookOutput::empty())
}

/// Remove only the exact still-unbound verifier handoff for a failed, denied,
/// or completed-without-SubagentStart Agent tool call.
///
/// The hook-local tool_use_id is hashed inside the store. Bound and consumed
/// audit rows are never eligible for cleanup.
pub fn handle_verifier_spawn_cleanup(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let Some(cas_root) = cas_root else {
        return Ok(HookOutput::empty());
    };
    if !matches!(input.tool_name.as_deref(), Some("Task" | "Agent"))
        || input
            .tool_input
            .as_ref()
            .and_then(|value| value.get("subagent_type"))
            .and_then(|value| value.as_str())
            != Some("task-verifier")
    {
        return Ok(HookOutput::empty());
    }
    let Some(tool_use_id) = input
        .tool_use_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(HookOutput::empty());
    };
    let parent_id = current_agent_id(input);
    let _ = cas_store::cancel_unbound_server_verifier_handoff(cas_root, &parent_id, tool_use_id);
    Ok(HookOutput::empty())
}

/// Handle SubagentStart hook - bind task-verifier authority.
///
/// Called when a Claude Code subagent (Task tool call) is about to start.
/// A verifier spawn claims only the named task's durable dispatch. It never
/// clears `pending_verification`; only a legitimate verdict may resolve that
/// exact task transition.
pub fn handle_subagent_start(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Official SubagentStart carries the parent session plus distinct child
    // agent_id/agent_type, but no Agent prompt or PreToolUse tool_use_id. Bind
    // only the sole durable sealed handoff for this exact registered parent.
    if input.agent_type.as_deref() != Some("task-verifier") {
        return Ok(HookOutput::empty());
    }

    let parent_id = current_agent_id(input);
    let child_id = match input
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != parent_id)
    {
        Some(child_id) => child_id,
        None => {
            return Ok(HookOutput::with_system_context(
                "CAS task-verifier authority binding failed: SubagentStart did not provide a distinct official child agent_id."
                    .to_string(),
            ));
        }
    };
    let agent_store = match open_agent_store(cas_root) {
        Ok(store) => store,
        Err(_) => {
            return Ok(HookOutput::with_system_context(
                "CAS agent registry is unavailable; verification will fail closed.".to_string(),
            ));
        }
    };
    let issuer = match agent_store.get(&parent_id) {
        Ok(agent)
            if matches!(
                agent.status,
                crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle
            ) =>
        {
            agent
        }
        _ => {
            return Ok(HookOutput::with_system_context(
                "CAS verifier issuer is anonymous, orphaned, or inactive; verification will fail closed."
                    .to_string(),
            ));
        }
    };
    if let Ok(existing) = agent_store.get(child_id)
        && (existing.agent_type != crate::types::AgentType::SubAgent
            || existing.role != AgentRole::Standard
            || existing.parent_id.as_deref() != Some(parent_id.as_str()))
    {
        return Ok(HookOutput::with_system_context(
            "CAS verifier child identity conflicts with an existing registered session; verification will fail closed."
                .to_string(),
        ));
    }

    let capability = match cas_store::bind_server_verifier_handoff(cas_root, &parent_id, child_id) {
        Ok(capability) => capability,
        Err(_) => {
            return Ok(HookOutput::with_system_context(
                    "CAS task-verifier handoff is missing, ambiguous, expired, or not bound to an active exact dispatch; verification will fail closed."
                        .to_string(),
                ));
        }
    };
    if capability.issuer_agent_id != parent_id {
        return Ok(HookOutput::with_system_context(
            "CAS task-verifier handoff parent binding is invalid; verification will fail closed."
                .to_string(),
        ));
    }

    let existing = agent_store.get(child_id).ok();
    let mut child = existing.clone().unwrap_or_else(|| {
        Agent::new_sub_agent(
            child_id.to_string(),
            "task-verifier".to_string(),
            parent_id.clone(),
        )
    });
    child.name = "task-verifier".to_string();
    child.agent_type = crate::types::AgentType::SubAgent;
    child.role = AgentRole::Standard;
    child.parent_id = Some(parent_id);
    child.factory_session = issuer.factory_session.clone();
    child.status = crate::types::AgentStatus::Active;
    child.last_heartbeat = chrono::Utc::now();
    let registry_result = if existing.is_some() {
        agent_store.update(&child)
    } else {
        agent_store.register(&child)
    };
    if registry_result.is_err() {
        return Ok(HookOutput::with_system_context(
            "CAS could not register the verifier child; verification will fail closed.".to_string(),
        ));
    }

    Ok(HookOutput::empty())
}

#[cfg(test)]
mod cas_85d9_lease_renewal_tests {
    use super::*;
    use crate::store::init_cas_dir;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct CasDir {
        _tmp: TempDir,
        pub root: PathBuf,
    }

    fn setup_cas() -> CasDir {
        let tmp = tempfile::tempdir().expect("TempDir");
        let root = init_cas_dir(tmp.path()).expect("init_cas_dir");
        CasDir { _tmp: tmp, root }
    }

    /// cas-85d9 AC3: a task held by a live, heartbeating worker past the
    /// lease duration must still be observable as in-progress — proven at
    /// the level that actually bit us (docs/requests wave-2 report):
    /// `cleanup_orphaned_tasks`, which used to treat "no active lease" as
    /// unconditional proof of a crashed session and silently reopen the
    /// task to `Open`, corrupting ownership for a worker that was still
    /// actively working it.
    #[test]
    fn heartbeating_worker_past_original_lease_duration_is_not_reopened() {
        let cas = setup_cas();
        let agent_store = open_agent_store(&cas.root).expect("open_agent_store");
        let task_store = open_task_store(&cas.root).expect("open_task_store");

        let agent = Agent::new("agent-85d9".to_string(), "Lease Renewal Test".to_string());
        agent_store.register(&agent).expect("register");

        let mut task = Task::new("cas-85d9-t1".to_string(), "Long-running task".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("agent-85d9".to_string());
        task_store.add(&task).expect("task.add");

        // Claim with a very short duration — simulates a task claimed near
        // (or past) the default ~30min window.
        agent_store
            .try_claim("cas-85d9-t1", "agent-85d9", 1, None)
            .expect("try_claim");

        // The worker heartbeats normally while still working — this is
        // what production does every ~5-30s via the daemon tick.
        agent_store.heartbeat("agent-85d9").expect("heartbeat");

        // Wait past the ORIGINAL 1s claim duration. Before cas-85d9, the
        // lease would now be expired and `cleanup_orphaned_tasks` would
        // reopen the task on the next SessionStart despite the worker
        // being alive and having just heartbeated.
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Model the same production reclaim-sweep timing as the sibling
        // no-heartbeat test below — must be a no-op here since the
        // heartbeat already pushed `expires_at` well into the future.
        agent_store
            .reclaim_expired_leases()
            .expect("reclaim_expired_leases");

        let reopened = cleanup_orphaned_tasks(&cas.root);
        assert_eq!(
            reopened, 0,
            "a heartbeat-renewed lease must not be treated as orphaned"
        );

        let after = task_store.get("cas-85d9-t1").expect("task.get");
        assert_eq!(
            after.status,
            TaskStatus::InProgress,
            "task held by a live, heartbeating worker past the original lease \
             duration must remain observable as in-progress, not silently reopened"
        );
    }

    /// Safety property: renewal only happens ON heartbeat. An agent that
    /// stops heartbeating (crashes, hangs before ever heartbeating again)
    /// must still let its lease expire and the orphaned task recover
    /// normally — auto-renewal must not create an unrecoverable lease.
    #[test]
    fn non_heartbeating_worker_past_lease_duration_is_still_reopened() {
        let cas = setup_cas();
        let agent_store = open_agent_store(&cas.root).expect("open_agent_store");
        let task_store = open_task_store(&cas.root).expect("open_task_store");

        let agent = Agent::new(
            "agent-85d9-dead".to_string(),
            "No Heartbeat Test".to_string(),
        );
        agent_store.register(&agent).expect("register");

        let mut task = Task::new("cas-85d9-t2".to_string(), "Abandoned task".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("agent-85d9-dead".to_string());
        task_store.add(&task).expect("task.add");

        agent_store
            .try_claim("cas-85d9-t2", "agent-85d9-dead", 1, None)
            .expect("try_claim");

        // No heartbeat call — the agent is presumed crashed/hung before
        // ever heartbeating on this lease.
        std::thread::sleep(std::time::Duration::from_secs(2));

        // `list_active_leases` (which `cleanup_orphaned_tasks` consults)
        // returns any row still `status='active'` regardless of whether
        // `expires_at` has passed — the flip to `status='expired'` only
        // happens via an explicit `reclaim_expired_leases()` sweep. In
        // production this runs continuously (daemon maintenance tick,
        // every `worker_status` poll, etc.), so an overdue lease is
        // reclaimed within moments; a standalone test has to trigger that
        // sweep explicitly to model the same production timing.
        agent_store
            .reclaim_expired_leases()
            .expect("reclaim_expired_leases");

        let reopened = cleanup_orphaned_tasks(&cas.root);
        assert_eq!(
            reopened, 1,
            "a lease with no heartbeat renewal must still expire and recover normally"
        );

        let after = task_store.get("cas-85d9-t2").expect("task.get");
        assert_eq!(
            after.status,
            TaskStatus::Open,
            "an orphaned task with no renewing heartbeat must still be recoverable"
        );
    }
}
