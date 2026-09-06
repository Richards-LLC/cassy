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
    // WORKER COMMAND-SCOPE GUARDS (cas-eb39, cas-852a0)
    //
    // A mutating `cargo fmt` selects Cargo targets rather than source paths,
    // and direct rustfmt follows child modules unless skip_children is set.
    // In a workspace that is not fmt-clean, either shape spills unrelated
    // changes. A full test invocation similarly links dozens of binaries.
    // Workers use non-mutating/scoped format commands, iterate with cargo check,
    // and run a test target through the receipt wrapper; the supervisor
    // integration merge and release gate own full-suite runs.
    // Hoist this before the cas_root early return and factory Bash auto-allow so
    // an unscoped run always gets the loud, actionable refusal.
    // ========================================================================
    if is_factory_agent && crate::harness_policy::is_worker(input) && tool_name == "Bash" {
        let command = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get("command"))
            .and_then(|command| command.as_str());
        if command.is_some_and(worker_command_runs_dangerous_formatter) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "🚫 UNSCOPED WORKER FORMAT RUN: this repository is not workspace-rustfmt-clean, and rustfmt follows child modules by default. Either command can rewrite hundreds of unrelated files.\n\n\
                 Use a non-mutating check:\n  \
                 `cargo fmt --all -- --check`\n  \
                 `rustfmt --edition 2024 --check --config skip_children=true <task-files>`\n\n\
                 To format task files, make recursion explicit:\n  \
                 `rustfmt --edition 2024 --config skip_children=true <task-files>`\n\n\
                 A workspace normalization requires separate operator approval and must not be run from a worker.",
            ));
        }
        if command.is_some_and(worker_command_runs_unguarded_tests) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "🚫 UNVERIFIED WORKER TEST RUN: Cargo can exit 0 when a filter selected zero tests, so direct Cargo test output is not a verification receipt.\n\n\
                 Iterate with `cargo check -p cas --lib --tests`, then run the affected target through the guarded recipe:\n  \
                 `scripts/run-scoped-tests.sh -p cas --lib <module>`\n  \
                 `scripts/run-scoped-tests.sh -p cas --test <name>`\n\n\
                 The wrapper requires a nonzero passed count. Full-suite runs are reserved for the supervisor integration merge and the release gate.",
            ));
        }
    }

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
    // Placed before the cas_root check so the gate fires even if Cassy isn't
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
    // resolve a Cassy root.
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
    // WORKER COMMIT GUARD — HOISTED ABOVE cas_root check (cas-bea2, LAYER 1)
    //
    // Must run before the hoisted FACTORY_AUTO_APPROVE block below. That
    // block returns "allow" for all Bash tool calls when cas_root=None,
    // which would bypass this guard. Placing it here ensures it fires on
    // both the cas_root=None and cas_root=Some paths.
    //
    // Intercepts `git commit` / `git merge` / `git push` from ALL factory workers
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
                if looks_like_git_push_to_origin(cmd)
                    && worker_delivery_mode() == cas_types::DeliveryMode::LocalMerge
                    && !local_merge_push_override()
                {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "🚫 LOCAL-MERGE DELIVERY: git push origin is disabled for this factory session. Commit locally; the supervisor merges your local factory branch. Set CAS_FACTORY_LOCAL_MERGE_PUSH_OVERRIDE=1 only when explicitly authorized by the supervisor.",
                    ));
                }
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
    // report where a supervisor session runs the hook without a Cassy root
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

    // Check if Cassy is initialized
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Create shared store cache — all store accesses below go through this
    // instead of calling open_*() directly, reducing ~11 SQLite connections to ~3-4.
    let mut stores = ToolHookStores::new(cas_root);

    // Durable workspace contract (GH #196, GH #528). Factory file creation is
    // intentionally narrow: the checked-out worktree, a configured durable
    // artifacts root, an optional configured scratch root, and the
    // harness-provided scratchpad are sanctioned.
    // Supervisors may also write the harness's per-project file-memory tree;
    // workers remain restricted to the original roots.
    // A scratchpad may itself be under /tmp, but it is explicitly ephemeral
    // and is rejected later if cited as close evidence.
    if is_factory_agent {
        // `input.cwd` is the tool's current directory, not the worker's
        // registered checkout. Claude can report a nested cwd (and it may
        // change during a long session), so resolve the durable binding from
        // the current agent row on every hook invocation. The environment is
        // the bootstrap fallback until SessionStart has registered the row.
        let registered_worktree = registered_factory_worktree_root(&mut stores, input);
        let (artifacts_root, scratch_root) = {
            let config = stores.config();
            let factory = config.factory();
            (
                factory.artifacts_root.clone(),
                config
                    .staging
                    .as_ref()
                    .and_then(|staging| staging.scratch_root.clone()),
            )
        };
        if let Some(violation) = factory_write_violation(
            input,
            &artifacts_root,
            scratch_root.as_deref(),
            crate::harness_policy::is_supervisor(input),
            registered_worktree.as_deref(),
        ) {
            let path = &violation.resolved_path;
            log_factory_workspace_rejection(cas_root, input, &violation);
            let artifacts =
                crate::config::resolved_factory_artifacts_root(artifacts_root.as_deref());
            let scratch = scratch_root
                .as_deref()
                .map(|root| format!(" or `{root}/...` for ephemeral scratch output"))
                .unwrap_or_default();
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &format!(
                    "🚫 FACTORY WORKSPACE CONTRACT: file creation outside the worktree, durable artifacts root, configured scratch root, or harness exceptions is blocked: {}. Use your worktree, `{}/<task-id>/` for durable proof{}; only this session's harness scratchpad is sanctioned for ephemeral notes. Bare /tmp and stray $HOME files are not sanctioned.",
                    path.display(),
                    artifacts.display(),
                    scratch
                ),
            ));
        }
    }

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
    // FACTORY MODE: Auto-route SendMessage → Cassy coordination (cas-f32b)
    //
    // In factory mode, agents communicate through Cassy coordination (push-based
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
    // Cassy prompt queue directly (same path `mcp__cas__coordination
    // action=message` uses), notify the daemon, then return `allow` with an
    // `additionalContext` success receipt (cas-73c8) so agents see tool
    // success — not a deny/`<error>` envelope — and stop retrying.
    //
    // On any failure (missing fields, queue open error, enqueue error) we
    // fall back to the original deny-with-guidance path — never silently drop.
    // ========================================================================
    // `is_factory_agent` already computed above for the hoisted
    // cas_root=None auto-approve check (cas-7f33).
    if is_factory_agent {
        // cas-7aa2 (GH #176): the auto-route below returns `allow`, so the
        // harness's native SendMessage runs too and writes a second copy into
        // THIS process's config-dir teams tree. When that tree is not the one
        // the factory daemon delivers into (a worker spawned with an explicit
        // `config_dir`), the copy is a dead letter that nothing consumes and
        // retention never prunes. Sweep it inert here rather than in the
        // auto-route itself: the native write happens AFTER the hook returns,
        // so the only seam that can see it is a LATER hook invocation. This
        // event fires on every intercepted tool call, so the stray is
        // neutralised within a call or two of being written. Cheap: two stat()
        // calls in the overwhelmingly common same-tree case, and a no-op
        // whenever `config.json` says this IS the factory's tree.
        reap_stranded_native_send_message_copies();
    }
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

    // Factory workers manage their own worktrees — skip Cassy worktree enforcement
    // to avoid conflicting redirects (factory uses per-worker worktrees, Cassy uses per-epic)
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
    //
    // Factory settings install PreToolUse without the `SubagentStart` binding
    // half. A handoff minted for a factory agent's `Agent` spawn could never
    // bind and would deny that parent's next verifier spawn with "already
    // awaiting SubagentStart" until expiry. Keep factory `Agent` excluded;
    // generated solo settings install both halves atomically.
    if matches!(tool_name, "Task" | "Agent")
        && !(tool_name == "Agent" && is_factory_agent)
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
                "task-verifier spawn requires a prompt naming exactly one Cassy task.",
            ));
        };
        let prompt = tool_input
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let Some(task_id) = unique_existing_task_id(prompt, stores.tasks()) else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "task-verifier prompt must name exactly one existing Cassy task ID.",
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
    // Cassy gives the supervisor agentId `supervisor@<team>` and workers
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
    // early return to rescue factory sessions where Cassy isn't initialized
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

/// Detect a worker shell command that executes Cargo tests without the
/// zero-executed receipt wrapper.  A target scope controls cost but does not
/// prove the filter matched anything, because Cargo exits zero for zero tests.
fn worker_command_runs_unguarded_tests(command: &str) -> bool {
    super::attribution::split_shell_statements(command)
        .iter()
        .any(|words| direct_test_invocation_without_receipt(words))
}

/// Detect formatter invocations that can mutate files outside a worker's scope.
///
/// `cargo fmt` always selects Cargo targets rather than individual source files,
/// while direct `rustfmt` follows `mod` declarations unless `skip_children` is
/// enabled. Checks and stdout-only runs are non-mutating and remain available.
fn worker_command_runs_dangerous_formatter(command: &str) -> bool {
    super::attribution::split_shell_statements(command)
        .iter()
        .any(|words| formatter_invocation_can_spill(words))
}

fn formatter_invocation_can_spill(words: &[String]) -> bool {
    if let Some(cargo_index) = words
        .iter()
        .position(|word| word == "cargo" || word.ends_with("/cargo"))
    {
        let mut cargo_args = &words[cargo_index + 1..];
        if cargo_args.first().is_some_and(|arg| arg.starts_with('+')) {
            cargo_args = &cargo_args[1..];
        }
        if cargo_args.first().is_some_and(|arg| arg == "fmt") {
            let is_read_only = cargo_args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--check" | "--help" | "-h" | "--version" | "-V"
                )
            });
            return !is_read_only;
        }
    }

    let Some(rustfmt_index) = words
        .iter()
        .position(|word| word == "rustfmt" || word.ends_with("/rustfmt"))
    else {
        return false;
    };
    let rustfmt_args = &words[rustfmt_index + 1..];
    let is_read_only = rustfmt_args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--check" | "--help" | "-h" | "--version" | "-V" | "--print-config"
        ) || arg == "--emit=stdout"
    }) || rustfmt_args
        .windows(2)
        .any(|pair| pair[0] == "--emit" && pair[1] == "stdout");
    if is_read_only {
        return false;
    }

    let skips_children = rustfmt_args
        .iter()
        .any(|arg| arg == "--config=skip_children=true" || arg.contains("skip_children=true"));
    !skips_children
}

fn direct_test_invocation_without_receipt(words: &[String]) -> bool {
    let Some(cargo_index) = words
        .iter()
        .position(|word| word == "cargo" || word.ends_with("/cargo"))
    else {
        return false;
    };
    let cargo_args = &words[cargo_index + 1..];
    let is_test = cargo_args.first().is_some_and(|arg| arg == "test")
        || (cargo_args.first().is_some_and(|arg| arg == "nextest")
            && cargo_args.get(1).is_some_and(|arg| arg == "run"));
    if !is_test {
        return false;
    }

    // `--no-run` intentionally compiles test targets without claiming tests
    // passed. It is not a test-execution receipt and remains available.
    !cargo_args.iter().any(|arg| arg == "--no-run")
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

/// Return true if `cmd` looks like a `git commit`, `git merge`, or `git push`
/// invocation.
///
/// Matches common forms:
/// - `git commit -m "msg"`
/// - `git -C /some/path commit`
/// - `git merge main`
/// - `git push origin HEAD:refs/heads/factory/my-worker`
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
            // We look for a guarded write as a word anywhere after "git".
            return after_git
                .split_whitespace()
                .any(|tok| matches!(tok, "commit" | "merge" | "push"));
        }
        // Not a word boundary — advance past this occurrence
        rest = &rest[pos + 1..];
    }
}

/// Return true when a shell command invokes `git push origin`.
///
/// The local-merge route only blocks publication to the configured origin;
/// local commits and other git writes remain governed by the existing branch
/// and worktree guards.
pub(crate) fn looks_like_git_push_to_origin(cmd: &str) -> bool {
    let mut saw_git = false;
    let mut saw_push = false;
    for token in cmd.split_whitespace() {
        let token = token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | ';' | '&' | '|'));
        if !saw_git {
            if token == "git" {
                saw_git = true;
            }
            continue;
        }
        if !saw_push {
            if token == "push" {
                saw_push = true;
            }
            continue;
        }
        if token == "origin" {
            return true;
        }
    }
    false
}

/// Read the route selected for the current factory session. Missing or
/// malformed metadata intentionally falls back to the legacy push route.
pub(crate) fn worker_delivery_mode() -> cas_types::DeliveryMode {
    let Some(session) = std::env::var_os("CAS_FACTORY_SESSION") else {
        return cas_types::DeliveryMode::PushBranch;
    };
    let session = session.to_string_lossy();
    let data = std::fs::read_to_string(crate::ui::factory::metadata_path(&session)).ok();
    data.and_then(|data| {
        serde_json::from_str::<crate::ui::factory::SessionMetadata>(&data)
            .ok()
            .map(|metadata| metadata.delivery_mode)
    })
    .unwrap_or_default()
}

fn local_merge_push_override() -> bool {
    std::env::var("CAS_FACTORY_LOCAL_MERGE_PUSH_OVERRIDE")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
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

/// Check whether a factory worker's `git commit` / `git merge` / `git push`
/// should be denied.
///
/// Returns `Some(denial_message)` when:
/// - HEAD at `cwd` is a protected branch (`main`, `master`, `staging`) or detached.
/// - `CAS_CLONE_PATH` is set (isolated worker) AND `cwd` is outside the worktree.
///
/// Returns `None` to allow when HEAD is on the worker's own factory branch.
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

    // Resolve HEAD once. Protected branches and detached HEAD retain the
    // established WORKER COMMIT GUARD contract before identity-specific
    // checks run. Besides keeping the message stable, this preserves the
    // shared-checkout remedy (worktree first, restore trunk on fallback).
    let worker_name =
        std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());

    let branch = match get_branch_at_cwd(cwd) {
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
        Some(branch) => branch,
    };

    // DENY: a shared, non-isolated checkout is not on the exact branch this
    // worker owns (cas-0efb). The earlier sibling-only comparison still
    // allowed a worker to push `HEAD:refs/heads/factory/<mine>` while the
    // shared checkout sat on a feature/epic branch. Isolated worktrees keep
    // the established denylist semantics: feature/epic branches are valid
    // because CAS_CLONE_PATH already proves checkout ownership, while a
    // sibling factory branch remains an identity mismatch (cas-30c6).
    //
    // The denylist above only protects the trunk, and `factory/*` is an
    // allowed prefix (cas-7e7b), so a worker whose harness was bound to a
    // sibling's worktree passed every check and could commit onto that
    // sibling's branch — respawn 1034. Resolve the canonical binding from the
    // worker's own registered identity instead; `my_context` reports the same
    // classification from the same module, so the two never disagree.
    //
    // Only fires when the worker's identity is actually known: without
    // CAS_AGENT_NAME there is no canonical branch to compare against, and
    // guessing would deny every legitimate `factory/*` commit.
    if let Some(registered_name) = std::env::var("CAS_AGENT_NAME")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
    {
        match crate::factory_isolation::classify_worker_binding(&registered_name, Some(&branch)) {
            crate::factory_isolation::WorkerBinding::Own => {}
            crate::factory_isolation::WorkerBinding::Sibling { owner } => {
                return Some(crate::factory_isolation::sibling_misbinding_message(
                    &registered_name,
                    &owner,
                    cwd,
                ));
            }
            crate::factory_isolation::WorkerBinding::Other if !is_isolated => {
                let expected = crate::factory_isolation::expected_worker_branch(&registered_name);
                return Some(format!(
                    "🚫 WORKER BRANCH MISMATCH: you are '{registered_name}', but {cwd} is \
                     checked out on '{branch}'.\n\nCommits, merges, and pushes are permitted only \
                     from your own '{expected}' branch. A shared checkout can change HEAD between \
                     tool calls; pushing from whatever branch happens to be current can graft \
                     another worker's commits onto '{expected}'.\n\nMove to the worktree that owns \
                     '{expected}', verify `git rev-parse --abbrev-ref HEAD` prints that exact \
                     branch, then retry. If this is a non-isolated worker in the shared checkout, \
                     stop and ask the supervisor to respawn it with isolate=true."
                ));
            }
            crate::factory_isolation::WorkerBinding::SharedTrunk => {
                unreachable!("protected trunk branches return before identity classification")
            }
            crate::factory_isolation::WorkerBinding::Other => {}
        }
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

#[derive(Debug, PartialEq, Eq)]
enum ShellToken {
    Word(String),
    Operator(char),
}

/// Split just enough shell syntax to distinguish command arguments from
/// redirections. In particular, metacharacters inside a quoted commit message
/// stay inside one word and can never be mistaken for file-creation syntax.
fn factory_shell_tokens(command: &str) -> Vec<ShellToken> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;

    let push_word = |tokens: &mut Vec<ShellToken>, word: &mut String| {
        if !word.is_empty() {
            tokens.push(ShellToken::Word(std::mem::take(word)));
        }
    };

    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => word.push(ch),
            None => match ch {
                '\\' => escaped = true,
                '\'' | '"' => quote = Some(ch),
                '>' | '<' | '|' | '&' | ';' | '(' | ')' => {
                    push_word(&mut tokens, &mut word);
                    tokens.push(ShellToken::Operator(ch));
                }
                '\n' | '\r' => {
                    push_word(&mut tokens, &mut word);
                    tokens.push(ShellToken::Operator(';'));
                }
                ch if ch.is_whitespace() => push_word(&mut tokens, &mut word),
                _ => word.push(ch),
            },
        }
    }
    if escaped {
        word.push('\\');
    }
    push_word(&mut tokens, &mut word);
    tokens
}

/// Collect the finite values that are visible in the simple shell forms used
/// by factory workers (`NAME=value` and `for NAME in ...`). This is not a
/// general shell evaluator: an unrecognised or unbound variable remains in the
/// returned path and therefore fails the workspace containment check closed.
fn factory_shell_variable_values(
    tokens: &[ShellToken],
) -> std::collections::HashMap<String, Vec<String>> {
    let mut values = std::collections::HashMap::new();
    if let Ok(home) = std::env::var("HOME") {
        values.insert("HOME".to_string(), vec![home]);
    }
    for token in tokens {
        let ShellToken::Word(word) = token else {
            continue;
        };
        let Some((name, value)) = word.split_once('=') else {
            continue;
        };
        if is_shell_variable_name(name) {
            values.insert(name.to_string(), vec![value.to_string()]);
        }
    }

    let mut index = 0;
    while index + 3 < tokens.len() {
        let Some(ShellToken::Word(keyword)) = tokens.get(index) else {
            index += 1;
            continue;
        };
        if keyword != "for" {
            index += 1;
            continue;
        }
        let (Some(ShellToken::Word(name)), Some(ShellToken::Word(in_keyword))) =
            (tokens.get(index + 1), tokens.get(index + 2))
        else {
            index += 1;
            continue;
        };
        if !is_shell_variable_name(name) || in_keyword != "in" {
            index += 1;
            continue;
        }
        let mut loop_values = Vec::new();
        let mut cursor = index + 3;
        while cursor < tokens.len() {
            match tokens.get(cursor) {
                Some(ShellToken::Word(word)) if word == "do" => break,
                Some(ShellToken::Operator(';')) | Some(ShellToken::Operator('&')) => break,
                Some(ShellToken::Word(word)) => loop_values.push(word.clone()),
                _ => {}
            }
            cursor += 1;
        }
        if !loop_values.is_empty() {
            values.insert(name.clone(), loop_values);
        }
        index = cursor;
    }
    values
}

fn is_shell_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Expand only variables whose values are finite and explicit in the command.
/// The bounded Cartesian product prevents a crafted command from making the
/// hook allocate without limit; retaining the original word on overflow keeps
/// the containment check fail-closed.
fn expand_factory_shell_word(
    word: &str,
    values: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    const MAX_EXPANSIONS: usize = 64;
    let chars: Vec<char> = word.chars().collect();
    let mut expanded = vec![String::new()];
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '$' {
            for value in &mut expanded {
                value.push(chars[index]);
            }
            index += 1;
            continue;
        }
        let (end, name) = if chars.get(index + 1) == Some(&'{') {
            let Some(close) = chars[index + 2..].iter().position(|ch| *ch == '}') else {
                for value in &mut expanded {
                    value.push('$');
                }
                index += 1;
                continue;
            };
            let end = index + 2 + close;
            (end + 1, chars[index + 2..end].iter().collect::<String>())
        } else {
            let mut end = index + 1;
            while end < chars.len() && (chars[end] == '_' || chars[end].is_ascii_alphanumeric()) {
                end += 1;
            }
            if end == index + 1 {
                for value in &mut expanded {
                    value.push('$');
                }
                index += 1;
                continue;
            }
            (end, chars[index + 1..end].iter().collect::<String>())
        };
        let Some(replacements) = values.get(&name) else {
            for value in &mut expanded {
                value.extend(chars[index..end].iter().copied());
            }
            index = end;
            continue;
        };
        if replacements.is_empty()
            || expanded.len().saturating_mul(replacements.len()) > MAX_EXPANSIONS
        {
            return vec![word.to_string()];
        }
        let mut next = Vec::with_capacity(expanded.len() * replacements.len());
        for prefix in &expanded {
            for replacement in replacements {
                next.push(format!("{prefix}{replacement}"));
            }
        }
        expanded = next;
        index = end;
    }
    expanded
}

/// Extract the file argument from the narrow Python heredoc rewrite shape
/// emitted by factory workers. A heredoc's body is opaque to the shell-token
/// recognizer above, but `open(path, 'w')` is still a real write target and
/// must remain inside the factory workspace contract. Unknown expressions are
/// ignored here and remain subject to Claude's own permission classifier.
fn bash_heredoc_write_targets(command: &str) -> Vec<String> {
    if !command.contains("<<") {
        return Vec::new();
    }
    let mut assignments = std::collections::HashMap::new();
    for line in command.lines() {
        let trimmed = line.trim();
        let Some(equal) = trimmed.find('=') else {
            continue;
        };
        let name = trimmed[..equal].trim();
        let value = trimmed[equal + 1..].trim();
        if is_shell_variable_name(name) {
            if let Some(quoted) = quoted_string_value(value) {
                assignments.insert(name.to_string(), quoted.to_string());
            }
        }
    }

    let mut targets = Vec::new();
    let mut remainder = command;
    while let Some(open) = remainder.find("open(") {
        let args = &remainder[open + "open(".len()..];
        let first = args.trim_start();
        let (target, consumed) = if let Some(quoted) = quoted_string_value(first) {
            (Some(quoted.to_string()), quoted.len() + 2)
        } else {
            let end = first
                .find(|ch: char| ch == ',' || ch == ')' || ch.is_whitespace())
                .unwrap_or(first.len());
            let name = &first[..end];
            (assignments.get(name).cloned(), end)
        };
        if let Some(target) = target {
            targets.push(target);
        }
        let advance =
            (open + "open(".len() + first.len().min(consumed.max(1))).min(remainder.len());
        remainder = &remainder[advance..];
    }
    targets
}

fn quoted_string_value(value: &str) -> Option<&str> {
    let mut chars = value.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let (end, _) = chars.find(|(_, ch)| *ch == quote)?;
    Some(&value[1..end])
}

/// Identify shell words that are actual output targets, without treating every
/// path-shaped argument as a write. This deliberately handles only the small
/// set of write forms guarded by the factory workspace contract; unrecognised
/// shell syntax is left to the shell rather than guessed at.
fn bash_write_targets(command: &str) -> Vec<String> {
    let tokens = factory_shell_tokens(command);
    let variable_values = factory_shell_variable_values(&tokens);
    let mut targets = Vec::new();

    let mut add_target = |target: &str| {
        targets.extend(expand_factory_shell_word(target, &variable_values));
    };

    // A `>` outside quotes always names its destination in the next shell
    // word. Repeated `>` tokens cover append redirections, while the numeric
    // descriptor in `2>/tmp/log` remains an unrelated word.
    for (index, token) in tokens.iter().enumerate() {
        if token == &ShellToken::Operator('>') {
            if let Some(ShellToken::Word(target)) = tokens.get(index + 1) {
                add_target(target);
            }
        }
    }
    for target in bash_heredoc_write_targets(command) {
        add_target(&target);
    }

    // Cover the explicit file-creation commands guarded by the original
    // workspace contract. Only a command-position word is considered; prose
    // passed to `git commit -m` or `git merge -m` is never reinterpreted as a
    // command. This is intentionally a small recognizer, not a shell parser.
    let mut index = 0;
    let mut command_position = true;
    while index < tokens.len() {
        match &tokens[index] {
            ShellToken::Operator(';')
            | ShellToken::Operator('|')
            | ShellToken::Operator('&')
            | ShellToken::Operator('(')
            | ShellToken::Operator(')') => {
                command_position = true;
                index += 1;
            }
            ShellToken::Operator(_) => index += 1,
            ShellToken::Word(command) if command_position => {
                // `do`/`then` introduce a new command position without an
                // operator token. Recognising them lets the small parser see
                // commands inside `for` loops and shell conditionals.
                if matches!(command.as_str(), "do" | "then" | "else" | "elif") {
                    command_position = true;
                    index += 1;
                    continue;
                }
                // Shell prefixes do not occupy command position: `env X=1
                // touch /tmp/x`, `sudo touch /tmp/x`, and `command touch
                // /tmp/x` still execute the creation command that follows.
                if command.starts_with('-')
                    || command.contains('=')
                    || matches!(command.as_str(), "command" | "env" | "sudo")
                {
                    index += 1;
                    continue;
                }
                command_position = false;
                let command = std::path::Path::new(command)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(command);
                if !matches!(command, "touch" | "mkdir" | "tee" | "cp" | "mv" | "rm") {
                    index += 1;
                    continue;
                }

                let mut operands = Vec::new();
                let mut cursor = index + 1;
                while cursor < tokens.len() {
                    match &tokens[cursor] {
                        ShellToken::Operator(';')
                        | ShellToken::Operator('|')
                        | ShellToken::Operator('&')
                        | ShellToken::Operator('(')
                        | ShellToken::Operator(')') => break,
                        ShellToken::Operator(_) => {}
                        ShellToken::Word(word) if !word.starts_with('-') => {
                            operands.push(word.clone())
                        }
                        ShellToken::Word(_) => {}
                    }
                    cursor += 1;
                }
                if matches!(command, "cp" | "mv") {
                    if let Some(destination) = operands.pop() {
                        add_target(&destination);
                    }
                } else {
                    for operand in operands {
                        add_target(&operand);
                    }
                }
                index = cursor;
            }
            ShellToken::Word(_) => {
                command_position = false;
                index += 1;
            }
        }
    }
    targets
}

/// Claude Code advertises this exact per-session ephemeral root to agents:
/// `/tmp/claude-<uid>/<project-slug>/<session-id>/scratchpad/...`.
/// Bind the exemption to the hook's own session ID and reject traversal so it
/// cannot become a general `/tmp` escape hatch.
fn is_harness_session_scratchpad(path: &std::path::Path, session_id: &str) -> bool {
    use std::path::Component;

    let mut components = path.components();
    if components.next() != Some(Component::RootDir)
        || components.next() != Some(Component::Normal(std::ffi::OsStr::new("tmp")))
    {
        return false;
    }
    let Some(Component::Normal(claude_user)) = components.next() else {
        return false;
    };
    let Some(uid) = claude_user
        .to_str()
        .and_then(|value| value.strip_prefix("claude-"))
    else {
        return false;
    };
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next() != Some(Component::Normal(std::ffi::OsStr::new(session_id)))
        || components.next() != Some(Component::Normal(std::ffi::OsStr::new("scratchpad")))
    {
        return false;
    }

    // The sanctioned root is for files below `scratchpad`, not the directory
    // entry itself, and no `..` component may escape it.
    let remaining: Vec<_> = components.collect();
    !remaining.is_empty()
        && remaining
            .iter()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn factory_unsanctioned_write_path(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
) -> Option<std::path::PathBuf> {
    factory_write_violation(
        input,
        configured_artifacts_root,
        configured_scratch_root,
        is_supervisor,
        None,
    )
    .map(|violation| violation.resolved_path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FactoryWriteViolation {
    evaluated_path: String,
    resolved_path: std::path::PathBuf,
    matched_rule: &'static str,
}

fn factory_write_violation(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
    registered_worktree_root: Option<&std::path::Path>,
) -> Option<FactoryWriteViolation> {
    let tool = input.tool_name.as_deref()?;
    let tool_input = input.tool_input.as_ref()?;
    let raw_paths = match tool {
        "Write" | "Edit" | "NotebookEdit" => tool_input
            .get("file_path")
            .or_else(|| tool_input.get("path"))
            .and_then(|value| value.as_str())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
        "Bash" => {
            let command = tool_input.get("command").and_then(|value| value.as_str())?;
            bash_write_targets(command)
        }
        _ => return None,
    };

    raw_paths.into_iter().find_map(|raw_path| {
        // A variable that survived the finite expansion above may resolve to
        // an absolute path only when Bash runs it. Treating the literal
        // `$NAME` as relative to the worktree would create an escape hatch.
        // Known `$HOME` and command-local assignment/loop variables have
        // already been expanded; the remainder is deliberately fail-closed.
        if raw_path.contains('$') {
            return Some(FactoryWriteViolation {
                evaluated_path: raw_path.clone(),
                resolved_path: std::path::PathBuf::from(&raw_path),
                matched_rule: "unresolved shell variable",
            });
        }
        unsanctioned_factory_path_with_worktree(
            input,
            configured_artifacts_root,
            configured_scratch_root,
            is_supervisor,
            &raw_path,
            registered_worktree_root,
        )
        .map(|resolved_path| FactoryWriteViolation {
            evaluated_path: raw_path,
            resolved_path,
            matched_rule: "none",
        })
    })
}

fn unsanctioned_factory_path(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
    raw_path: &str,
) -> Option<std::path::PathBuf> {
    unsanctioned_factory_path_with_worktree(
        input,
        configured_artifacts_root,
        configured_scratch_root,
        is_supervisor,
        raw_path,
        None,
    )
}

fn unsanctioned_factory_path_with_worktree(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
    raw_path: &str,
    registered_worktree_root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let env_worktree_root = std::env::var_os("CAS_CLONE_PATH")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let worktree_root = registered_worktree_root
        .map(std::path::Path::to_path_buf)
        .or(env_worktree_root);
    // Once a durable clone binding exists, cwd is only a tool location and
    // must not widen the contract. Keep cwd as the standalone-worker fallback
    // for sessions that have no registered or environment-provided root.
    let mut sanctioned =
        vec![worktree_root.unwrap_or_else(|| std::path::PathBuf::from(&input.cwd))];
    sanctioned.push(crate::config::resolved_factory_artifacts_root(
        configured_artifacts_root.as_deref(),
    ));
    if let Some(scratch_root) = configured_scratch_root.filter(|root| !root.trim().is_empty()) {
        sanctioned.push(std::path::PathBuf::from(scratch_root));
    }
    for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
        if let Some(value) = std::env::var_os(key) {
            sanctioned.push(std::path::PathBuf::from(value));
        }
    }

    let raw_path = if let Some(suffix) = raw_path.strip_prefix("~/") {
        home.as_ref()?.join(suffix)
    } else if let Some(suffix) = raw_path.strip_prefix("$HOME/") {
        home.as_ref()?.join(suffix)
    } else {
        std::path::PathBuf::from(raw_path)
    };
    let path = if raw_path.is_absolute() {
        lexically_normalize_path(raw_path)
    } else {
        lexically_normalize_path(std::path::PathBuf::from(&input.cwd).join(raw_path))
    };
    // A path whose symlink chain cannot be resolved is not safely attributable
    // to any sanctioned root. Fail closed rather than letting `None` mean
    // "allowed"; the narrow stream-device exception is handled first.
    if is_non_creation_stream_device(&path) {
        return None;
    }
    let Some(resolved_path) = canonicalize_for_containment(&path) else {
        return Some(path);
    };
    let is_explicitly_sanctioned = sanctioned.iter().any(|root| {
        canonicalize_for_containment(&lexically_normalize_path(root.clone()))
            .is_some_and(|root| resolved_path.starts_with(root))
    });
    if is_harness_session_scratchpad(&resolved_path, &input.session_id)
        || (is_supervisor && is_harness_file_memory_path(&resolved_path, home.as_deref()))
        || is_explicitly_sanctioned
    {
        return None;
    }
    Some(resolved_path)
}

/// Resolve the worker's registered checkout for each hook call.
///
/// SessionStart persists `CAS_CLONE_PATH` in the current agent's metadata.
/// Reading that row here makes a mid-session registration refresh visible to
/// PreToolUse immediately. The environment remains the bootstrap fallback for
/// the short interval before the first registration succeeds.
fn registered_factory_worktree_root(
    stores: &mut ToolHookStores<'_>,
    input: &HookInput,
) -> Option<std::path::PathBuf> {
    let agent_id = current_agent_id(input);
    stores
        .agents()
        .and_then(|store| store.get(&agent_id).ok())
        .and_then(|agent| agent.metadata.get("clone_path").cloned())
        .filter(|path| !path.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("CAS_CLONE_PATH")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
        })
}

/// Canonicalize a path for containment checks even when its final components
/// do not exist yet. The nearest existing ancestor carries symlink semantics;
/// the missing suffix is then appended. Dangling symlinks fail closed because
/// they are reported by `symlink_metadata` but cannot be canonicalized.
fn canonicalize_for_containment(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let normalized = lexically_normalize_path(path.to_path_buf());
    let mut existing = normalized.clone();
    let mut missing = Vec::new();
    while !existing.exists() {
        if std::fs::symlink_metadata(&existing).is_ok() {
            break;
        }
        let name = existing.file_name()?.to_os_string();
        missing.push(name);
        existing.pop();
    }

    let mut canonical = existing.canonicalize().ok()?;
    for component in missing.into_iter().rev() {
        canonical.push(component);
    }
    Some(canonical)
}

fn log_factory_workspace_rejection(
    cas_root: &Path,
    input: &HookInput,
    violation: &FactoryWriteViolation,
) {
    let tool = input.tool_name.as_deref().unwrap_or("unknown");
    let payload_bytes = input
        .tool_input
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|payload| payload.len().to_string())
        .unwrap_or_else(|| "0".to_string());
    let resolved_path = violation.resolved_path.display().to_string();
    let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
        cas_root,
        "workspace_contract_rejection",
        &[
            ("tool", tool),
            ("evaluated_path", violation.evaluated_path.as_str()),
            ("resolved_path", resolved_path.as_str()),
            ("matched_rule", violation.matched_rule),
            ("payload_bytes", payload_bytes.as_str()),
        ],
    );
}

/// Normalize `.` and `..` without requiring the target to exist. Write
/// guardrails decide before creation, so `canonicalize` would both fail for
/// the common case and make a configured root vulnerable to `root/../escape`.
fn lexically_normalize_path(path: std::path::PathBuf) -> std::path::PathBuf {
    use std::path::Component;

    let rooted = matches!(
        path.components().next(),
        Some(Component::RootDir | Component::Prefix(_))
    );
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                } else if !rooted {
                    normalized.push(component.as_os_str());
                }
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// The Claude harness persists its direct file memory per project below
/// `~/.claude/projects/<project>/memory`, or an account-specific sibling such
/// as `~/.claude-work/projects/<project>/memory`.  This is a narrow
/// supervisor-only workspace-contract exception: all components after HOME
/// are fixed except the account suffix, project slug, and files below memory.
/// Reject parent traversal so the exception cannot authorize an adjacent
/// harness directory.
fn is_harness_file_memory_path(path: &std::path::Path, home: Option<&std::path::Path>) -> bool {
    use std::path::Component;

    let Some(home) = home else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let mut components = relative.components();
    let Some(Component::Normal(config_dir)) = components.next() else {
        return false;
    };
    let Some(config_dir) = config_dir.to_str() else {
        return false;
    };
    if config_dir != ".claude"
        && !config_dir
            .strip_prefix(".claude-")
            .is_some_and(|suffix| !suffix.is_empty())
    {
        return false;
    }
    if components.next() != Some(Component::Normal(std::ffi::OsStr::new("projects")))
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next() != Some(Component::Normal(std::ffi::OsStr::new("memory")))
    {
        return false;
    }

    components.all(|component| matches!(component, Component::Normal(_)))
}

/// Shell redirection to these character devices is a stream operation, not
/// file creation. Keep this exact allowlist narrow: other `/dev` paths remain
/// subject to the workspace contract.
fn is_non_creation_stream_device(path: &std::path::Path) -> bool {
    matches!(
        path.to_str(),
        Some("/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty")
    )
}

#[cfg(test)]
mod workspace_contract_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    fn bash_input(command: &str, cwd: &Path) -> HookInput {
        HookInput {
            cwd: cwd.to_string_lossy().to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({ "command": command })),
            ..Default::default()
        }
    }

    #[test]
    fn home_paths_in_read_only_commands_are_not_write_targets() {
        for command in [
            r#"printenv CLAUDE_CONFIG_DIR || echo "unset - using default ~/.claude""#,
            "grep -n claude ~/.zshrc | head",
            "file ~/.local/bin/claude; head -c 400 ~/.local/bin/claude | strings | head",
            "echo cp ~/.zshrc",
        ] {
            assert!(
                bash_write_targets(command).is_empty(),
                "read-only command must not identify a write target: {command}"
            );
        }
    }

    #[test]
    fn quoted_redirect_character_is_not_a_redirect() {
        assert!(bash_write_targets(r#"echo "a > ~/.claude""#).is_empty());
    }

    #[test]
    fn merged_parser_keeps_both_sides_fail_closed_write_forms() {
        let targets = bash_write_targets(
            "env MODE=test touch /tmp/from-env && printf proof >> /tmp/append-log; (/usr/bin/mkdir /tmp/from-path)",
        );
        for target in ["/tmp/from-env", "/tmp/append-log", "/tmp/from-path"] {
            assert!(
                targets.iter().any(|actual| actual == target),
                "merged parser must retain guarded target {target:?}: {targets:?}"
            );
        }
    }

    #[test]
    fn bash_rm_targets_expand_loop_variables() {
        let targets = bash_write_targets(
            "B=cas-cli/src/builtins; for h in codex grok; do for sk in cas-html-reports cas-dataviz; do rm -rf $B/$h/skills/$sk; done; done",
        );
        for target in [
            "cas-cli/src/builtins/codex/skills/cas-html-reports",
            "cas-cli/src/builtins/codex/skills/cas-dataviz",
            "cas-cli/src/builtins/grok/skills/cas-html-reports",
            "cas-cli/src/builtins/grok/skills/cas-dataviz",
        ] {
            assert!(
                targets.iter().any(|actual| actual == target),
                "rm target should be expanded and guarded: {target:?}; got {targets:?}"
            );
        }
    }

    #[test]
    fn bash_python_heredoc_extracts_open_rewrite_target() {
        let targets = bash_write_targets(
            "python3 - <<'PY'\np='cas-cli/src/builtins/skills/example.html'; s=open(p).read(); open(p,'w').write(s)\nPY",
        );
        assert!(
            targets
                .iter()
                .any(|target| target == "cas-cli/src/builtins/skills/example.html"),
            "Python heredoc open() target must be guarded: {targets:?}"
        );
    }

    #[test]
    fn factory_guard_allows_variable_rm_and_heredoc_inside_registered_worktree() {
        let cwd = tempfile::tempdir().expect("worktree");
        let mut env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", None)]);
        env.set("CAS_CLONE_PATH", cwd.path());
        for command in [
            "B=cas-cli/src/builtins; for h in codex grok; do rm -rf $B/$h/skills/cas-dataviz; done",
            "python3 - <<'PY'\np='cas-cli/src/builtins/skills/example.html'; open(p,'w').write('ok')\nPY",
        ] {
            let input = bash_input(command, cwd.path());
            assert_eq!(
                factory_unsanctioned_write_path(&input, &None, None, false),
                None,
                "registered worktree write should be sanctioned: {command}"
            );
        }
    }

    #[test]
    fn actual_write_destinations_still_resolve_to_unsanctioned_paths() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("worktree");
        let mut env = TestEnvGuard::with_optional_vars(&[("HOME", None)]);
        env.set("HOME", home.path());

        for (command, expected) in [
            ("echo hi > ~/stray", "stray"),
            ("cp source ~/copy", "copy"),
            ("mv source ~/moved", "moved"),
            ("tee ~/captured", "captured"),
            ("touch ~/touched", "touched"),
            ("mkdir ~/created", "created"),
        ] {
            let input = bash_input(command, cwd.path());
            assert_eq!(
                factory_unsanctioned_write_path(&input, &None, None, false),
                Some(home.path().join(expected)),
                "write must remain guarded: {command}"
            );
        }
    }

    #[test]
    fn configured_scratch_root_allows_its_children_and_denies_escape() {
        let cwd = tempfile::tempdir().expect("worktree");
        let input = bash_input("true", cwd.path());
        let scratch = std::path::PathBuf::from("/var/lib/cas-3bd6-scratch");
        let scratch_root = scratch.to_string_lossy().to_string();
        let outside = std::path::PathBuf::from("/var/lib/cas-3bd6-outside/stray.log");

        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                Some(&scratch_root),
                false,
                &scratch.join("logs/build.log").to_string_lossy(),
            ),
            None,
            "configured scratch root must permit nested output"
        );
        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                Some(&scratch_root),
                false,
                &scratch.join("../escape.log").to_string_lossy(),
            ),
            Some(scratch.parent().unwrap().join("escape.log")),
            "a lexical parent traversal must not escape the configured root"
        );
        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                Some(&scratch_root),
                false,
                &outside.to_string_lossy(),
            ),
            Some(outside),
            "configured scratch enforcement must deny unrelated host paths"
        );
    }

    #[test]
    fn relative_worktree_writes_remain_sanctioned_and_bare_tmp_is_denied() {
        let cwd = tempfile::tempdir().expect("worktree");
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", None)]);
        let input = bash_input("true", cwd.path());

        assert_eq!(
            unsanctioned_factory_path(&input, &None, None, false, "relative-output.log"),
            None,
            "relative outputs resolve inside the current worktree"
        );
        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                None,
                false,
                &std::env::temp_dir()
                    .join("cas-3bd6-tool-temp.log")
                    .to_string_lossy(),
            ),
            Some(std::env::temp_dir().join("cas-3bd6-tool-temp.log")),
            "bare system temp must remain outside the workspace contract"
        );
    }

    #[test]
    fn registered_worktree_allows_sibling_subtrees_when_cwd_is_nested() {
        let cwd = tempfile::tempdir().expect("worktree");
        let frontend = cwd.path().join("apps/frontend");
        let backend = cwd.path().join("apps/backend");
        std::fs::create_dir_all(&frontend).expect("frontend");
        std::fs::create_dir_all(&backend).expect("backend");
        let clone_path = cwd.path().to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", &backend);

        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                None,
                false,
                &frontend.join("new-component.ts").to_string_lossy(),
            ),
            None,
            "the registered worktree root must permit every nested subtree"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_registered_worktree_root_is_canonicalized() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let real_root = parent.path().join("real-worktree");
        let linked_root = parent.path().join("linked-worktree");
        std::fs::create_dir_all(real_root.join("apps/frontend")).expect("real root");
        std::fs::create_dir_all(real_root.join("apps/backend")).expect("real subtrees");
        symlink(&real_root, &linked_root).expect("worktree symlink");
        let clone_path = linked_root.to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", &linked_root.join("apps/backend"));

        assert_eq!(
            unsanctioned_factory_path(
                &input,
                &None,
                None,
                false,
                &linked_root
                    .join("apps/frontend/new-component.ts")
                    .to_string_lossy(),
            ),
            None,
            "symlinked registered roots must compare by their canonical target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_worktree_subtree_that_escapes_is_denied() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let worktree = parent.path().join("worktree");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::create_dir_all(&outside).expect("outside");
        symlink(&outside, worktree.join("link-out")).expect("escape symlink");
        let clone_path = worktree.to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", worktree.as_path());
        let target = worktree.join("link-out/escape.txt");

        assert_eq!(
            unsanctioned_factory_path(&input, &None, None, false, &target.to_string_lossy(),),
            Some(outside.join("escape.txt")),
            "canonical containment must reject a symlinked subtree outside the worktree"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_write_is_denied() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let worktree = parent.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("worktree");
        symlink(worktree.join("missing-target"), worktree.join("dangling"))
            .expect("dangling symlink");
        let clone_path = worktree.to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", worktree.as_path());
        let target = worktree.join("dangling/escape.txt");

        assert_eq!(
            unsanctioned_factory_path(&input, &None, None, false, &target.to_string_lossy()),
            Some(target),
            "unresolvable symlink targets must fail closed"
        );
    }

    #[test]
    fn refreshed_registered_worktree_is_used_mid_session() {
        let cas_root = tempfile::tempdir().expect("cas root");
        let first_root = tempfile::tempdir().expect("first worktree");
        let second_root = tempfile::tempdir().expect("refreshed worktree");
        let first_path = first_root.path().to_string_lossy().to_string();
        let second_path = second_root.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_SESSION_ID", Some("refresh-session")),
            ("CAS_CLONE_PATH", Some(first_path.as_str())),
        ]);

        let agent_store = open_agent_store(cas_root.path()).expect("agent store");
        let mut agent = Agent::new("refresh-session".to_string(), "refresh-worker".to_string());
        agent.role = AgentRole::Worker;
        agent
            .metadata
            .insert("clone_path".to_string(), first_path.clone());
        agent_store.register(&agent).expect("register worker");

        let input = bash_input("true", &second_root.path().join("apps/backend"));
        assert_eq!(
            registered_factory_worktree_root(&mut ToolHookStores::new(cas_root.path()), &input,)
                .as_deref(),
            Some(first_root.path()),
            "the initial durable registration should be authoritative"
        );

        agent
            .metadata
            .insert("clone_path".to_string(), second_path.clone());
        agent_store.update(&agent).expect("refresh worker");
        let mut stores = ToolHookStores::new(cas_root.path());
        let refreshed = registered_factory_worktree_root(&mut stores, &input)
            .expect("refreshed registered root");
        assert_eq!(refreshed, second_root.path());
        assert_eq!(
            unsanctioned_factory_path_with_worktree(
                &input,
                &None,
                None,
                false,
                &second_root
                    .path()
                    .join("apps/frontend/new-component.ts")
                    .to_string_lossy(),
                Some(&refreshed),
            ),
            None,
            "PreToolUse must see a refreshed worktree binding without a new process"
        );
    }

    #[test]
    fn pre_tool_uses_registered_worktree_for_nested_cwd() {
        let cas_root = tempfile::tempdir().expect("cas root");
        let worktree = tempfile::tempdir().expect("worktree");
        let frontend = worktree.path().join("apps/frontend");
        let backend = worktree.path().join("apps/backend");
        std::fs::create_dir_all(&frontend).expect("frontend");
        std::fs::create_dir_all(&backend).expect("backend");
        let clone_path = worktree.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_SESSION_ID", Some("registered-session")),
            ("CAS_CLONE_PATH", None),
        ]);

        let agent_store = open_agent_store(cas_root.path()).expect("agent store");
        let mut agent = Agent::new(
            "registered-session".to_string(),
            "registered-worker".to_string(),
        );
        agent.role = AgentRole::Worker;
        agent.metadata.insert("clone_path".to_string(), clone_path);
        agent_store.register(&agent).expect("register worker");

        let input = HookInput {
            session_id: "native-session-id".to_string(),
            cwd: backend.to_string_lossy().to_string(),
            hook_event_name: "PreToolUse".to_string(),
            tool_name: Some("Write".to_string()),
            tool_input: Some(serde_json::json!({
                "file_path": frontend.join("new-component.ts")
            })),
            agent_role: Some("worker".to_string()),
            ..HookInput::default()
        };

        let output = handle_pre_tool_use(&input, Some(cas_root.path())).expect("handler ok");
        let value = serde_json::to_value(output).expect("hook output JSON");
        assert_eq!(
            value["hookSpecificOutput"]["permissionDecision"], "allow",
            "a registered worktree sibling must not be rejected: {value}"
        );
    }

    #[test]
    fn outside_worktree_with_similar_prefix_stays_denied() {
        let parent = tempfile::tempdir().expect("parent");
        let worktree = parent.path().join("worktree");
        let sibling = parent.path().join("worktree-sibling");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::create_dir_all(&sibling).expect("sibling");
        let clone_path = worktree.to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", worktree.as_path());
        let target = sibling.join("not-allowed.txt");

        assert_eq!(
            unsanctioned_factory_path(&input, &None, None, false, &target.to_string_lossy(),),
            Some(target),
            "a sibling path sharing the root's string prefix must remain outside"
        );
    }

    #[test]
    fn cwd_outside_registered_worktree_stays_denied() {
        let worktree = tempfile::tempdir().expect("worktree");
        let outside = tempfile::tempdir().expect("outside");
        let clone_path = worktree.path().to_string_lossy().to_string();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(clone_path.as_str()))]);
        let input = bash_input("true", outside.path());
        let target = outside.path().join("not-allowed.txt");

        assert_eq!(
            unsanctioned_factory_path(&input, &None, None, false, &target.to_string_lossy()),
            Some(target),
            "an out-of-worktree cwd must not become a sanctioned write root"
        );
    }

    #[test]
    fn workspace_rejection_log_contains_path_rule_and_payload_size() {
        let cas_root = tempfile::tempdir().expect("cas root");
        let outside = tempfile::tempdir().expect("outside");
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_FACTORY_SESSION", Some("workspace-contract-test")),
            ("CAS_AGENT_NAME", Some("test-worker")),
            ("CAS_AGENT_ROLE", Some("worker")),
        ]);
        let input = HookInput {
            session_id: "test-session".to_string(),
            cwd: outside.path().to_string_lossy().to_string(),
            tool_name: Some("Write".to_string()),
            tool_input: Some(serde_json::json!({
                "file_path": "/not-sanctioned/escape.txt",
                "content": "payload"
            })),
            ..HookInput::default()
        };
        let violation = FactoryWriteViolation {
            evaluated_path: "/not-sanctioned/escape.txt".to_string(),
            resolved_path: std::path::PathBuf::from("/not-sanctioned/escape.txt"),
            matched_rule: "none",
        };

        log_factory_workspace_rejection(cas_root.path(), &input, &violation);
        let log_path = cas_root.path().join(format!(
            "logs/factory-session-{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        ));
        let line = std::fs::read_to_string(log_path).expect("workspace rejection log");
        let record: serde_json::Value = serde_json::from_str(line.trim()).expect("JSON event");
        let expected_payload_bytes = serde_json::to_vec(input.tool_input.as_ref().unwrap())
            .unwrap()
            .len()
            .to_string();
        assert_eq!(record["event"], "workspace_contract_rejection");
        assert_eq!(record["evaluated_path"], "/not-sanctioned/escape.txt");
        assert_eq!(record["resolved_path"], "/not-sanctioned/escape.txt");
        assert_eq!(record["matched_rule"], "none");
        assert_eq!(record["payload_bytes"], expected_payload_bytes);
    }
}

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

/// Return the single existing Cassy task ID named in a verifier prompt.
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

/// cas-7aa2 (GH #176): make this agent's stranded native `SendMessage` copies
/// inert.
///
/// Thin env-reading wrapper over
/// [`crate::ui::factory::daemon::runtime::teams::reap_stranded_native_inbox_copies`]
/// — see that function for the full mechanism. Resolves the inboxes this agent
/// may legitimately be the reader of so the sweep never touches them: its pane
/// name, plus the `supervisor` alias, which is the inbox file name the
/// supervisor's own messages arrive under (its `CAS_AGENT_NAME` is the
/// generated pane name, not `supervisor`).
///
/// Silent and infallible by construction: housekeeping must never fail a tool
/// call.
fn reap_stranded_native_send_message_copies() {
    let Ok(session) = std::env::var("CAS_FACTORY_SESSION") else {
        return;
    };
    let session = session.trim();
    if session.is_empty() {
        return;
    }

    let mut own_inbox_names: Vec<String> = Vec::new();
    if let Ok(name) = std::env::var("CAS_AGENT_NAME") {
        let name = name.trim();
        if !name.is_empty() {
            own_inbox_names.push(name.to_string());
        }
    }
    if crate::harness_policy::is_supervisor_from_env() {
        own_inbox_names.push("supervisor".to_string());
    }

    let reaped = crate::ui::factory::daemon::runtime::teams::reap_stranded_native_inbox_copies(
        session,
        &own_inbox_names,
    );
    if reaped > 0 {
        info!(
            session,
            reaped,
            "cas-7aa2: neutralised stranded native SendMessage copies in a non-factory teams tree"
        );
    }
}

/// Auto-route a factory-mode `SendMessage` tool call onto the Cassy prompt
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
                 Use Cassy coordination instead:\n\
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
        "SendMessage auto-routed onto Cassy prompt queue"
    );

    let prefix = crate::harness_policy::own_tool_prefix();
    // cas-73c8: success-shaped receipt. `permissionDecision=allow` so Claude
    // Code does not wrap the receipt in `<error>`; the guidance lives in
    // `additionalContext` (visible to the model next to the tool result).
    // `permissionDecisionReason` is user-facing only on allow.
    //
    // Native SendMessage also runs after allow, writing its own copy into THIS
    // process's config-dir teams tree. Two cases, and the old comment here
    // claimed both were covered by one mechanism — cas-7aa2 (GH #176) found
    // that only the first is:
    //   - Same tree as the daemon (the common case): covered. The daemon
    //     delivers `queued.prompt` verbatim (`queue_and_events.rs`:
    //     `let prompt_with_instructions = queued.prompt.clone();`) under the
    //     same sender name, so the two rows are byte-identical and the
    //     `(from, text)` guard in `teams::write_to_inbox_impl` collapses them.
    //   - Different tree (worker spawned with an explicit `config_dir`): NOT
    //     covered, and never was. `TeamsManager` only ever looks in the
    //     daemon's own tree, so it cannot see — let alone dedupe — a row in
    //     another config dir. That copy is a dead letter: no reader, and
    //     retention deliberately never prunes unread rows.
    // `reap_stranded_native_send_message_copies` (called from the factory
    // branch of this handler) makes those cross-tree strays inert.
    let receipt = format!(
        "✅ AUTO-ROUTED via Cassy coordination (message id {message_id}).\n\n\
         Message delivered to `{target}`. DO NOT retry this SendMessage call.\n\n\
         For future messages, call `{prefix}coordination action=message target=<name> message=\"...\" summary=\"...\"` directly — skip SendMessage."
    );
    HookOutput::with_pre_tool_permission_and_context(
        "allow",
        "Cassy auto-routed SendMessage",
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
    fn git_push_with_explicit_refspec_detected_cas_0efb() {
        assert!(looks_like_git_write_op(
            "git push origin HEAD:refs/heads/factory/fair-pelican-51"
        ));
    }

    #[test]
    fn local_merge_guard_detects_origin_push_forms() {
        assert!(looks_like_git_push_to_origin(
            "git push origin factory/worker"
        ));
        assert!(looks_like_git_push_to_origin(
            "git -C /repo push --set-upstream origin factory/worker"
        ));
        assert!(!looks_like_git_push_to_origin(
            "git push upstream factory/worker"
        ));
        assert!(!looks_like_git_push_to_origin("git commit -m 'local work'"));
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

    // ── cas-30c6 regression: sibling-worktree misbinding ─────────────────

    /// Respawn 1034 class: the harness was bound to a SIBLING worker's
    /// factory worktree. `factory/*` is allowlisted by the cas-7e7b denylist
    /// and the cwd is inside CAS_CLONE_PATH, so both existing checks passed
    /// and the worker was free to commit onto another worker's branch.
    ///
    /// The guard must compare the branch against the worker's own registered
    /// identity, not just against the protected-trunk denylist.
    #[test]
    fn sibling_factory_branch_commit_is_denied() {
        let tmp = make_git_repo();
        let p = tmp.path().to_string_lossy().to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "factory/bright-dolphin-92"])
            .current_dir(tmp.path())
            .output()
            .expect("switch to the sibling's branch");

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&p)),
            ("CAS_AGENT_NAME", Some("fair-pelican-51")),
        ]);

        let msg = check_worker_git_commit_scope(&p)
            .expect("a worker bound to a sibling's factory branch must be denied");
        assert!(
            msg.contains("bright-dolphin-92"),
            "the refusal must name the sibling that owns the branch: {msg}"
        );
        assert!(
            msg.contains("fair-pelican-51"),
            "the refusal must name the worker's own identity: {msg}"
        );
        assert!(
            msg.contains("factory/fair-pelican-51"),
            "the refusal must name the branch this worker actually owns: {msg}"
        );
    }

    /// Without a registered identity there is no canonical branch to compare
    /// against, so the sibling check must stand down rather than guess and
    /// deny every `factory/*` commit.
    #[test]
    fn unknown_identity_does_not_trigger_the_sibling_check() {
        let tmp = make_git_repo();
        let p = tmp.path().to_string_lossy().to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "factory/some-other-worker"])
            .current_dir(tmp.path())
            .output()
            .expect("switch branch");

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&p)),
            ("CAS_AGENT_NAME", None),
        ]);

        assert!(
            check_worker_git_commit_scope(&p).is_none(),
            "an unidentified worker must not be denied by the identity comparison"
        );
    }

    /// A worker on its OWN factory branch is untouched by the sibling check.
    #[test]
    fn own_factory_branch_commit_is_still_allowed() {
        let tmp = make_git_repo();
        let p = tmp.path().to_string_lossy().to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "factory/fair-pelican-51"])
            .current_dir(tmp.path())
            .output()
            .expect("switch to the worker's own branch");

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&p)),
            ("CAS_AGENT_NAME", Some("fair-pelican-51")),
        ]);

        assert!(
            check_worker_git_commit_scope(&p).is_none(),
            "a worker committing on its own factory branch must still be allowed"
        );
    }

    /// GH #337/#339: a non-isolated worker shares one checkout with every
    /// sibling. If another worker parks that checkout on its branch, both a
    /// commit and an explicit `HEAD:<mine>` push must stop before Git runs.
    #[test]
    fn non_isolated_foreign_factory_branch_commit_and_push_are_denied_cas_0efb() {
        let tmp = make_git_repo();
        let p = tmp.path().to_string_lossy().to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "factory/support-triage"])
            .current_dir(tmp.path())
            .output()
            .expect("park shared checkout on foreign branch");

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("credit-repairs")),
        ]);

        for command in [
            "git commit -m 'credit repair (cas-0efb)'",
            "git push origin HEAD:refs/heads/factory/credit-repairs",
        ] {
            let mut input = crate::hooks::handlers::HookInput::default();
            input.hook_event_name = "PreToolUse".to_string();
            input.tool_name = Some("Bash".to_string());
            input.cwd = p.clone();
            input.tool_input = Some(serde_json::json!({"command": command}));

            let out = handle_pre_tool_use(&input, None).expect("handler ok");
            let val = serde_json::to_value(&out).unwrap();
            let decision = val
                .get("hookSpecificOutput")
                .and_then(|h| h.get("permissionDecision"))
                .and_then(|v| v.as_str());
            assert_eq!(decision, Some("deny"), "{command}: {val}");
            let reason = val
                .get("hookSpecificOutput")
                .and_then(|h| h.get("permissionDecisionReason"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            assert!(reason.contains("support-triage"), "{reason}");
            assert!(reason.contains("credit-repairs"), "{reason}");
        }
    }

    #[test]
    fn known_worker_on_non_factory_branch_is_denied_cas_0efb() {
        let tmp = make_git_repo();
        let p = tmp.path().to_string_lossy().to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "feature/shared"])
            .current_dir(tmp.path())
            .output()
            .expect("switch branch");
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("credit-repairs")),
        ]);

        let msg = check_worker_git_commit_scope(&p).expect("wrong branch must be denied");
        assert!(msg.contains("WORKER BRANCH MISMATCH"), "{msg}");
        assert!(msg.contains("factory/credit-repairs"), "{msg}");
        assert!(msg.contains("graft"), "{msg}");
    }

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
        let switch_c = msg
            .find("git switch -c factory/bright-eagle-91")
            .expect("fallback");
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
        // cas-30c6: the branch must belong to THIS worker. Pinning the identity
        // also makes the case hermetic against an inherited CAS_AGENT_NAME.
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("test-worker")),
        ]);

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
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&ps)),
            ("CAS_AGENT_NAME", Some("isolated-worker")),
        ]);

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
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&ps)),
            ("CAS_AGENT_NAME", Some("isolated-worker")),
        ]);

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
        // cas-30c6: factory/worker1 is allowed because this IS worker1.
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&ps)),
            ("CAS_AGENT_NAME", Some("worker1")),
        ]);

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
            ("CAS_AGENT_NAME", Some("isolated-worker")),
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
            ("CAS_AGENT_NAME", Some("isolated-worker")),
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
        // cas-30c6: factory/guards is this worker's own branch — name it so the
        // canonical binding check has an identity to compare against instead of
        // whatever CAS_AGENT_NAME the test process inherited.
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&ps)),
            ("CAS_AGENT_NAME", Some("guards")),
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
            "prompt": "Review Cassy task cas-hook-capability"
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
            "prompt": "Review Cassy task cas-hook-unrelated"
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
