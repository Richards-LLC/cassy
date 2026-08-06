use crate::hooks::handlers::*;

// PreToolUse Hook Handler
// ============================================================================

/// Handle PreToolUse hook - rule-based auto-approval
///
/// This hook fires BEFORE a tool is executed and can:
/// 1. Auto-approve safe tools based on proven rules with path matching
/// 2. Block or warn for protected files/directories
/// 3. Modify tool parameters via updatedInput
///
/// Returns permission_decision: "allow" | "deny" | null (ask user)
pub fn handle_pre_tool_use(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let tool_name = match &input.tool_name {
        Some(name) => name.as_str(),
        None => return Ok(HookOutput::empty()),
    };

    let is_factory_agent = crate::harness_policy::is_factory_agent(input);

    // ========================================================================
    // SUPERVISOR DISCIPLINE: Block Agent(isolation="worktree") for supervisors
    //
    // Supervisors must spawn workers via `mcp__cas__coordination spawn_workers`
    // so worktrees are factory-tracked and garbage-collected. Raw `Agent` calls
    // with `isolation: "worktree"` create worktrees Claude Code cleans up only
    // on process exit — which leaks across Petrastella repos when the session
    // is long-lived (see EPIC cas-7c88 / project_factory_worktree_leak).
    //
    // Non-isolation Agent calls (Explore, code-review personas, task-verifier)
    // stay allowed — they're load-bearing for correctness verification.
    //
    // Placed before the cas_root check so the gate fires even if CAS isn't
    // initialized in the supervisor's cwd (belt-and-suspenders; should never
    // happen in factory mode).
    // ========================================================================
    if tool_name == "Agent" && crate::harness_policy::is_supervisor(input) {
        let tool_input = input.tool_input.as_ref();
        let isolation = tool_input.and_then(|ti| ti.get("isolation").and_then(|v| v.as_str()));
        let subagent_type =
            tool_input.and_then(|ti| ti.get("subagent_type").and_then(|v| v.as_str()));
        // Task-verifier is exempt: supervisors legitimately spawn it to resolve
        // a task-scoped verification dispatch.
        let is_verifier_exempt = subagent_type == Some("task-verifier");
        if isolation == Some("worktree") && !is_verifier_exempt {
            // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — this reminder
            // tells the supervisor what IT can call, so it needs the
            // supervisor's own tool prefix, not a hardcoded mcp__cas__.
            let prefix = crate::harness_policy::own_tool_prefix();
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &format!(
                    "🚫 Supervisors must not spawn isolated-worktree subagents.\n\
                    Use {prefix}coordination action=spawn_workers — factory-managed worktrees get cleaned up; Agent(isolation=\"worktree\") ones leak.\n\
                    If you genuinely need a throwaway subagent, drop `isolation` or run as a worker via `cas factory`."
                ),
            ));
        }
    }

    // ========================================================================
    // FACTORY MODE: Block AskUserQuestion self-directed permission trap
    //
    // In factory mode, AskUserQuestion has no human UI surface. It appears as
    // a permission prompt in the caller's own session and pauses the system, so
    // both supervisors and workers must use factory-safe routes instead.
    //
    // This gate runs before the cas_root check because it only needs the
    // factory role env/snapshot and must still fire when hook dispatch cannot
    // resolve a CAS root.
    // ========================================================================
    if is_factory_agent && tool_name == "AskUserQuestion" {
        let prefix = crate::harness_policy::own_tool_prefix();
        let guidance = if crate::harness_policy::is_supervisor(input) {
            format!(
                "AskUserQuestion cannot reach the human in factory mode - it surfaces as a permission prompt on your own session and pauses the system. \
                Ask the human in plain text in your reply and END YOUR TURN; the director relays their answer. \
                For workers/teammates use {prefix}coordination action=message."
            )
        } else {
            format!(
                "AskUserQuestion is blocked in factory mode - it surfaces as a permission prompt on your own session and pauses the system. \
                Message your supervisor with {prefix}coordination action=message target=<supervisor> summary=\"...\" message=\"...\"."
            )
        };
        return Ok(HookOutput::with_pre_tool_permission("deny", &guidance));
    }

    // ========================================================================
    // REVIEW DISPATCH GATE — every harness entry path (cas-bcfb, GH #125)
    //
    // cas-4fef put the ownership gate on `cas_skill_use`, i.e. only on
    // `mcp__cas__skill action=use id=cas-code-review`. No agent takes that
    // path: the skill is installed on disk as a Claude Code skill and is
    // invoked with the native `Skill` tool, and the Phase C workflow
    // (`.claude/workflows/cas-code-review.js`) is callable straight from the
    // `Workflow` tool. Both bypass the MCP server entirely, which is how a
    // worker ran the full persona fan-out under `owner = "supervisor"` on a
    // binary that provably contained the gate (GH #125: same refusal string
    // compiled into both binary generations alive that day).
    //
    // PreToolUse is the only seam CAS owns for harness-native tool calls, so
    // the gate is re-stated here — same pure `review_dispatch_decision`, same
    // refusal text, so the MCP site and this one cannot drift. The matcher
    // lists that make this hook fire for `Skill`/`Workflow` live in
    // `ui/factory/daemon/runtime/teams.rs` (factory settings) and
    // `config/hooks.rs` (generated settings); both must keep these tools.
    //
    // Hoisted above the cas_root check for the same reason the
    // AskUserQuestion gate is: role comes from the hook's own snapshot, and a
    // hook dispatch that cannot resolve a CAS root must still refuse (with
    // cas_root=None `supervisor_owned_at` returns the cas-865b default).
    // ========================================================================
    if crate::harness_policy::is_worker(input)
        && crate::code_review_dispatch::tool_call_enters_review(
            tool_name,
            input.tool_input.as_ref(),
        )
    {
        if let crate::code_review_dispatch::ReviewDispatchDecision::Refused { message } =
            crate::code_review_dispatch::review_dispatch_decision(
                true,
                crate::code_review_dispatch::supervisor_owned_at(cas_root),
            )
        {
            return Ok(HookOutput::with_pre_tool_permission("deny", &message));
        }
    }

    // ========================================================================
    // WORKER COMMIT GUARD — HOISTED ABOVE cas_root check (cas-bea2, LAYER 1)
    //
    // Must run before the hoisted FACTORY_AUTO_APPROVE block below. That
    // block returns "allow" for all Bash tool calls when cas_root=None,
    // which would bypass this guard. Placing it here ensures it fires on
    // both the cas_root=None and cas_root=Some paths.
    //
    // Intercepts `git commit` / `git merge` from ALL factory workers
    // (CAS_AGENT_ROLE=worker && CAS_FACTORY_MODE set), whether or not
    // they have an isolated worktree (CAS_CLONE_PATH). Non-factory roles
    // fall through silently. This prevents standalone-task workers that
    // lack a CAS_CLONE_PATH from committing directly to main/master/staging
    // in the shared primary checkout (cas-ba04).
    // ========================================================================
    {
        let is_factory_worker_guard = std::env::var("CAS_AGENT_ROLE")
            .map(|r| r.eq_ignore_ascii_case("worker"))
            .unwrap_or(false)
            && std::env::var("CAS_FACTORY_MODE").is_ok();
        if is_factory_worker_guard && tool_name == "Bash" {
            let command = input
                .tool_input
                .as_ref()
                .and_then(|ti| ti.get("command").and_then(|v| v.as_str()));
            if let Some(cmd) = command {
                if looks_like_git_write_op(cmd) {
                    if let Some(deny_msg) = check_worker_git_commit_scope(&input.cwd) {
                        return Ok(HookOutput::with_pre_tool_permission("deny", &deny_msg));
                    }
                }
            }
        }
    }

    // ========================================================================
    // FACTORY AUTO-APPROVE — HOISTED ABOVE cas_root check (cas-7f33)
    //
    // The factory filesystem auto-approve also runs below, AFTER all
    // protection gates, for the cas_root=Some case. But that path is
    // unreachable when `cas_root` is `None` because of the early return
    // immediately following this block. Since `is_factory_agent` derives
    // from the hook's role snapshot with an env fallback (no store access
    // required), we fire the allow here to rescue the cas_root=None case — the
    // scenario the user hit in the BUG-factory-write-permission-deadlock
    // report where a supervisor session runs the hook without a CAS root
    // resolved at dispatch time.
    //
    // Invariant preservation: protection gates (.env deny, credential
    // patterns) live inside the `cas_root=Some` section below. When
    // `cas_root` is `None` those gates cannot run anyway (they read
    // config via `stores.config()`), so hoisting here does not widen the
    // surface on any path where the guard previously applied. When
    // `cas_root` is `Some`, this block is a no-op — we fall through to
    // the normal flow where the post-protection auto-approve still fires.
    // ========================================================================
    if cas_root.is_none() && is_factory_agent && FACTORY_AUTO_APPROVE_TOOLS.contains(&tool_name) {
        return Ok(HookOutput::with_pre_tool_permission(
            "allow",
            &format!(
                "Factory agent auto-approve ({tool_name}) — bypasses Claude Code team-mode leader-escalation deadlock (UG9 bug); cas_root=None path"
            ),
        ));
    }

    // Check if CAS is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Create shared store cache — all store accesses below go through this
    // instead of calling open_*() directly, reducing ~11 SQLite connections to ~3-4.
    let mut stores = ToolHookStores::new(cas_root);

    // Compute current agent's task IDs (via leases) once for all jail checks.
    // This prevents cross-agent jail contamination where Agent A's pending tasks
    // block Agent B in a different session.
    let current_agent_id = current_agent_id(input);
    let agent_task_ids: std::collections::HashSet<String> = stores
        .agents()
        .and_then(|store| store.list_agent_leases(&current_agent_id).ok())
        .map(|leases| leases.into_iter().map(|l| l.task_id).collect())
        .unwrap_or_default();

    // ========================================================================
    // FACTORY MODE: Auto-route SendMessage → CAS coordination (cas-f32b)
    //
    // In factory mode, agents communicate through CAS coordination (push-based
    // via the Director/TUI). The built-in SendMessage tool bypasses this system
    // and would cause messages to be lost.
    //
    // Claude Code's Team Coordination system-reminder tells agents to use
    // `SendMessage`, so agents default to it. Previously this hook just
    // denied-with-guidance, but agents frequently spammed retries before
    // switching tools — effectively wedging workers on the deny loop
    // (observed 2026-04-23 in gabber-studio).
    //
    // New behaviour: parse the SendMessage call, enqueue the message on the
    // CAS prompt queue directly (same path `mcp__cas__coordination
    // action=message` uses), notify the daemon, then return `allow` with an
    // `additionalContext` success receipt (cas-73c8) so agents see tool
    // success — not a deny/`<error>` envelope — and stop retrying.
    //
    // On any failure (missing fields, queue open error, enqueue error) we
    // fall back to the original deny-with-guidance path — never silently drop.
    // ========================================================================
    // `is_factory_agent` already computed above for the hoisted
    // cas_root=None auto-approve check (cas-7f33).
    if is_factory_agent && tool_name == "SendMessage" {
        return Ok(auto_route_send_message(
            input.tool_input.as_ref(),
            cas_root,
            &current_agent_id,
        ));
    }

    let is_supervisor = crate::harness_policy::is_supervisor_from_env();

    // ========================================================================
    // CODEMAP FRESHNESS GATE: Block supervisor from creating tasks / spawning
    // workers while CODEMAP.md is significantly out of date.
    //
    // Workers use CODEMAP for codebase orientation. Dispatching them against a
    // stale map wastes tokens and produces drift. The SessionStart warning is
    // informational; this gate enforces "update before assigning work".
    //
    // Only fires for supervisors, only on the two dispatch tools, only when
    // staleness >= SIGNIFICANT_STALENESS_THRESHOLD. Running `/codemap` bumps
    // CODEMAP.md's mtime and clears the gate on the next call.
    // ========================================================================
    if is_supervisor {
        let action = input
            .tool_input
            .as_ref()
            .and_then(|ti| ti.get("action").and_then(|v| v.as_str()));
        let is_gated =
            is_codemap_gated_tool_call(tool_name, action, crate::harness_policy::own_tool_prefix());
        if is_gated {
            if let Some(
                crate::hooks::handlers::handlers_events::CodemapStaleness::SignificantlyStale {
                    total_changes,
                    ..
                },
            ) = crate::hooks::handlers::handlers_events::check_codemap_freshness(cas_root)
            {
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &format!(
                        "🗺️  CODEMAP.md is significantly out of date ({total_changes} structural changes).\n\n\
                        Workers rely on CODEMAP for codebase orientation — dispatching against a stale map wastes tokens.\n\n\
                        Run `/codemap` to refresh, then retry."
                    ),
                ));
            }
        }
    }

    // ========================================================================
    // WORKTREE MERGE JAIL: Block all tools except worktree-merger when pending
    //
    // When a task has pending_worktree_merge=true, block all tools except:
    // 1. Task tool spawning worktree-merger - unjails by clearing pending_worktree_merge
    //
    // The unjail happens in PreToolUse when Task(worktree-merger) is detected.
    //
    // NOTE: This entire system is EXPERIMENTAL and only active when worktrees.enabled=true
    //
    // Only jail the agent that owns the tasks (via leases), not all agents.
    // ========================================================================
    let worktrees_enabled = stores.config().worktrees_enabled();

    // Factory workers manage their own worktrees — skip CAS worktree enforcement
    // to avoid conflicting redirects (factory uses per-worker worktrees, CAS uses per-epic)
    let is_factory_worker_for_wt = std::env::var("CAS_AGENT_ROLE")
        .map(|role| role.to_lowercase() == "worker")
        .unwrap_or(false);

    if worktrees_enabled && !is_factory_worker_for_wt {
        if let Some(task_store) = stores.tasks().cloned() {
            if let Ok(tasks) = task_store.list_pending_worktree_merge() {
                // Only consider tasks the current agent owns (reuses agent_task_ids from above)
                let pending_merge_tasks: Vec<_> = tasks
                    .iter()
                    .filter(|t| {
                        agent_task_ids.contains(&t.id)
                            || t.assignee
                                .as_ref()
                                .map(|a| a == &current_agent_id)
                                .unwrap_or(false)
                    })
                    .collect();

                if !pending_merge_tasks.is_empty() {
                    // Check if this is Task tool spawning worktree-merger
                    let is_worktree_merger = if tool_name == "Task" {
                        input
                            .tool_input
                            .as_ref()
                            .and_then(|ti| ti.get("subagent_type").and_then(|v| v.as_str()))
                            .map(|st| st == "worktree-merger")
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if is_worktree_merger {
                        // Clear jail - worktree-merger agent will handle the merge
                        let task_ids: Vec<_> =
                            pending_merge_tasks.iter().map(|t| t.id.as_str()).collect();
                        for task in &pending_merge_tasks {
                            let mut task_to_update = (*task).clone();
                            task_to_update.pending_worktree_merge = false;
                            task_to_update.updated_at = chrono::Utc::now();
                            let _ = task_store.update(&task_to_update);
                        }
                        eprintln!(
                            "cas: Unjailing for worktree-merger (tasks: {})",
                            task_ids.join(", ")
                        );
                    } else {
                        let task_ids: Vec<_> =
                            pending_merge_tasks.iter().map(|t| t.id.as_str()).collect();
                        let task_list = task_ids.join(", ");

                        return Ok(HookOutput::with_pre_tool_permission(
                            "deny",
                            &format!(
                                "🔒 WORKTREE MERGE JAIL: Task(s) {task_list} require worktree merge before you can continue.\n\n\
                            You MUST spawn the 'worktree-merger' agent to merge and clean up the worktree.\n\n\
                            Example: Use the Task tool with subagent_type=\"worktree-merger\" and prompt describing the task to merge."
                            ),
                        ));
                    }
                }
            }
        }

        // ========================================================================
        // WORKTREE PATH ENFORCEMENT: Redirect file ops to worktree when applicable
        //
        // When an agent is working on a task that belongs to an epic with a worktree,
        // block file operations in the main repo and redirect to the worktree.
        // This ensures isolation between concurrent agents working on different epics.
        // ========================================================================
        let file_tools = ["Read", "Write", "Edit", "Glob", "Grep", "Bash"];
        if file_tools.iter().any(|t| tool_name.eq_ignore_ascii_case(t)) {
            // Get file path from tool input
            let tool_file_path = input.tool_input.as_ref().and_then(|ti| {
                ti.get("file_path")
                    .or_else(|| ti.get("path"))
                    .and_then(|v| v.as_str())
            });

            if let Some(file_path) = tool_file_path {
                // Check if agent has tasks in epics with worktrees
                if let Some(agent_store) = stores.agents().cloned() {
                    if let Some(task_store) = stores.tasks().cloned() {
                        if let Ok(leases) = agent_store.list_agent_leases(&current_agent_id) {
                            for lease in &leases {
                                if let Ok(task) = task_store.get(&lease.task_id) {
                                    // Check if this task belongs to an epic with a worktree
                                    if let Ok(deps) = task_store.get_dependencies(&task.id) {
                                        for dep in &deps {
                                            if dep.dep_type == DependencyType::ParentChild {
                                                if let Ok(parent) = task_store.get(&dep.to_id) {
                                                    if parent.task_type == TaskType::Epic {
                                                        if let Some(ref worktree_id) =
                                                            parent.worktree_id
                                                        {
                                                            // Epic has a worktree - check if file is in main repo
                                                            if let Some(wt_store) =
                                                                stores.worktrees().cloned()
                                                            {
                                                                if let Ok(worktree) =
                                                                    wt_store.get(worktree_id)
                                                                {
                                                                    let worktree_path = worktree
                                                                        .path
                                                                        .to_string_lossy();
                                                                    let main_repo =
                                                                        input.cwd.clone();

                                                                    // If file is in main repo but NOT in worktree, block
                                                                    let file_in_main = file_path
                                                                        .starts_with(&main_repo);
                                                                    let file_in_worktree =
                                                                        file_path.starts_with(
                                                                            worktree_path.as_ref(),
                                                                        );

                                                                    if file_in_main
                                                                        && !file_in_worktree
                                                                    {
                                                                        // Calculate the equivalent path in worktree
                                                                        let relative_path =
                                                                            file_path
                                                                                .strip_prefix(
                                                                                    &main_repo,
                                                                                )
                                                                                .unwrap_or(
                                                                                    file_path,
                                                                                )
                                                                                .trim_start_matches(
                                                                                    '/',
                                                                                );
                                                                        let suggested_path = format!(
                                                                            "{worktree_path}/{relative_path}"
                                                                        );

                                                                        return Ok(HookOutput::with_pre_tool_permission(
                                                                        "deny",
                                                                        &format!(
                                                                            "🌳 WORKTREE REDIRECT: You're working on epic [{}] \"{}\" which has a dedicated worktree.\n\n\
                                                                            ❌ Blocked: {}\n\
                                                                            ✅ Use instead: {}\n\n\
                                                                            All file operations for this epic should happen in the worktree directory:\n\
                                                                            📁 {}",
                                                                            parent.id,
                                                                            parent.title,
                                                                            file_path,
                                                                            suggested_path,
                                                                            worktree_path
                                                                        ),
                                                                    ));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ========================================================================
                // WORKTREE LEASE CHECK: Warn if accessing a worktree locked by another agent
                //
                // If the file path is in a worktree directory and another agent holds
                // the lease, warn the user (coordination-level, not blocking)
                // ========================================================================
                if let Some(wt_store) = stores.worktrees().cloned() {
                    if let Some(agent_store) = stores.agents().cloned() {
                        // Get all active worktrees and check if file is in any of them
                        if let Ok(worktrees) = wt_store.list_active() {
                            for worktree in worktrees {
                                let worktree_path_str = worktree.path.to_string_lossy();
                                if file_path.starts_with(worktree_path_str.as_ref()) {
                                    // File is in this worktree - check the lease
                                    if let Ok(Some(lease)) =
                                        agent_store.get_worktree_lease(&worktree.id)
                                    {
                                        if lease.agent_id != current_agent_id && lease.is_valid() {
                                            // Another agent holds the lease - warn but don't block
                                            eprintln!(
                                                "⚠️  WORKTREE LEASE: {} is locked by agent {} (expires in {}s)",
                                                worktree.path.display(),
                                                lease.agent_id,
                                                lease.remaining_secs()
                                            );
                                        }
                                    }
                                    break; // Found the worktree, no need to check others
                                }
                            }
                        }
                    }
                }
            }
        }
    } // End of worktrees_enabled block

    // Get file path from tool input (if applicable)
    let file_path = input
        .tool_input
        .as_ref()
        .and_then(|ti| ti.get("file_path").and_then(|v| v.as_str()));

    // Load proven rules with auto-approve configuration
    let rule_store = stores.rules()?;
    let rules = rule_store.list_proven()?;

    // Check if any rule auto-approves this tool call
    for rule in &rules {
        if !rule.can_auto_approve() {
            continue;
        }

        // Check if this tool is in the rule's auto-approve list
        if !rule.auto_approves_tool(tool_name) {
            continue;
        }

        // If rule has path patterns, check if the file matches
        if let Some(path) = file_path {
            if rule.matches_auto_approve_path(path) {
                eprintln!(
                    "cas: PreToolUse auto-approved {} on {} via rule {}",
                    tool_name, path, &rule.id
                );
                return Ok(HookOutput::with_pre_tool_permission(
                    "allow",
                    &format!("Auto-approved by rule {}: {}", rule.id, rule.preview(50)),
                ));
            }
        } else {
            // No file path - auto-approve if tool is in safe list and rule allows it
            if Rule::SAFE_AUTO_APPROVE_TOOLS
                .iter()
                .any(|t| t.eq_ignore_ascii_case(tool_name))
            {
                eprintln!(
                    "cas: PreToolUse auto-approved {} (safe tool) via rule {}",
                    tool_name, &rule.id
                );
                return Ok(HookOutput::with_pre_tool_permission(
                    "allow",
                    &format!("Auto-approved safe tool by rule {}", rule.id),
                ));
            }
        }
    }

    // Check for protected paths that should be blocked (configurable)
    let protection = &stores.config().hooks().pre_tool_use.protection;

    if protection.enabled {
        if let Some(path) = file_path {
            // Block access to protected files (e.g., .env files)
            for pattern in &protection.files {
                if path.ends_with(pattern) || path.contains(&format!("/{pattern}")) {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        &format!("Protected file: {pattern} files may contain secrets"),
                    ));
                }
            }

            // Block access to credential files
            for pattern in &protection.patterns {
                if path.ends_with(pattern) || path.contains(pattern) {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "Protected file: may contain credentials or private keys",
                    ));
                }
            }
        }
    }

    // Mint verifier authority only on the authenticated parent's Task/Agent
    // spawn path. Authority stays entirely server-side: this hook leaves the
    // model-visible prompt/input byte-for-byte unchanged, while SubagentStart
    // binds the sole sealed handoff to the official distinct child agent_id.
    if matches!(tool_name, "Task" | "Agent")
        && input
            .tool_input
            .as_ref()
            .and_then(|value| value.get("subagent_type"))
            .and_then(|value| value.as_str())
            == Some("task-verifier")
    {
        let Some(agent_store) = stores.agents() else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "Cannot establish verifier authority: agent registry is unavailable.",
            ));
        };
        let Ok(parent) = agent_store.get(&current_agent_id) else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "Cannot establish verifier authority for an anonymous or orphan session.",
            ));
        };
        if !matches!(
            parent.status,
            crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle
        ) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "Cannot establish verifier authority for an inactive parent session.",
            ));
        }

        let Some(tool_input) = input.tool_input.as_ref() else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "task-verifier spawn requires a prompt naming exactly one CAS task.",
            ));
        };
        let prompt = tool_input
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let Some(task_id) = unique_existing_task_id(prompt, stores.tasks()) else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "task-verifier prompt must name exactly one existing CAS task ID.",
            ));
        };
        let dispatch_id = match cas_store::get_latest_verification_dispatch(cas_root, &task_id) {
            Ok(Some(dispatch))
                if matches!(
                    dispatch.state,
                    cas_types::VerificationDispatchState::Pending
                        | cas_types::VerificationDispatchState::Claimed
                ) =>
            {
                if dispatch.owner_agent_id != current_agent_id {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "This task's verification dispatch is owned by another registered session.",
                    ));
                }
                if dispatch.deadline_at <= chrono::Utc::now() {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "This task's verification dispatch deadline has elapsed; use the recorded recovery path.",
                    ));
                }
                dispatch.id
            }
            Ok(_) => {
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    "No active owned verification dispatch exists for this task; create the exact close proof cycle before spawning a verifier.",
                ));
            }
            Err(_) => {
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    "Could not validate task-scoped verification dispatch authority.",
                ));
            }
        };
        let Some(tool_use_id) = input
            .tool_use_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "Cannot establish verifier authority: PreToolUse did not provide tool_use_id correlation.",
            ));
        };
        match issue_hook_verifier_handoff(
            cas_root,
            &task_id,
            &dispatch_id,
            &current_agent_id,
            tool_use_id,
        ) {
            Ok(_) => {}
            Err(error) => {
                let message = error.to_string();
                if message.contains("already awaiting SubagentStart") {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "Another task-verifier spawn is already awaiting SubagentStart for this parent. Wait for it to bind, or retry after the failed spawn is cleaned up or expires.",
                    ));
                }
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    "Could not establish server-side task-verifier authority for the exact dispatch.",
                ));
            }
        }
        return Ok(HookOutput::empty());
    }

    // ========================================================================
    // FACTORY MODE: Unconditional auto-approve for filesystem tool families.
    //
    // Claude Code 2.1.116's team-mode permission layer escalates any "ask"
    // decision to the team leader via `Mq4()`, gated on a broken self-check:
    //
    //     function UG9(H) {
    //       let q = hP();                    // self agentId
    //       return !q || q === "team-lead";  // hardcoded string compare
    //     }
    //     function L6$(){ ... return teamName && selfAgentId && !UG9(); }
    //
    // CAS gives the supervisor agentId `supervisor@<team>` and workers
    // `<worker-name>@<team>` — neither is the literal string `"team-lead"`,
    // so `UG9()` returns false for every factory agent, `L6$()` returns true,
    // and every Write/Edit/Bash permission check routes to the leader. The
    // leader IS the supervisor, which has no UX path to self-approve → the
    // modal `Waiting for team lead approval` hangs forever. Workers hit it
    // too, escalating to a supervisor that may be busy or idle.
    //
    // cas-e15d (ffb76df) tried to bypass this by shipping `--settings`
    // allowlist files with `permissions.allow: ["Write",...]` for supervisor
    // and workers, expecting the classifier to return `{behavior:"allow"}`
    // and skip `Mq4`. Empirically the escalation still fires — the classifier
    // does not honor bare-toolname allow rules the way we assumed, or merge
    // precedence clobbers them. We keep those files as belt-and-suspenders
    // but the real fix lives HERE: a PreToolUse hook runs *before* the
    // classifier, and an explicit `permissionDecision: "allow"` short-circuits
    // the entire local-then-team decision flow.
    //
    // Scope: only the filesystem tool families whose allowlist matched the
    // supervisor/worker settings file. MCP tools, Agent, Task, and the rest
    // still flow through Claude Code's normal paths so their own rule logic
    // keeps working. Protection gates above (this block runs AFTER) still
    // win — .env / credential writes are denied before we reach here.
    //
    // cas-7f33: a second copy of this gate runs ABOVE the cas_root=None
    // early return to rescue factory sessions where CAS isn't initialized
    // in the supervisor's cwd at hook-dispatch time. That hoisted copy
    // fires only when cas_root is None (so no protection gates apply
    // anyway). When cas_root is Some the flow reaches HERE, preserving
    // the .env-deny-before-auto-approve invariant.
    //
    // See: project_cas_team_permission_escalation_bug memory for the
    // full disassembly that identified the upstream root cause.
    // ========================================================================
    if is_factory_agent && FACTORY_AUTO_APPROVE_TOOLS.contains(&tool_name) {
        return Ok(HookOutput::with_pre_tool_permission(
            "allow",
            &format!(
                "Factory agent auto-approve ({tool_name}) — bypasses Claude Code team-mode leader-escalation deadlock (UG9 bug)"
            ),
        ));
    }

    // No rule matched, no protection triggered - let Claude ask the user
    Ok(HookOutput::empty())
}

// ── Worker commit guard helpers (cas-bea2, LAYER 1) ───────────────────────
//
// Detects `git commit` / `git merge` Bash commands from factory workers
// and denies them when HEAD is on a protected branch OR (for isolated
// workers) the cwd is outside the assigned worktree (CAS_CLONE_PATH).
//
// Fires for ALL factory workers (CAS_AGENT_ROLE=worker && CAS_FACTORY_MODE),
// whether or not they have an isolated worktree (CAS_CLONE_PATH). This
// prevents standalone-task workers without a CAS_CLONE_PATH from committing
// to protected branches (main/master/staging) in the shared primary checkout
// (cas-ba04 regression fix).
//
// cas-7e7b: branch policy changed from allowlist (only factory/*) to
// denylist (block main/master/staging + detached HEAD; everything else is
// allowed). Workers on feature/, fix/, epic/, or arbitrary branches can now
// commit without supervisor intervention.
//
// Escape-hatch note: `--no-verify` does NOT bypass this guard. That flag
// only skips git's own commit-msg/pre-commit hooks, not the Claude Code
// PreToolUse harness. The only way to commit is to be on a non-protected
// branch.

/// Return true if `branch` is a branch a factory worker is allowed to commit on.
///
/// Policy (cas-7e7b, denylist semantics — previously allowlist):
/// - DENIED: `main`, `master`, `staging`, or empty string (detached HEAD).
/// - ALLOWED: everything else — `factory/<name>`, `feature/*`, `fix/*`,
///   `epic/*`, arbitrary named branches.
///
/// This was changed from allowlist (only `factory/*`) because workers
/// legitimately work on feature branches (e.g. spawned outside the
/// isolated-worktree flow, or on a project-level branch), and blocking them
/// causes hard stalls that require supervisor intervention.
pub(crate) fn is_worker_commit_allowed_branch(branch: &str) -> bool {
    let b = branch.trim();
    !matches!(b, "main" | "master" | "staging" | "")
}

/// Return true if `cmd` looks like a `git commit` or `git merge` invocation.
///
/// Matches common forms:
/// - `git commit -m "msg"`
/// - `git -C /some/path commit`
/// - `git merge main`
/// - Commands with env-var prefixes like `GIT_AUTHOR_NAME=... git commit`
///
/// Intentionally conservative: false-negatives (missed commands) are safe
/// because LAYER 2 (pre-commit hook) is the hard floor.
pub(crate) fn looks_like_git_write_op(cmd: &str) -> bool {
    // Find the first occurrence of "git" as a word boundary
    let mut rest = cmd;
    loop {
        let pos = match rest.find("git") {
            Some(p) => p,
            None => return false,
        };
        // Ensure "git" is not a substring of another word (e.g. "config")
        let before_ok = pos == 0 || !rest.as_bytes()[pos - 1].is_ascii_alphanumeric();
        let after_idx = pos + 3;
        let after_ok =
            after_idx >= rest.len() || !rest.as_bytes()[after_idx].is_ascii_alphanumeric();
        if before_ok && after_ok {
            let after_git = &rest[after_idx..];
            // After "git" there may be flags like -C /path before the subcommand
            // We look for "commit" or "merge" as a word anywhere after "git"
            return after_git
                .split_whitespace()
                .any(|tok| tok == "commit" || tok == "merge");
        }
        // Not a word boundary — advance past this occurrence
        rest = &rest[pos + 1..];
    }
}

/// Run `git symbolic-ref --short HEAD` in `cwd` and return the branch name.
/// Returns `None` on detached HEAD, git unavailable, or any error.
pub(crate) fn get_branch_at_cwd(cwd: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", cwd, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Check whether a factory worker's `git commit` / `git merge` should be denied.
///
/// Returns `Some(denial_message)` when:
/// - HEAD at `cwd` is a protected branch (`main`, `master`, `staging`) or detached.
/// - `CAS_CLONE_PATH` is set (isolated worker) AND `cwd` is outside the worktree.
///
/// Returns `None` to allow when HEAD is on a non-protected named branch.
///
/// This guard fires for BOTH isolated workers (CAS_CLONE_PATH set) and
/// non-isolated workers (no CAS_CLONE_PATH). Non-isolated (standalone-task)
/// workers that run in the shared primary checkout must not commit to
/// main/master/staging either (cas-ba04).
///
/// Note: `--no-verify` does NOT bypass this guard — it only skips git's own
/// commit-msg/pre-commit hooks, not the Claude Code PreToolUse harness.
/// Switching to a non-protected branch is the only way to unblock.
pub(crate) fn check_worker_git_commit_scope(cwd: &str) -> Option<String> {
    let clone_path = std::env::var("CAS_CLONE_PATH").ok();
    let is_isolated = clone_path
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // DENY: isolated worker's cwd is outside the assigned worktree.
    // Only applicable when CAS_CLONE_PATH is set.
    if is_isolated {
        let clone_path = clone_path.as_deref().unwrap();
        let cwd_path = std::path::Path::new(cwd);
        let worktree_path = std::path::Path::new(clone_path);

        if !cwd_path.starts_with(worktree_path) {
            let worker_name =
                std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());
            return Some(format!(
                "🚫 WORKER COMMIT GUARD: Your current directory ({cwd}) is outside \
                your assigned worktree ({clone_path}).\n\n\
                Workers MUST commit inside their worktree. Switch first:\n  \
                cd {clone_path}\n  git switch factory/{worker_name}\n\n\
                Then retry your commit from there.\n\n\
                Note: --no-verify does NOT bypass this guard (it only skips git hooks,\n\
                not the Claude Code PreToolUse harness)."
            ));
        }
    }

    // DENY: HEAD is on a protected branch (main/master/staging) or detached.
    // Any other named branch — factory/*, feature/*, fix/*, epic/*, etc. — is allowed.
    // Applies to BOTH isolated and non-isolated factory workers (cas-ba04): a worker
    // without CAS_CLONE_PATH running in the shared checkout must not commit to main.
    let worker_name =
        std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());
    match get_branch_at_cwd(cwd) {
        None => {
            return Some(format!(
                "🚫 WORKER COMMIT GUARD: HEAD is detached — cannot determine branch.\n\n\
                Commits require a named branch. Switch to your work branch first:\n  \
                git switch factory/{worker_name}   # or your feature/fix branch\n\n\
                Your staged changes are preserved — only the branch matters.\n\n\
                Note: --no-verify does NOT bypass this guard (it only skips git hooks,\n\
                not the Claude Code PreToolUse harness)."
            ));
        }
        Some(branch) if !is_worker_commit_allowed_branch(&branch) => {
            // cas-5bef (GH #120): the refusal below used to end at "create a
            // branch here", and a non-isolated worker took exactly that escape
            // — it left the SHARED checkout parked on factory/*, after which
            // the supervisor's `git merge --ff-only` / `git push origin main`
            // both reported success while landing nothing on main. For the
            // non-isolated case the remedy must preserve shared HEAD (own
            // worktree), and the in-place fallback must say: restore trunk
            // after pushing.
            let remedy = if is_isolated {
                format!(
                    "Switch to your work branch and commit there:\n  \
                    git switch factory/{worker_name}   # or: git switch <your-feature-branch>\n  \
                    git commit ...\n"
                )
            } else {
                format!(
                    "\nYou are running without an isolated worktree (CAS_CLONE_PATH not set), so \
                    this is the SHARED checkout.\n\
                    Its HEAD is read by every other agent here and by the supervisor's \
                    merge/tag sequence — do NOT leave it pointing at a factory branch.\n\n\
                    PREFERRED — keep shared HEAD on '{branch}' and work in your own worktree:\n  \
                    git worktree add ../factory-{worker_name} -b factory/{worker_name}\n  \
                    cd ../factory-{worker_name}   # commit and push from there\n\n\
                    FALLBACK — if you must branch in place, RESTORE TRUNK AFTER PUSH:\n  \
                    git switch -c factory/{worker_name}\n  \
                    git commit ... && git push -u origin factory/{worker_name}\n  \
                    git switch {branch}   # MANDATORY: a shared checkout left on factory/* \
                    makes `git merge --ff-only` and `git push origin {branch}` silently \
                    misfire (GH #120)\n"
                )
            };
            return Some(format!(
                "🚫 WORKER COMMIT GUARD: Direct commits to '{branch}' are blocked.\n\n\
                Workers must NOT commit directly to protected branches \
                (main, master, staging).\n\
                {remedy}\n\
                Your staged changes are preserved — only the branch matters.\n\n\
                Note: --no-verify does NOT bypass this guard (it only skips git hooks,\n\
                not the Claude Code PreToolUse harness). Switching branches is the only option."
            ));
        }
        Some(_) => {} // any non-protected named branch — allowed
    }

    None
}

/// Filesystem tool families auto-approved for factory agents (supervisor and
/// workers). Matches the `permissions.allow` list written by
/// `cas-cli/src/ui/factory/daemon/runtime/teams.rs::worker_settings_contents`
/// and `supervisor_settings_contents` — keep the two lists in sync or the
/// belt-and-suspenders settings-file path diverges from the hook path.
///
/// Consumers in this crate (keep all in sync when editing membership):
/// - `handle_pre_tool_use` (this file) — PreToolUse auto-approve. Two
///   copies: hoisted `cas_root=None` rescue and the post-protection
///   `cas_root=Some` path.
/// - `super::notifications::handle_permission_request` — the cas-7f33
///   PermissionRequest belt #3 that covers Claude Code 2.1.x builds where
///   PreToolUse `allow` doesn't pre-empt team-mode leader escalation.
pub(crate) const FACTORY_AUTO_APPROVE_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "Bash",
    "NotebookEdit",
];

/// Whether `tool_name`/`action` is one of the CODEMAP-freshness-gated calls
/// (task creation, worker spawn) — keyed on the CALLER's own `tool_prefix`
/// so this recognizes `mcp__cas__task`/`mcp__cs__task`/`cas__task` etc.
/// correctly for whichever harness is actually running.
///
/// EPIC cas-8888 (cas-fd9f): extracted from `handle_pre_tool_use` (was
/// inline, hardcoded to `mcp__cas__task`/`mcp__cas__coordination` — silently
/// inert for every non-Claude supervisor, since `tool_name` is whatever the
/// CALLING process's own harness actually named the tool).
fn is_codemap_gated_tool_call(tool_name: &str, action: Option<&str>, tool_prefix: &str) -> bool {
    let task_tool = format!("{tool_prefix}task");
    let coordination_tool = format!("{tool_prefix}coordination");
    (tool_name == task_tool && action == Some("create"))
        || (tool_name == coordination_tool
            && matches!(action, Some("spawn_workers") | Some("spawn_worker")))
}

/// Return the single existing CAS task ID named in a verifier prompt.
///
/// Ambiguous prompts fail closed: authority must bind to one exact task, not
/// whichever task ID happens to be encountered first.
fn unique_existing_task_id(
    prompt: &str,
    task_store: Option<&std::sync::Arc<dyn TaskStore>>,
) -> Option<String> {
    let store = task_store?;
    let mut matches = std::collections::BTreeSet::new();
    for token in prompt.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-')) {
        if token.starts_with("cas-") && store.get(token).is_ok() {
            matches.insert(token.to_string());
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn issue_hook_verifier_handoff(
    cas_root: &Path,
    task_id: &str,
    dispatch_id: &str,
    issuer_agent_id: &str,
    tool_use_id: &str,
) -> cas_store::Result<cas_types::VerifierCapability> {
    #[cfg(test)]
    if let Some(secret) = TEST_VERIFIER_HANDOFF_SECRET.with(|slot| slot.borrow().as_ref().cloned())
    {
        return cas_store::issue_server_verifier_handoff_with_secret(
            cas_root,
            task_id,
            dispatch_id,
            issuer_agent_id,
            tool_use_id,
            &secret,
        );
    }
    cas_store::issue_server_verifier_handoff(
        cas_root,
        task_id,
        dispatch_id,
        issuer_agent_id,
        tool_use_id,
    )
}

#[cfg(test)]
thread_local! {
    static TEST_VERIFIER_HANDOFF_SECRET: std::cell::RefCell<Option<Vec<u8>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn with_test_verifier_handoff_secret<T>(secret: &[u8], f: impl FnOnce() -> T) -> T {
    TEST_VERIFIER_HANDOFF_SECRET.with(|slot| {
        *slot.borrow_mut() = Some(secret.to_vec());
    });
    let result = f();
    TEST_VERIFIER_HANDOFF_SECRET.with(|slot| {
        *slot.borrow_mut() = None;
    });
    result
}

/// Auto-route a factory-mode `SendMessage` tool call onto the CAS prompt
/// queue so the message actually reaches its recipient, then return an
/// `allow` + `additionalContext` success receipt (cas-73c8). Returning
/// `deny` wrapped the ✅ receipt in Claude Code's `<error>` envelope, which
/// agents and tooling treated as failure even though delivery succeeded.
///
/// On any parse / queue failure, falls back to the original deny-with-
/// guidance path — we never silently drop the agent's message.
fn auto_route_send_message(
    tool_input: Option<&serde_json::Value>,
    cas_root: &Path,
    current_agent_id: &str,
) -> HookOutput {
    // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — reminder text describing
    // what THIS agent should call instead of SendMessage.
    let fallback_guidance = || {
        let prefix = crate::harness_policy::own_tool_prefix();
        HookOutput::with_pre_tool_permission(
            "deny",
            &format!(
                "🚫 SendMessage is disabled in factory mode.\n\n\
                 Use CAS coordination instead:\n\
                 {prefix}coordination action=message target=<agent-name> message=\"...\" summary=\"<brief summary>\"\n\n\
                 This ensures messages are routed through the factory Director."
            ),
        )
    };

    let Some(ti) = tool_input else {
        return fallback_guidance();
    };

    let target = match ti.get("to").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => return fallback_guidance(),
    };

    // SendMessage.message may be a plain string OR a structured object
    // (shutdown_response, plan_approval_response, etc.). Serialize objects
    // to JSON so downstream reads still carry the full payload.
    let body = match ti.get("message") {
        Some(v) if v.is_string() => v.as_str().unwrap_or("").to_string(),
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => return fallback_guidance(),
    };
    if body.trim().is_empty() {
        return fallback_guidance();
    }

    let summary = ti
        .get("summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            body.lines()
                .next()
                .unwrap_or(&body)
                .chars()
                .take(80)
                .collect()
        });

    // Resolve sender display name — prefer CAS_AGENT_NAME env (set by
    // factory supervisor/worker spawn), fall back to agent_store lookup by
    // session id, else "unknown".
    let display_name = std::env::var("CAS_AGENT_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            crate::store::open_agent_store(cas_root)
                .ok()
                .and_then(|store| store.get(current_agent_id).ok())
                .map(|agent| {
                    use cas_types::AgentRole;
                    if agent.role == AgentRole::Supervisor {
                        "supervisor".to_string()
                    } else {
                        agent.name
                    }
                })
        })
        .unwrap_or_else(|| "unknown".to_string());

    let queue = match crate::store::open_prompt_queue_store(cas_root) {
        Ok(q) => q,
        Err(e) => {
            warn!(
                error = %e,
                "SendMessage auto-route: failed to open prompt queue — falling back to deny-with-guidance"
            );
            return fallback_guidance();
        }
    };

    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let message_id = match queue.enqueue_full(
        &display_name,
        &target,
        &body,
        factory_session.as_deref(),
        Some(summary.as_str()),
        None,
    ) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                target = target.as_str(),
                "SendMessage auto-route: enqueue_full failed — falling back to deny-with-guidance"
            );
            return fallback_guidance();
        }
    };

    // Best-effort: wake the daemon so delivery doesn't wait for the next
    // poll cycle. Failure just means the daemon isn't running — the row
    // is still persisted and will be picked up.
    let _ = cas_factory::notify_daemon(cas_root);

    info!(
        message_id,
        source = display_name.as_str(),
        target = target.as_str(),
        "SendMessage auto-routed onto CAS prompt queue"
    );

    let prefix = crate::harness_policy::own_tool_prefix();
    // cas-73c8: success-shaped receipt. `permissionDecision=allow` so Claude
    // Code does not wrap the receipt in `<error>`; the guidance lives in
    // `additionalContext` (visible to the model next to the tool result).
    // `permissionDecisionReason` is user-facing only on allow.
    //
    // Native SendMessage may also run after allow; inbox content-dedupe
    // (teams write_to_inbox) suppresses an identical second write.
    let receipt = format!(
        "✅ AUTO-ROUTED via CAS coordination (message id {message_id}).\n\n\
         Message delivered to `{target}`. DO NOT retry this SendMessage call.\n\n\
         For future messages, call `{prefix}coordination action=message target=<name> message=\"...\" summary=\"...\"` directly — skip SendMessage."
    );
    HookOutput::with_pre_tool_permission_and_context(
        "allow",
        "CAS auto-routed SendMessage",
        &receipt,
    )
}

#[cfg(test)]
mod worker_commit_guard_tests {
    use super::*;

    use crate::test_support::TestEnvGuard;

    // Helper: create a temp git repo with an initial commit on `main`.
    fn make_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        std::fs::write(p.join("f.txt"), "hi").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        tmp
    }

    // ── looks_like_git_write_op tests ─────────────────────────────────────

    #[test]
    fn git_commit_detected() {
        assert!(looks_like_git_write_op("git commit -m 'foo'"));
    }

    #[test]
    fn git_commit_with_path_flag_detected() {
        assert!(looks_like_git_write_op("git -C /some/path commit -m msg"));
    }

    #[test]
    fn git_merge_detected() {
        assert!(looks_like_git_write_op("git merge main"));
    }

    #[test]
    fn git_merge_with_flags_detected() {
        assert!(looks_like_git_write_op("git merge --no-ff factory/worker1"));
    }

    #[test]
    fn git_status_not_detected() {
        assert!(!looks_like_git_write_op("git status"));
    }

    #[test]
    fn git_add_not_detected() {
        assert!(!looks_like_git_write_op("git add ."));
    }

    #[test]
    fn non_git_command_not_detected() {
        assert!(!looks_like_git_write_op("ls -la"));
        assert!(!looks_like_git_write_op("cargo test"));
        assert!(!looks_like_git_write_op("echo commit this"));
    }

    #[test]
    fn git_substring_in_other_word_not_detected() {
        // "config" contains "git" — must not false-positive
        assert!(!looks_like_git_write_op("digitalocean config commit"));
    }

    // ── is_worker_commit_allowed_branch tests (cas-7e7b denylist) ──────────

    #[test]
    fn factory_branches_are_allowed() {
        // factory/* branches are still allowed
        assert!(is_worker_commit_allowed_branch("factory/worker1"));
        assert!(is_worker_commit_allowed_branch("factory/guards"));
        assert!(is_worker_commit_allowed_branch("factory/surface"));
        // Leading/trailing whitespace (from git output) is tolerated
        assert!(is_worker_commit_allowed_branch("  factory/guards  "));
    }

    #[test]
    fn protected_trunk_branches_are_denied() {
        // Only the trunk protection branches are denied (denylist semantics)
        assert!(!is_worker_commit_allowed_branch("main"));
        assert!(!is_worker_commit_allowed_branch("master"));
        assert!(!is_worker_commit_allowed_branch("staging"));
        // Empty string (detached HEAD sentinel)
        assert!(!is_worker_commit_allowed_branch(""));
    }

    #[test]
    fn non_trunk_branches_are_allowed() {
        // cas-7e7b: feature/fix/epic branches are now allowed (denylist, not allowlist)
        assert!(is_worker_commit_allowed_branch("epic/big-feature"));
        assert!(is_worker_commit_allowed_branch("epic/cas-073f"));
        assert!(is_worker_commit_allowed_branch("feature/foo"));
        assert!(is_worker_commit_allowed_branch("fix/my-bug"));
        assert!(is_worker_commit_allowed_branch("chore/update-deps"));
        assert!(is_worker_commit_allowed_branch("my-arbitrary-branch"));
    }

    // ── get_branch_at_cwd tests ───────────────────────────────────────────

    #[test]
    fn get_branch_returns_branch_name() {
        let tmp = make_git_repo();
        let branch = get_branch_at_cwd(&tmp.path().to_string_lossy());
        assert_eq!(branch.as_deref(), Some("main"));
    }

    #[test]
    fn get_branch_returns_none_for_nonexistent_dir() {
        let branch = get_branch_at_cwd("/nonexistent/path/12345");
        assert!(branch.is_none());
    }

    // ── check_worker_git_commit_scope tests ──────────────────────────────

    // ── cas-ba04 regression: non-isolated worker protection ──────────────

    #[test]
    fn non_isolated_worker_on_main_is_denied() {
        // Regression test for cas-ba04: a factory worker with no CAS_CLONE_PATH
        // (standalone task, no isolated worktree) must be blocked from committing
        // to protected branches just as isolated workers are.
        let tmp = make_git_repo(); // creates a repo on `main`
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", None)]);

        let result = check_worker_git_commit_scope(&p);
        assert!(
            result.is_some(),
            "non-isolated worker on main must be denied (cas-ba04)"
        );
        let msg = result.unwrap();
        assert!(
            msg.contains("WORKER COMMIT GUARD"),
            "expected guard msg, got: {msg}"
        );
        assert!(msg.contains("main"), "expected 'main' in msg, got: {msg}");
        assert!(
            msg.contains("CAS_CLONE_PATH not set"),
            "message should mention lack of isolation for actionable guidance: {msg}"
        );
    }

    /// cas-5bef (GH #120): the refusal a NON-ISOLATED worker sees must not
    /// leave "branch in place and stay there" as the implied escape — that is
    /// what parked the shared checkout on factory/bright-eagle-91 and made the
    /// supervisor's `git merge --ff-only` / `git push origin main` silently
    /// no-op during the v2.45.0 cut.
    #[test]
    fn non_isolated_refusal_steers_away_from_parking_shared_head() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("bright-eagle-91")),
        ]);

        let msg = check_worker_git_commit_scope(&p).expect("non-isolated worker on main is denied");

        // Preferred path: a worktree, which never re-points shared HEAD.
        assert!(
            msg.contains("git worktree add"),
            "refusal must offer a shared-HEAD-preserving path: {msg}"
        );
        assert!(
            msg.contains("SHARED checkout"),
            "refusal must say why this directory is special: {msg}"
        );
        // Fallback path: branching in place is allowed only with a restore.
        assert!(
            msg.contains("RESTORE TRUNK AFTER PUSH"),
            "the in-place fallback must mandate restoring trunk: {msg}"
        );
        assert!(
            msg.contains("git switch main"),
            "the restore step must name the trunk to return to: {msg}"
        );
        assert!(
            msg.contains("GH #120"),
            "the consequence must be attributable to the incident: {msg}"
        );
        // The old advice ended here; it must no longer be the last word.
        let switch_c = msg.find("git switch -c factory/bright-eagle-91").expect("fallback");
        let restore = msg.find("git switch main").expect("restore");
        assert!(
            restore > switch_c,
            "the restore step must follow the in-place branch creation: {msg}"
        );
    }

    /// The isolated case is unchanged: those workers own their worktree, so
    /// switching branches there re-points nothing shared.
    #[test]
    fn isolated_refusal_keeps_the_plain_branch_switch_remedy() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&p)),
            ("CAS_AGENT_NAME", Some("iso-worker")),
        ]);

        let msg = check_worker_git_commit_scope(&p).expect("isolated worker on main is denied");
        assert!(
            msg.contains("git switch factory/iso-worker"),
            "isolated workers keep the direct switch remedy: {msg}"
        );
        assert!(
            !msg.contains("git worktree add"),
            "isolated workers must not be told to create another worktree: {msg}"
        );
    }

    #[test]
    fn non_isolated_worker_on_safe_branch_is_allowed() {
        // Non-isolated worker on a non-protected branch (e.g. their own feature
        // branch) must still be allowed to commit.
        let tmp = make_git_repo();
        let p = tmp.path();
        std::process::Command::new("git")
            .args(["checkout", "-b", "factory/test-worker"])
            .current_dir(p)
            .output()
            .unwrap();
        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", None)]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_none(),
            "non-isolated worker on safe branch must be allowed, got: {result:?}"
        );
    }

    #[test]
    fn cwd_outside_worktree_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_path = tmp.path().join("worktree").to_string_lossy().to_string();
        let other_dir = tmp.path().join("other").to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&clone_path))]);

        let result = check_worker_git_commit_scope(&other_dir);
        assert!(result.is_some(), "expected deny when cwd outside worktree");
        let msg = result.unwrap();
        assert!(msg.contains("WORKER COMMIT GUARD"));
        assert!(msg.contains("outside your assigned worktree"));
    }

    #[test]
    fn cwd_inside_worktree_on_main_branch_denied() {
        // main is a protected branch — must still be blocked after cas-7e7b.
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&p))]);

        let result = check_worker_git_commit_scope(&p);
        assert!(result.is_some(), "expected deny on protected branch 'main'");
        let msg = result.unwrap();
        assert!(msg.contains("WORKER COMMIT GUARD"));
        assert!(msg.contains("main"));
        // Message must include the --no-verify note (cas-7e7b AC)
        assert!(
            msg.contains("--no-verify"),
            "message must explain --no-verify limitation: {msg}"
        );
    }

    #[test]
    fn cwd_inside_worktree_on_epic_branch_allowed() {
        // cas-7e7b: epic/* branches are no longer blocked (denylist semantics)
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "epic/cas-073f"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&ps))]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_none(),
            "epic/* branch must be allowed now, got: {result:?}"
        );
    }

    #[test]
    fn cwd_inside_worktree_on_feature_branch_allowed() {
        // cas-7e7b: feature/* branches are allowed (denylist semantics)
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "feature/my-widget"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&ps))]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_none(),
            "feature/* branch must be allowed, got: {result:?}"
        );
    }

    #[test]
    fn cwd_inside_worktree_detached_head_denied() {
        let tmp = make_git_repo();
        let p = tmp.path();

        // Detach HEAD by checking out the commit SHA directly
        let head_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(p)
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
        std::process::Command::new("git")
            .args(["checkout", "--detach", &sha])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&ps))]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_some(),
            "detached HEAD must be denied, got: {result:?}"
        );
        let msg = result.unwrap();
        assert!(msg.contains("WORKER COMMIT GUARD"));
        assert!(msg.contains("detached"));
    }

    #[test]
    fn cwd_inside_worktree_on_worker_branch_allowed() {
        let tmp = make_git_repo();
        let p = tmp.path();

        // Create and switch to factory/worker1 branch
        std::process::Command::new("git")
            .args(["checkout", "-b", "factory/worker1"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&ps))]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_none(),
            "expected allow on factory/worker1 branch, got: {result:?}"
        );
    }

    // ── Integration: handle_pre_tool_use for Bash git commit ─────────────

    #[test]
    fn pre_tool_denies_git_commit_on_protected_branch() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&p)),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = p.clone();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'oops'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(decision, "deny", "expected deny, got: {val}");
    }

    #[test]
    fn pre_tool_allows_git_commit_on_epic_branch() {
        // cas-7e7b: epic/* branches are now allowed (denylist semantics).
        // Previously these were denied; the over-broad allowlist caused worker
        // stalls in gabber-studio (true-wolf-20, 2026-06-26).
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "epic/big-feature"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = ps.clone();
        input.tool_input =
            Some(serde_json::json!({"command": "git commit -m 'work on epic branch'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(
            decision,
            Some("deny"),
            "epic/* branch must be allowed now, got: {val}"
        );
    }

    #[test]
    fn pre_tool_allows_git_commit_on_feature_branch() {
        // cas-7e7b: feature/* branches are allowed (denylist semantics).
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "feature/my-widget"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = ps.clone();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'add widget'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(
            decision,
            Some("deny"),
            "feature/* branch must be allowed, got: {val}"
        );
    }

    #[test]
    fn pre_tool_allows_git_commit_on_worker_branch() {
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "factory/guards"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = ps.clone();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'wip'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        // On a factory branch with correct cwd, guard must not deny
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(decision, Some("deny"), "expected allow/empty, got: {val}");
    }

    #[test]
    fn pre_tool_passes_through_for_non_worker() {
        // No CAS_AGENT_ROLE set → guard must not fire
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_FACTORY_MODE", None),
            ("CAS_CLONE_PATH", Some("/tmp/some-worktree")),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = "/tmp/other".to_string();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'foo'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(decision, Some("deny"), "non-worker must not be denied");
    }

    #[test]
    fn pre_tool_denies_git_commit_on_main_without_clone_path() {
        // Regression test for cas-ba04: a factory worker with no CAS_CLONE_PATH
        // (standalone task, no isolated worktree) must still be blocked from
        // committing to main via handle_pre_tool_use, not just check_worker_git_commit_scope.
        let tmp = make_git_repo(); // starts on `main`
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None), // no isolated worktree
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = p.clone();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'oops on main'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            decision, "deny",
            "non-isolated factory worker on main must be denied (cas-ba04), got: {val}"
        );
    }

    // ========================================================================
    // EPIC cas-8888 (cas-fd9f): harness-aware tool-name matcher guard tests.
    // ========================================================================

    #[test]
    fn codemap_gate_recognizes_claude_task_create() {
        assert!(is_codemap_gated_tool_call(
            "mcp__cas__task",
            Some("create"),
            "mcp__cas__"
        ));
    }

    #[test]
    fn codemap_gate_recognizes_codex_spawn_workers() {
        assert!(is_codemap_gated_tool_call(
            "mcp__cs__coordination",
            Some("spawn_workers"),
            "mcp__cs__"
        ));
    }

    /// The load-bearing regression: before cas-fd9f this matcher was
    /// hardcoded to "mcp__cas__task"/"mcp__cas__coordination" and so NEVER
    /// fired for a Grok supervisor (whose tool_name is "cas__task" etc.) —
    /// the CODEMAP freshness gate was silently inert for every non-Claude
    /// supervisor.
    #[test]
    fn codemap_gate_recognizes_grok_task_create_and_spawn_worker() {
        assert!(is_codemap_gated_tool_call(
            "cas__task",
            Some("create"),
            "cas__"
        ));
        assert!(is_codemap_gated_tool_call(
            "cas__coordination",
            Some("spawn_worker"),
            "cas__"
        ));
        assert!(is_codemap_gated_tool_call(
            "cas__coordination",
            Some("spawn_workers"),
            "cas__"
        ));
    }

    #[test]
    fn codemap_gate_does_not_match_wrong_prefix_or_action() {
        // A Grok tool_name must not match under a stale/wrong prefix guess.
        assert!(!is_codemap_gated_tool_call(
            "cas__task",
            Some("create"),
            "mcp__cas__"
        ));
        // Right tool, wrong action.
        assert!(!is_codemap_gated_tool_call(
            "cas__task",
            Some("list"),
            "cas__"
        ));
        // Unrelated tool.
        assert!(!is_codemap_gated_tool_call("Bash", Some("create"), "cas__"));
    }

    /// Sanity check that the full `handle_pre_tool_use` entrypoint reaches
    /// the codemap-gate matcher for a Grok supervisor's `cas__coordination`
    /// call at all (rather than short-circuiting on role/tool checks
    /// upstream) and, with no CODEMAP.md present to be stale against,
    /// doesn't false-positive deny. The matcher's actual gate/no-gate logic
    /// is proven by the dedicated unit tests above — this only guards the
    /// wiring between `own_tool_prefix()`, the env, and the real handler.
    #[test]
    fn grok_supervisor_codemap_gate_wiring_reaches_matcher_without_false_deny() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_FACTORY_SUPERVISOR_CLI", Some("grok")),
            ("CAS_FACTORY_WORKER_CLI", None),
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("cas__coordination".to_string());
        input.cwd = tmp.path().to_string_lossy().to_string();
        input.tool_input = Some(serde_json::json!({"action": "spawn_workers"}));

        let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(
            decision,
            Some("deny"),
            "no CODEMAP.md present → nothing to gate on, must not deny: {val}"
        );
    }

    #[test]
    fn task_verifier_spawn_uses_secret_free_server_handoff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_FACTORY_MODE", None),
            ("CAS_SESSION_ID", None),
            ("CAS_AGENT_ROLE", None),
        ]);
        let cas_root = crate::store::init_cas_dir(tmp.path()).expect("init cas");
        let agent_store = open_agent_store(&cas_root).expect("agent store");
        let task_store = open_task_store(&cas_root).expect("task store");
        let parent_id = "registered-parent";
        agent_store
            .register(&Agent::new(
                parent_id.to_string(),
                "standard-parent".to_string(),
            ))
            .expect("register parent");
        task_store
            .add(&Task::new(
                "cas-hook-capability".to_string(),
                "Hook authority".to_string(),
            ))
            .expect("task");

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.session_id = parent_id.to_string();
        input.cwd = tmp.path().to_string_lossy().to_string();
        input.tool_name = Some("Agent".to_string());
        input.tool_use_id = Some("tool-use-verifier-6939".to_string());
        input.tool_input = Some(serde_json::json!({
            "subagent_type": "task-verifier",
            "prompt": "Review CAS task cas-hook-capability"
        }));

        let denied = handle_pre_tool_use(&input, Some(&cas_root)).expect("pretool denial");
        let denied_value = serde_json::to_value(denied).expect("serialize denial");
        assert_eq!(
            denied_value["hookSpecificOutput"]["permissionDecision"], "deny",
            "verifier capability mint must fail closed without an active dispatch"
        );

        cas_store::create_verification_dispatch_bound(
            &cas_root,
            "cas-hook-capability",
            parent_id,
            parent_id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .expect("create proof-cycle dispatch");

        let mut missing_tool_use = input.clone();
        missing_tool_use.tool_use_id = None;
        let missing_output =
            handle_pre_tool_use(&missing_tool_use, Some(&cas_root)).expect("missing correlation");
        assert_eq!(
            serde_json::to_value(missing_output).expect("serialize missing correlation")["hookSpecificOutput"]
                ["permissionDecision"],
            "deny",
            "missing official PreToolUse tool_use_id must fail closed"
        );

        let sentinel = b"CAS_SENTINEL_RAW_VERIFIER_CREDENTIAL_6939";
        let output = with_test_verifier_handoff_secret(sentinel, || {
            handle_pre_tool_use(&input, Some(&cas_root)).expect("pretool")
        });
        let value = serde_json::to_value(output).expect("serialize output");
        assert_eq!(
            value,
            serde_json::json!({}),
            "successful verifier spawn must not emit updatedInput or context"
        );
        let original_input = serde_json::to_string(&input.tool_input).expect("serialize input");
        let sentinel_text = std::str::from_utf8(sentinel).expect("utf8 sentinel");
        assert!(!original_input.contains(sentinel_text));
        assert!(!value.to_string().contains(sentinel_text));

        let mut concurrent = input.clone();
        concurrent.tool_use_id = Some("tool-use-concurrent-6939".to_string());
        let concurrent_output =
            handle_pre_tool_use(&concurrent, Some(&cas_root)).expect("concurrent spawn denial");
        let concurrent_json =
            serde_json::to_value(concurrent_output).expect("serialize concurrent denial");
        assert_eq!(
            concurrent_json["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
        assert!(
            concurrent_json["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("already awaiting SubagentStart")),
            "concurrent denial must be actionable: {concurrent_json}"
        );
        assert!(!concurrent_json.to_string().contains(sentinel_text));

        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).expect("db");
        let (persisted_hash, tool_use_id_hash): (String, String) = conn
            .query_row(
                "SELECT c.token_hash, h.tool_use_id_hash
                 FROM verification_capabilities c
                 JOIN verification_handoffs h ON h.capability_id = c.id
                 WHERE c.task_id = ?1",
                rusqlite::params!["cas-hook-capability"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("capability row");
        assert!(!persisted_hash.contains(sentinel_text));
        assert!(!tool_use_id_hash.contains(sentinel_text));

        let wrong_type: crate::hooks::handlers::HookInput =
            serde_json::from_value(serde_json::json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent_id,
                "agent_id": "wrong-type-child",
                "agent_type": "Explore",
                "cwd": cas_root.clone(),
            }))
            .expect("wrong-type payload");
        crate::hooks::handlers::handle_subagent_start(&wrong_type, Some(&cas_root))
            .expect("wrong type ignored");
        assert!(
            agent_store.get("wrong-type-child").is_err(),
            "non-verifier child must never claim the sealed handoff"
        );

        let missing_child: crate::hooks::handlers::HookInput =
            serde_json::from_value(serde_json::json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent_id,
                "agent_type": "task-verifier",
                "cwd": cas_root.clone(),
            }))
            .expect("missing-child payload");
        let missing_child_output =
            crate::hooks::handlers::handle_subagent_start(&missing_child, Some(&cas_root))
                .expect("missing child denial");
        assert!(
            serde_json::to_value(missing_child_output)
                .expect("serialize missing child denial")
                .get("systemMessage")
                .is_some(),
            "missing official child agent_id must fail closed"
        );

        let other_parent_id = "unrelated-registered-parent";
        agent_store
            .register(&Agent::new(
                other_parent_id.to_string(),
                "unrelated-parent".to_string(),
            ))
            .expect("register unrelated parent");
        task_store
            .add(&Task::new(
                "cas-hook-unrelated".to_string(),
                "Unrelated hook authority".to_string(),
            ))
            .expect("unrelated task");
        cas_store::create_verification_dispatch_bound(
            &cas_root,
            "cas-hook-unrelated",
            other_parent_id,
            other_parent_id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .expect("create unrelated dispatch");
        let mut other_pre_tool = input.clone();
        other_pre_tool.session_id = other_parent_id.to_string();
        other_pre_tool.tool_use_id = Some("tool-use-unrelated-6939".to_string());
        other_pre_tool.tool_input = Some(serde_json::json!({
            "subagent_type": "task-verifier",
            "prompt": "Review CAS task cas-hook-unrelated"
        }));
        assert_eq!(
            serde_json::to_value(
                handle_pre_tool_use(&other_pre_tool, Some(&cas_root))
                    .expect("issue unrelated handoff")
            )
            .expect("serialize unrelated issuance"),
            serde_json::json!({})
        );
        let other_child_start: crate::hooks::handlers::HookInput =
            serde_json::from_value(serde_json::json!({
                "hook_event_name": "SubagentStart",
                "session_id": other_parent_id,
                "agent_id": "unrelated-verifier-child",
                "agent_type": "task-verifier",
                "cwd": cas_root,
            }))
            .expect("unrelated production-shaped child payload");
        assert_eq!(
            serde_json::to_value(
                crate::hooks::handlers::handle_subagent_start(&other_child_start, Some(&cas_root))
                    .expect("bind unrelated child")
            )
            .expect("serialize unrelated child output"),
            serde_json::json!({})
        );
        let unrelated_before: (String, String, String, String) = conn
            .query_row(
                "SELECT c.id, h.state, d.id, d.state
                 FROM verification_capabilities c
                 JOIN verification_handoffs h ON h.capability_id = c.id
                 JOIN verification_dispatches d ON d.id = c.dispatch_id
                 WHERE c.task_id = ?1 AND c.verifier_agent_id = ?2
                       AND d.verifier_agent_id = ?2",
                rusqlite::params!["cas-hook-unrelated", "unrelated-verifier-child"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("snapshot unrelated bound authority");
        assert_eq!(unrelated_before.1, "bound");
        assert_eq!(unrelated_before.3, "claimed");

        conn.execute_batch(
            "CREATE TRIGGER fail_verifier_child_registration
             BEFORE INSERT ON agents
             WHEN NEW.id = 'failed-verifier-child'
             BEGIN
                 SELECT RAISE(FAIL, 'injected verifier child registry failure');
             END;",
        )
        .expect("install registry failure injection");
        let failed_child_start: crate::hooks::handlers::HookInput =
            serde_json::from_value(serde_json::json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent_id,
                "agent_id": "failed-verifier-child",
                "agent_type": "task-verifier",
                "cwd": cas_root,
            }))
            .expect("failed production-shaped child payload");
        let failed_child_output =
            crate::hooks::handlers::handle_subagent_start(&failed_child_start, Some(&cas_root))
                .expect("failure-atomic child denial");
        let failed_child_json =
            serde_json::to_value(failed_child_output).expect("serialize child denial");
        assert!(
            failed_child_json.get("systemMessage").is_some(),
            "registry failure must fail closed: {failed_child_json}"
        );
        assert!(!failed_child_json.to_string().contains(sentinel_text));
        assert!(
            agent_store.get("failed-verifier-child").is_err(),
            "failed registry write must not leave a child row"
        );
        let failed_flow_state: (
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT c.verifier_agent_id, h.verifier_agent_id, h.state,
                        d.verifier_agent_id, d.capability_id, d.state
                 FROM verification_capabilities c
                 JOIN verification_handoffs h ON h.capability_id = c.id
                 JOIN verification_dispatches d ON d.id = c.dispatch_id
                 WHERE c.task_id = ?1",
                rusqlite::params!["cas-hook-capability"],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("failure-atomic authority state");
        assert_eq!(
            failed_flow_state,
            (
                None,
                None,
                "pending".to_string(),
                None,
                None,
                "pending".to_string()
            ),
            "registry failure must roll back capability, handoff, and dispatch together"
        );
        let unrelated_after: (String, String, String, String) = conn
            .query_row(
                "SELECT c.id, h.state, d.id, d.state
                 FROM verification_capabilities c
                 JOIN verification_handoffs h ON h.capability_id = c.id
                 JOIN verification_dispatches d ON d.id = c.dispatch_id
                 WHERE c.task_id = ?1 AND c.verifier_agent_id = ?2
                       AND d.verifier_agent_id = ?2",
                rusqlite::params!["cas-hook-unrelated", "unrelated-verifier-child"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("reload unrelated authority");
        assert_eq!(
            unrelated_after, unrelated_before,
            "rollback for one exact flow must not mutate another bound flow"
        );
        conn.execute_batch("DROP TRIGGER fail_verifier_child_registration")
            .expect("remove registry failure injection");

        let child_start: crate::hooks::handlers::HookInput =
            serde_json::from_value(serde_json::json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent_id,
                "agent_id": "registered-verifier-child",
                "agent_type": "task-verifier",
                "agent_transcript_path": "/portable/child.jsonl",
                "cwd": cas_root,
                "permission_mode": "default"
            }))
            .expect("deserialize official production-shaped SubagentStart payload");
        let child_output =
            crate::hooks::handlers::handle_subagent_start(&child_start, Some(&cas_root))
                .expect("bind child");
        assert_eq!(
            serde_json::to_value(child_output).expect("serialize child output"),
            serde_json::json!({}),
            "successful bind must not inject model-visible context"
        );

        let child = agent_store
            .get("registered-verifier-child")
            .expect("registered child");
        assert_eq!(child.agent_type, crate::types::AgentType::SubAgent);
        assert_eq!(child.role, AgentRole::Standard);
        assert_eq!(child.parent_id.as_deref(), Some(parent_id));
        let bound_to: String = conn
            .query_row(
                "SELECT verifier_agent_id FROM verification_capabilities WHERE task_id = ?1",
                rusqlite::params!["cas-hook-capability"],
                |row| row.get(0),
            )
            .expect("bound capability");
        assert_eq!(bound_to, child.id);
        let replay_output =
            crate::hooks::handlers::handle_subagent_start(&child_start, Some(&cas_root))
                .expect("replayed child start denial");
        assert!(
            serde_json::to_value(replay_output)
                .expect("serialize replay denial")
                .get("systemMessage")
                .is_some(),
            "an already-bound handoff must remain one-time"
        );

        let parent_transcript = tmp.path().join("parent.jsonl");
        let child_transcript = tmp.path().join("child.jsonl");
        std::fs::write(&parent_transcript, format!("{original_input}\n{value}\n"))
            .expect("parent transcript fixture");
        std::fs::write(
            &child_transcript,
            serde_json::to_string(&child).expect("child session payload"),
        )
        .expect("child transcript fixture");
        let mut pending = vec![tmp.path().to_path_buf()];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    std::fs::read_dir(&path)
                        .expect("scan observable surfaces")
                        .filter_map(|entry| entry.ok().map(|entry| entry.path())),
                );
            } else if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !bytes
                        .windows(sentinel.len())
                        .any(|window| window == sentinel),
                    "raw sentinel leaked into observable/persisted surface {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn verifier_spawn_terminal_cleanup_is_exact_and_unbound_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_FACTORY_MODE", None),
            ("CAS_SESSION_ID", None),
            ("CAS_AGENT_ROLE", None),
        ]);
        let cas_root = crate::store::init_cas_dir(tmp.path()).expect("init cas");
        let agent_store = open_agent_store(&cas_root).expect("agent store");
        let task_store = open_task_store(&cas_root).expect("task store");
        let parent_id = "cleanup-parent";
        agent_store
            .register(&Agent::new(
                parent_id.to_string(),
                "cleanup-parent".to_string(),
            ))
            .expect("register parent");
        task_store
            .add(&Task::new(
                "cas-hook-cleanup".to_string(),
                "Hook cleanup".to_string(),
            ))
            .expect("task");
        cas_store::create_verification_dispatch(
            &cas_root,
            "cas-hook-cleanup",
            parent_id,
            parent_id,
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .expect("dispatch");

        let mut input = crate::hooks::handlers::HookInput {
            session_id: parent_id.to_string(),
            cwd: tmp.path().to_string_lossy().to_string(),
            hook_event_name: "PreToolUse".to_string(),
            tool_name: Some("Agent".to_string()),
            tool_input: Some(serde_json::json!({
                "subagent_type": "task-verifier",
                "prompt": "Review cas-hook-cleanup",
            })),
            tool_use_id: Some("tool-use-cleanup".to_string()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(
                handle_pre_tool_use(&input, Some(&cas_root)).expect("issue handoff")
            )
            .expect("serialize"),
            serde_json::json!({})
        );

        let mut wrong = input.clone();
        wrong.hook_event_name = "PermissionDenied".to_string();
        wrong.tool_use_id = Some("tool-use-wrong".to_string());
        crate::hooks::handlers::handle_verifier_spawn_cleanup(&wrong, Some(&cas_root))
            .expect("wrong cleanup no-op");
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).expect("db");
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM verification_handoffs WHERE state = 'pending'",
                [],
                |row| row.get(0),
            )
            .expect("pending count");
        assert_eq!(pending, 1, "wrong tool_use_id must not remove the handoff");
        drop(conn);
        input.hook_event_name = "PostToolUseFailure".to_string();
        crate::hooks::handlers::handle_verifier_spawn_cleanup(&input, Some(&cas_root))
            .expect("exact failure cleanup");
        assert!(
            cas_store::bind_server_verifier_handoff(&cas_root, parent_id, "child-after-failure")
                .is_err(),
            "exact failed-spawn cleanup must remove the still-unbound handoff"
        );
    }
}
