mod shell_effects;

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
    // Codex 0.163 reports the turn cwd and drops exec_command's workdir.
    // Honor the execution directory when a newer projection supplies it.
    let mut execution_input = None;
    if input.tool_name.as_deref() == Some("Bash")
        && cas_core::env_overlay::var("CAS_HOOK_HARNESS")
            .is_ok_and(|harness| harness.eq_ignore_ascii_case("codex"))
        && let Some(tool_input) = input.tool_input.as_ref()
    {
        if let Some(workdir) = tool_input.get("workdir").and_then(|value| value.as_str()).filter(|path| !path.is_empty()) {
            let mut resolved = input.clone();
            resolved.cwd = Path::new(&input.cwd).join(workdir).display().to_string();
            execution_input = Some(resolved);
        }
    }
    let input = execution_input.as_ref().unwrap_or(input);

    // Select the literal capped Cargo route before policy stores or provider
    // lookups. Every protection still evaluates the rewritten command below.
    if worker_cargo_request(input) {
        let command = input
            .tool_input
            .as_ref()
            .and_then(|v| v.get("command"))
            .and_then(|v| v.as_str())
            .unwrap();
        let (args, suffix, isolated_test_env) = worker_check_command(command).expect("route was selected");
        let Some(root) = cas_root else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "Worker check needs a Cassy root for max_concurrent_builders admission",
            ));
        };
        if let Some(reason) = worker_check_log_denial(input, &suffix) {
            return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
        }
        // cas-3d04: a shell `&` job dies with its caller under Codex, whose
        // exec_command SIGKILLs the shell's process group on exit. Run the
        // runner in the foreground with `--detach` instead; it starts the
        // check in its own session and returns. The log redirect stays on
        // the shell so the workspace guard still sees the write.
        let (detach, suffix) = match suffix.trim_end().strip_suffix('&') {
            Some(redirect) => (true, redirect.trim_end().to_string()),
            None => (false, suffix),
        };
        if detach && !suffix.contains('>') {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "WORKER CHECK BACKGROUND: a backgrounded check needs a log, or its output and receipt line are lost. Use `... > target/worker-check.log 2>&1 &` (create target/ first) or your task artifacts directory.",
            ));
        }
        // The shell opens this log before the runner starts. Preserve every
        // attempt's refusal/receipt even when the caller retries with `>`.
        let suffix = if suffix.trim_start().starts_with('>')
            && !suffix.trim_start().starts_with(">>")
        {
            suffix.replacen('>', ">>", 1)
        } else {
            suffix
        };
        let executable = std::env::current_exe()?;
        let rewritten = format!(
            "{} factory worker-check --cas-root {}{}{} -- {}{}",
            shell_quote_path(&executable),
            shell_quote_path(root),
            if detach { " --detach" } else { "" },
            isolated_test_env.as_ref().map(|path| format!(" --isolated-test-env {}", shell_quote_path(path))).unwrap_or_default(),
            args.iter()
                .map(|arg| if arg.trim_start().starts_with("test(") {
                    format!("'{arg}'")
                } else {
                    arg.clone()
                })
                .collect::<Vec<_>>()
                .join(" "),
            suffix
        );
        let mut updated = input.tool_input.clone().unwrap_or_default();
        updated["command"] = serde_json::Value::String(rewritten);
        return handle_pre_tool_use_inner(input, cas_root, Some(updated));
    }
    let mut output = handle_pre_tool_use_inner(input, cas_root, None)?;
    // cas-8dbe: only an edit the policy above allows takes a file lease.
    let denied = matches!(
        &output.hook_specific_output,
        Some(crate::hooks::types::HookSpecificOutput::PreToolUse { permission_decision: Some(decision), .. }) if decision == "deny"
    );
    if !denied && let Some(root) = cas_root {
        let codex = cas_core::env_overlay::var("CAS_HOOK_HARNESS")
            .is_ok_and(|harness| harness.eq_ignore_ascii_case("codex"));
        for note in worker_file_lease_notes(input, root, codex) {
            output.append_pre_tool_context(&note);
        }
    }
    Ok(output)
}

/// The files a worker's edit writes: Claude's Edit/Write/MultiEdit/NotebookEdit
/// target, or every file a Codex `apply_patch` adds, updates, deletes or moves.
fn worker_edit_targets(input: &HookInput) -> Vec<String> {
    let Some(tool_input) = input.tool_input.as_ref() else {
        return Vec::new();
    };
    match input.tool_name.as_deref() {
        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => tool_input
            .get("file_path")
            .or_else(|| tool_input.get("notebook_path"))
            .and_then(|value| value.as_str())
            .map(|file| vec![file.to_string()])
            .unwrap_or_default(),
        Some("apply_patch") => tool_input
            .get("command")
            .and_then(|value| value.as_str())
            .map(apply_patch_write_targets)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// cas-8dbe: a worker about to edit repository files claims them through
/// Cassy Cloud, and learns when another session's worker already holds one.
/// Claude gets the returned notes as PreToolUse context. Codex rejects that
/// context, so its worker is messaged instead, once per file per window
/// (cas-a30e9), and no note is returned.
fn worker_file_lease_notes(input: &HookInput, cas_root: &Path, codex: bool) -> Vec<String> {
    if !crate::harness_policy::is_worker(input) {
        return Vec::new();
    }
    let targets = worker_edit_targets(input);
    if targets.is_empty() {
        return Vec::new();
    }
    let cwd = Path::new(&input.cwd);
    let toplevel = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success());
    let Some(toplevel) = toplevel else {
        return Vec::new();
    };
    let repo = std::path::PathBuf::from(String::from_utf8_lossy(&toplevel.stdout).trim());
    let agent_id = current_agent_id(input);
    let agent_name = std::env::var("CAS_AGENT_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| agent_id.clone());
    let Some(task) = crate::store::open_task_store(cas_root)
        .ok()
        .and_then(|store| store.list(Some(cas_types::TaskStatus::InProgress)).ok())
        .unwrap_or_default()
        .into_iter()
        .find(|task| {
            task.assignee
                .as_deref()
                .is_some_and(|assignee| assignee == agent_id || assignee == agent_name)
        })
    else {
        return Vec::new();
    };
    let session = std::env::var("CAS_FACTORY_SESSION").ok().filter(|s| !s.trim().is_empty());
    let mut notes = Vec::new();
    for target in targets {
        let file = if Path::new(&target).is_absolute() {
            std::path::PathBuf::from(&target)
        } else {
            cwd.join(&target)
        };
        let Ok(relative) = file.strip_prefix(&repo) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() || relative.starts_with(".cas/") || relative.starts_with("target/") {
            continue;
        }
        let crate::cloud::file_leases::FileLease::HeldBy(holder) =
            crate::cloud::file_leases::claim(cas_root, &agent_id, &agent_name, &task.id, &relative)
        else {
            continue;
        };
        if codex {
            crate::cloud::file_leases::notify_worker_of_holder(
                cas_root, &agent_id, &agent_name, session.as_deref(), &relative, &holder,
            );
        } else {
            notes.push(crate::cloud::file_leases::collision_note(&relative, &holder));
        }
    }
    notes
}

/// cas-d95c: the verifier's supported capped command for a task's named tests.
fn verifier_rust_build_denial(what: &str, cas_root: Option<&Path>) -> String {
    let root = cas_root.map_or_else(|| "<cas root>".to_string(), |root| root.display().to_string());
    format!(
        "🚫 VERIFIER RUST BUILD: `{what}` would build outside the host builder cap and PSI admission. \
         Run the task's named tests at the verification's bound head through the counted verifier lane: \
         `cas factory verifier-test --cas-root {root} --task-id <task-id> --head <full bound head SHA> -- nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`. \
         A \"Waiting for build admission\" refusal means retry later; it is not a test failure."
    )
}

/// Pure admission selection; safe before logging, DB discovery or telemetry.
pub(crate) fn worker_cargo_request(input: &HookInput) -> bool {
    crate::harness_policy::is_worker(input)
        && input.tool_name.as_deref() == Some("Bash")
        && input
            .tool_input
            .as_ref()
            .and_then(|v| v.get("command"))
            .and_then(|v| v.as_str())
            .and_then(worker_check_command)
            .is_some()
}
fn handle_pre_tool_use_inner(
    input: &HookInput,
    cas_root: Option<&Path>,
    prepared_cargo_input: Option<serde_json::Value>,
) -> Result<HookOutput, MemError> {
    let tool_name = match &input.tool_name {
        Some(name) => name.as_str(),
        None => return Ok(HookOutput::empty()),
    };

    // Run before factory auto-approval and the missing-root early return.
    // Slack transport is an operator policy for every session.
    if let Some(reason) =
        super::slack_transport::denial(tool_name, input.tool_input.as_ref(), cas_root)
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }
    // cas-f6ad (GH #1057): no deliverable share before its epic's
    // verification passes, for every harness and role.
    if let Some(reason) =
        super::publication_gate::denial(tool_name, input.tool_input.as_ref(), cas_root)
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }

    if let Some(reason) = super::browser_tier_guard::denial(input) {
        return Ok(HookOutput::with_pre_tool_permission("deny", reason));
    }

    if let Some(reason) =
        super::worker_denied_tools::credential_store_access_denial(input, tool_name)
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }

    // cas-d757 (GH #1219): operator-authenticated CLIs (vercel/neonctl) and their
    // MCP tools are the supervisor's. Workers only; before auto-approval and
    // the missing-root early return.
    if let Some(reason) = super::worker_denied_tools::denial(input, tool_name, cas_root) {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }

    // cas-3147 (GH #1169): write roots and grants are operator-only. Every
    // harness tool call is an agent's, so the operator-only CLI is refused
    // here for every role, before any auto-approval or root early return.
    if tool_name == "Bash"
        && let Some(reason) = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get("command"))
            .and_then(|command| command.as_str())
            .and_then(|command| {
                operator_policy_command_denial(command)
                    .or_else(|| role_env_tampering_denial(command))
            })
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }
    // cas-1b94: agent `cas mcp add|remove|import` cannot remove, replace or
    // alias a supervisor-only MCP server (the proxy_add rule, cas-0d4f0).
    #[cfg(feature = "mcp-proxy")]
    if tool_name == "Bash"
        && let Some(root) = cas_root
        && let Some(reason) = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get("command"))
            .and_then(|command| command.as_str())
            .and_then(|command| supervisor_only_mcp_command_denial(command, root))
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }
    // cas-0d4f0: no agent writes a Cassy config file directly, for every role.
    if let Some(reason) = cas_config_file_write_denial(input) {
        return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
    }

    let is_factory_agent = crate::harness_policy::is_factory_agent(input);

    // ========================================================================
    // WORKER COMMAND-SCOPE GUARDS (cas-eb39, cas-852a0)
    //
    // A mutating `cargo fmt` selects Cargo targets rather than source paths,
    // and direct rustfmt follows child modules unless skip_children is set.
    // In a workspace that is not fmt-clean, either shape spills unrelated
    // changes. Workers use scoped format commands and may run only capped,
    // package-scoped cargo check --lib or --tests (cas-3efd, cas-3051).
    // Workers run targeted tests; the supervisor runs the full suite at assembly.
    // Hoist this before the cas_root early return and factory Bash auto-allow so
    // an unscoped run always gets the loud, actionable refusal.
    // ========================================================================
    if is_factory_agent && crate::harness_policy::is_worker(input) && tool_name == "Bash" {
        let command = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get("command"))
            .and_then(|command| command.as_str());
        if command.is_some_and(worker_command_pattern_kill) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                "WORKER PROCESS SIGNAL: pattern-based kills can terminate another worker's processes on this shared host. Kill by the PID you started: capture $! or use your own PID file, then run `kill <pid>` (or `kill -0 <pid>` to inspect it). Do not use pkill, killall, or pgrep results to send signals.",
            ));
        }
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
        if let Some(updated) = prepared_cargo_input {
            let mut rewritten_input = input.clone();
            rewritten_input.tool_input = Some(updated.clone());
            let mut output = handle_pre_tool_use_inner(&rewritten_input, cas_root, None)?;
            output.rewrite_pre_tool_input(updated);
            return Ok(output);
        }
        if let Some(what) = command.and_then(|command| worker_command_rust_build(command, Path::new(&input.cwd))) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &format!(
                    "🚫 NO WORKER RUST BUILDS: `{what}` is outside the capped worker exceptions. \
                     Use exactly `cargo check -p <crate> [-p <crate> ...] --lib` for lib-only edits or `--tests` when test files changed, optionally with log redirection and backgrounding. Choose one target flag. \
                     The hook runs it under max_concurrent_builders using your private seeded target cache. \
                     Commit first so a successful check records `check: PASS <sha>`. \
                     Targeted tests use exactly `cargo nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`; an omitted target selects --lib. Empty/all() filters, repeated packages, broad flags and compound commands are refused. Full builds and suites remain supervisor-only at epic assembly (cas-4cbb)."
                ),
            ));
        }
    }

    // cas-d95c: a task-verifier subagent's Rust build runs on the shared host
    // like a worker's. Plain Cargo there bypassed the builder cap and PSI
    // admission; send it to the counted verifier lane at the bound head.
    if tool_name == "Bash"
        && input.agent_type.as_deref() == Some("task-verifier")
        && let Some(what) = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get("command"))
            .and_then(|command| command.as_str())
            .and_then(|command| worker_command_rust_build(command, Path::new(&input.cwd)))
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &verifier_rust_build_denial(&what, cas_root)));
    }

    // ========================================================================
    // WORKER NEON PRODUCTION WRITE GUARD (cas-d8fc, GH #907)
    //
    // A worker's Neon run_sql / run_sql_transaction that carries a write and
    // resolves to production (no branchId, or the recorded production branch)
    // is denied with the branch it resolved to. Reads and writes to other
    // branches pass. Hoisted with the other root-independent worker guards so
    // a missing Cassy root cannot skip it.
    // ========================================================================
    if is_factory_agent && crate::harness_policy::is_worker(input) {
        let mut roots = vec![std::path::PathBuf::from(&input.cwd)];
        if let Some(parent) = cas_root.and_then(Path::parent) {
            roots.push(parent.to_path_buf());
        }
        if let Some(reason) = super::neon_sql_guard::neon_production_write_denial(
            tool_name,
            input.tool_input.as_ref(),
            &roots,
        ) {
            return Ok(HookOutput::with_pre_tool_permission("deny", &reason));
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
                    Use {prefix}factory action=spawn_workers — factory-managed worktrees get cleaned up; Agent(isolation=\"worktree\") ones leak.\n\
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
    // FACTORY SUPERVISOR: Deny native SendMessage before cas_root resolution
    // (GH #793).
    //
    // A supervisor must never reach the host harness's native transport: an
    // `allow` result lets that transport run after this hook and bypass the
    // durable CAS coordination queue. Hoist the deny next to the other
    // root-independent factory guards so a missing Cassy root cannot create a
    // silent escape hatch.
    // ========================================================================
    if is_factory_agent
        && tool_name == "SendMessage"
        && crate::harness_policy::is_supervisor(input)
    {
        return Ok(deny_factory_supervisor_send_message());
    }

    // Native SendMessage bypasses the policy and origin stamps on the CAS
    // coordination path, so factory workers must use that path directly.
    if is_factory_agent
        && tool_name == "SendMessage"
        && crate::harness_policy::is_worker(input)
    {
        return Ok(deny_factory_worker_send_message());
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
                // cas-6db4: judge the push by the delivery mode of the task
                // the worker is delivering, the same field the close gate
                // reads. The session-wide mode (set by any spawn with
                // delivery_mode) only decides for a worker with no task in
                // progress; otherwise a push_branch task in a local_merge
                // session could neither push nor pass its close's merge check.
                if looks_like_git_push_to_origin(cmd)
                    && !local_merge_push_override()
                    && effective_worker_delivery_mode(
                        worker_delivery_mode(),
                        &cas_root
                            .map(|root| {
                                let mut stores = ToolHookStores::new(root);
                                worker_active_task_delivery_modes(&mut stores, input)
                            })
                            .unwrap_or_default(),
                    ) == cas_types::DeliveryMode::LocalMerge
                {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        "🚫 LOCAL-MERGE DELIVERY: git push origin is disabled for this factory session. Commit locally; the supervisor merges your local factory branch. Set CAS_FACTORY_LOCAL_MERGE_PUSH_OVERRIDE=1 only when explicitly authorized by the supervisor.",
                    ));
                }
                if looks_like_git_write_op(cmd) {
                    // `CAS_CLONE_PATH` is the bootstrap value, but SessionStart
                    // persists the authoritative checkout binding in the agent
                    // row. Resolve it when the hook has a Cassy root so a
                    // worker whose environment was refreshed (or whose hook
                    // process inherited only CAS_ROOT) cannot turn the shared
                    // primary checkout into an apparently unbound target.
                    let registered_worktree = cas_root.and_then(|root| {
                        let mut stores = ToolHookStores::new(root);
                        registered_factory_worktree_root(&mut stores, input)
                    });
                    if let Some((target, deny_msg)) =
                        worker_git_scope_violation_for_command_with_worktree(
                            &input.cwd,
                            cmd,
                            registered_worktree.as_deref(),
                        )
                    {
                        if let Some(root) = cas_root {
                            log_factory_git_scope_rejection(root, input, &target, cmd);
                        }
                        return Ok(HookOutput::with_pre_tool_permission("deny", &deny_msg));
                    }
                }
            }
        }
    }

    // ========================================================================
    // INDEPENDENT QA MERGE GUARD (cas-619f)
    //
    // Raw `git merge factory/<worker>` is the documented supervisor merge
    // path, so the "a user-facing delivery cannot merge without an
    // independent QA pass" rule has to hold here, not only in
    // `worktree_merge`. Denies only when a parked, QA-bound delivery's
    // current tip lacks a passed or waived round; everything else is
    // untouched, and unreadable state fails open to the close backstop.
    // ========================================================================
    if tool_name == "Bash"
        && crate::harness_policy::is_supervisor(input)
        && let Some(root) = cas_root
        && let Some(cmd) = input
            .tool_input
            .as_ref()
            .and_then(|ti| ti.get("command").and_then(|v| v.as_str()))
        && let Some(refusal) =
            crate::qa_pass::supervisor_merge_refusal(root, std::path::Path::new(&input.cwd), cmd)
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &refusal));
    }

    // cas-2ee2 (GH #1023): a pull request merged on GitHub skips both Cassy
    // merge paths, and any agent holding a `gh` token can do it, so the raw
    // GitHub merge check applies to every role. (The supervisor already got
    // it through `supervisor_merge_refusal` above.)
    if tool_name == "Bash"
        && !crate::harness_policy::is_supervisor(input)
        && let Some(root) = cas_root
        && let Some(cmd) = input
            .tool_input
            .as_ref()
            .and_then(|ti| ti.get("command").and_then(|v| v.as_str()))
        && let Some(refusal) =
            crate::qa_pass::github_merge_refusal(root, std::path::Path::new(&input.cwd), cmd)
    {
        return Ok(HookOutput::with_pre_tool_permission("deny", &refusal));
    }

    // ========================================================================
    // FACTORY WORKSPACE CONTRACT — ROOT-INDEPENDENT FALLBACK
    //
    // Filesystem auto-approval must remain available when Cassy cannot resolve
    // a root for the hook invocation, but it must not bypass the workspace
    // contract. The normal guard below can read the registered checkout and
    // configured roots from Cassy's stores; this fallback uses the bootstrap
    // `CAS_CLONE_PATH` binding without granting a shared artifact base, so an isolated
    // worker cannot write into the primary checkout on the cas_root=None path.
    // ========================================================================
    if cas_root.is_none() && is_factory_agent {
        let worktree_root = std::env::var_os("CAS_CLONE_PATH")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from);
        // Without a store identity, sanction only the bound worktree and
        // harness scratchpad; the shared artifact base is not a write root.
        let artifacts_root = Some(worktree_root.as_deref().unwrap_or(Path::new(&input.cwd)).display().to_string());
        if let Some(violation) = factory_write_violation(
            input,
            &artifacts_root,
            None,
            crate::harness_policy::is_supervisor(input),
            worktree_root.as_deref(),
        ) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &factory_workspace_contract_denial(
                    input,
                    &violation,
                    artifacts_root.as_deref(),
                    None,
                    worktree_root.as_deref(),
                ),
            ));
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
        let mut artifacts_root = Some(crate::config::resolved_factory_artifact_paths(
            cas_root, artifacts_root.as_deref()
        ).project_root.display().to_string());
        if artifacts_root.as_deref().is_some_and(|root| {
            std::fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink())
        }) {
            // An aliased namespace must not authorize writes to its target.
            // Leave worktree operations and historical reads available.
            artifacts_root = Some(registered_worktree.as_deref().unwrap_or(Path::new(&input.cwd)).display().to_string());
        }
        // cas-3147 (GH #1169): operator write roots and task grants.
        let is_supervisor = crate::harness_policy::is_supervisor(input);
        let (write_policy, write_task_ids) =
            operator_write_policy_for(cas_root, &mut stores, input, is_supervisor);
        match factory_write_decision(
            input,
            &artifacts_root,
            scratch_root.as_deref(),
            is_supervisor,
            registered_worktree.as_deref(),
            &write_policy,
            &write_task_ids,
        ) {
            FactoryWriteDecision::Allowed => {}
            FactoryWriteDecision::AllowedByRoot(uses) => {
                log_write_root_uses(cas_root, input, &uses);
            }
            FactoryWriteDecision::Denied(violation) => {
                log_factory_workspace_rejection(cas_root, input, &violation);
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &factory_workspace_contract_denial_with_roots(
                        input,
                        &violation,
                        artifacts_root.as_deref(),
                        scratch_root.as_deref(),
                        registered_worktree.as_deref(),
                        &write_policy,
                        &write_task_ids,
                    ),
                ));
            }
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
    // FACTORY MODE: clean up stranded legacy native SendMessage copies
    //
    // New messages use the dedicated coordination command. Keep this
    // housekeeping for inert copies left by older auto-routing hooks.
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
            let measurement = super::docs_freshness::Measurement::at(Path::new(&input.cwd), cas_root);
            if let Some(
                crate::hooks::handlers::handlers_events::CodemapStaleness::SignificantlyStale {
                    total_changes,
                    ..
                },
            ) = super::codemap::evaluate_codemap_at(&measurement).and_then(|evaluation| evaluation.staleness)
            {
                let sync = if measurement.reference != "HEAD"
                    && super::docs_freshness::git(&measurement.root,
                        &["merge-base", "--is-ancestor", &measurement.reference, "HEAD"]).is_none()
                {
                    match crate::worktree::GitOperations::git_remedies([
                        &["merge", &measurement.reference],
                        &["rebase", &measurement.reference],
                    ]) {
                        Ok([merge, rebase]) => {
                            format!("Sync this checkout: `{merge}` or `{rebase}`.\n")
                        }
                        Err(refusal) => format!("{refusal}\n"),
                    }
                } else {
                    String::new()
                };
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &format!(
                        "🗺️  CODEMAP.md is significantly out of date ({total_changes} structural changes).\n\
                        Measured ref: {} ({}).\n\n\
                        Workers rely on CODEMAP for codebase orientation — dispatching against a stale map wastes tokens.\n\n\
                        {sync}Run `/codemap` to refresh, then retry.", measurement.reference, measurement.root.display()
                    ),
                ));
            }
        }
    }

    // ========================================================================
    // WORKTREE MERGE JAIL: Block all tools except the worktree merge when pending
    //
    // When a task has pending_worktree_merge=true, block all tools except:
    // 1. `factory action=worktree_merge` (any harness prefix), or a
    //    Task/Agent spawn of a project-defined `worktree-merger` agent —
    //    either unjails by clearing pending_worktree_merge.
    //
    // The unjail happens in PreToolUse when one of those calls is detected.
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
                    // cas-dc1b (M38): the sanctioned exit is the Cassy
                    // `factory action=worktree_merge` call under any
                    // harness prefix. No `worktree-merger` agent ships, so
                    // the legacy Task/Agent spawn is only honoured for
                    // projects that define one themselves.
                    let tool_input_str = |key: &str| {
                        input
                            .tool_input
                            .as_ref()
                            .and_then(|ti| ti.get(key).and_then(|v| v.as_str()))
                    };
                    let is_worktree_merger = ((tool_name == "Task" || tool_name == "Agent")
                        && tool_input_str("subagent_type") == Some("worktree-merger"))
                        || ((tool_name.ends_with("factory")
                            || tool_name.ends_with("coordination"))
                            && tool_input_str("action") == Some("worktree_merge"));

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
                            Merge and clean up each task's worktree with \
                            `{prefix}factory action=worktree_merge id=<worktree branch> task_id=<task-id> cleanup=true` \
                            (the close rejection names the branch). That call is allowed through the jail and releases it; \
                            then retry `{prefix}task action=close id=<task-id>`.",
                                prefix = crate::harness_policy::own_tool_prefix()
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
        // Bash has no file_path. Reuse the structural destination recognizer
        // from workspace containment; credential reads and quoted command text
        // do not name write targets. Resolve relative destinations as a direct
        // Write would name them, including .env.local / .env.example paths.
        let shell_targets: Vec<String> = if tool_name == "Bash" {
            input
                .tool_input
                .as_ref()
                .and_then(|ti| ti.get("command").and_then(|v| v.as_str()))
                .map(bash_write_targets)
                .unwrap_or_default()
                .into_iter()
                .map(|target| {
                    Path::new(&input.cwd)
                        .join(target)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        } else {
            Vec::new()
        };
        for path in file_path
            .into_iter()
            .chain(shell_targets.iter().map(String::as_str))
        {
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
        // cas-90e8: every denial names the next call the spawning agent
        // can make, rendered once in close_ops::gate_text.
        use crate::mcp::tools::core::task::lifecycle::close_ops::gate_text::{
            VerifierSpawnDenial, verifier_spawn_denial,
        };
        let verifier_denial = |denial: VerifierSpawnDenial, task_id: Option<&str>| {
            verifier_spawn_denial(denial, task_id, crate::harness_policy::own_tool_prefix())
        };
        let Some(agent_store) = stores.agents() else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &verifier_denial(VerifierSpawnDenial::RegistryUnavailable, None),
            ));
        };
        let Ok(parent) = agent_store.get(&current_agent_id) else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &verifier_denial(VerifierSpawnDenial::UnregisteredParent, None),
            ));
        };
        if !matches!(
            parent.status,
            crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle
        ) {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &verifier_denial(VerifierSpawnDenial::InactiveParent, None),
            ));
        }

        let Some(tool_input) = input.tool_input.as_ref() else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &verifier_denial(VerifierSpawnDenial::MissingPrompt, None),
            ));
        };
        let prompt = tool_input
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let Some(task_id) = unique_existing_task_id(prompt, stores.tasks()) else {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &verifier_denial(VerifierSpawnDenial::NoUniqueTask, None),
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
                        &verifier_denial(
                            VerifierSpawnDenial::DispatchOwnedElsewhere,
                            Some(task_id.as_str()),
                        ),
                    ));
                }
                if dispatch.deadline_at <= chrono::Utc::now() {
                    return Ok(HookOutput::with_pre_tool_permission(
                        "deny",
                        &verifier_denial(
                            VerifierSpawnDenial::DispatchDeadlineElapsed,
                            Some(task_id.as_str()),
                        ),
                    ));
                }
                dispatch.id
            }
            Ok(_) => {
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &verifier_denial(VerifierSpawnDenial::NoActiveDispatch, Some(task_id.as_str())),
                ));
            }
            Err(_) => {
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &verifier_denial(VerifierSpawnDenial::DispatchUnreadable, Some(task_id.as_str())),
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
                &verifier_denial(VerifierSpawnDenial::MissingToolUseId, Some(task_id.as_str())),
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
                        &verifier_denial(
                            VerifierSpawnDenial::SpawnAlreadyPending,
                            Some(task_id.as_str()),
                        ),
                    ));
                }
                return Ok(HookOutput::with_pre_tool_permission(
                    "deny",
                    &verifier_denial(VerifierSpawnDenial::HandoffFailed, Some(task_id.as_str())),
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
        // cas-4143: Claude Code can still re-check this allow against its
        // own safety rules (e.g. a heredoc) and park the call as a teammate
        // permission request for a lead nobody plays. Record CAS's verdict
        // for exactly this call so the factory daemon can answer that
        // request with it (crate::factory_permission_relay).
        if let Some(tool_use_id) = input.tool_use_id.as_deref() {
            crate::factory_permission_relay::record_hook_allow(cas_root, tool_use_id, tool_name);
        }
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

/// Accept a literal check/targeted nextest plus a simple log/background suffix.
/// The sole env prefix is the complete fixed provider-isolation profile; never
/// accept general wrappers, shell substitutions, toolchains or build overrides.
fn worker_check_command(command: &str) -> Option<(Vec<String>, String, Option<std::path::PathBuf>)> {
    let command = command.trim();
    if command.contains(['\n', '\r']) { return None; }
    let (cargo, isolated) = if command.starts_with("env ") {
        let split = command.find(" cargo ")?;
        let words = command[..split].split_whitespace().collect::<Vec<_>>();
        let (assignment, prefix) = words.split_last()?;
        let directory = assignment.strip_prefix("XDG_CONFIG_HOME=")?;
        // A literal path, never a shell expansion or arbitrary assignment.
        if !Path::new(directory).is_absolute()
            || !directory.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_/.-".contains(&byte))
            || prefix.first() != Some(&"env")
        { return None; }
        let mut removed = std::collections::BTreeSet::new();
        for pair in prefix[1..].chunks(2) {
            if pair.len() != 2 || pair[0] != "-u"
                || !crate::factory_worker_check::ISOLATED_TEST_CREDENTIALS.contains(&pair[1])
                || !removed.insert(pair[1])
            { return None; }
        }
        if removed.len() != crate::factory_worker_check::ISOLATED_TEST_CREDENTIALS.len() { return None; }
        (&command[split + 1..], Some(std::path::PathBuf::from(directory)))
    } else { (command, None) };
    let (args, suffix) = literal_worker_check_command(cargo)?;
    if isolated.is_some() && args.first().map(String::as_str) != Some("nextest") { return None; }
    Some((args, suffix, isolated))
}

fn literal_worker_check_command(command: &str) -> Option<(Vec<String>, String)> {
    let command = command.trim();
    if command.contains(['\n', '\r']) {
        return None;
    }
    // Single quotes make the mandatory filter a literal shell argument. Match
    // the suffix after the filter so '&' within a filter is not backgrounding.
    if command.starts_with("cargo nextest ") {
        let pattern = regex::Regex::new(r"^cargo\s+nextest\s+run\s+-p\s+([A-Za-z0-9_-]+)(?:\s+(--lib)|\s+--test\s+([A-Za-z0-9_-]+))?\s+-E\s+'([^'\n]+)'(\s*(?:>{1,2}\s*[A-Za-z0-9_/.-]+\s*(?:2>&1\s*)?)?&?\s*)$").ok()?;
        let matched = pattern.captures(command)?;
        let mut args = vec![
            "nextest".into(),
            "run".into(),
            "-p".into(),
            matched[1].into(),
        ];
        if matched.get(2).is_some() {
            args.push("--lib".into());
        }
        if let Some(harness) = matched.get(3) {
            args.extend(["--test".into(), harness.as_str().into()]);
        }
        args.extend(["-E".into(), matched[4].into()]);
        if !crate::factory_worker_check::allowed_args(&args) {
            return None;
        }
        let suffix = matched.get(5)?.as_str();
        return Some((
            args,
            if suffix.trim().is_empty() {
                String::new()
            } else {
                format!(" {}", suffix.trim_start())
            },
        ));
    }
    let split = command.find(['>', '&']).unwrap_or(command.len());
    let (body, suffix) = command.split_at(split);
    let suffix_pattern =
        regex::Regex::new(r"^\s*(?:>{1,2}\s*[A-Za-z0-9_/.-]+\s*(?:2>&1\s*)?)?&?\s*$").ok()?;
    if !suffix_pattern.is_match(suffix) {
        return None;
    }
    let words = body.split_whitespace().collect::<Vec<_>>();
    if words.get(..2)? != ["cargo", "check"] {
        return None;
    }
    let args = words[2..]
        .iter()
        .map(|word| word.to_string())
        .collect::<Vec<_>>();
    crate::factory_worker_check::check_packages(&args)?;
    let suffix = if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    };
    Some((args, suffix))
}

fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

/// The shell creates the log before the runner can inspect HEAD. An ignored
/// outbox is compatible with its clean-commit evidence; an untracked source
/// file is not. External logs still pass through the workspace guard below.
fn worker_check_log_denial(input: &HookInput, suffix: &str) -> Option<String> {
    let pattern = regex::Regex::new(r"^\s*>{1,2}\s*([A-Za-z0-9_/.-]+)").ok()?;
    let matched = pattern.captures(suffix)?;
    let cwd = std::path::PathBuf::from(&input.cwd);
    let path = canonicalize_for_containment(&cwd.join(&matched[1]))?;
    if is_non_creation_stream_device(&path) {
        return None;
    }
    let root = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            canonicalize_for_containment(std::path::Path::new(
                String::from_utf8(output.stdout).ok()?.trim(),
            ))
        })
        .or_else(|| canonicalize_for_containment(&cwd))?;
    if !path.starts_with(&root) {
        return None;
    }
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "--quiet", "--"])
        .arg(&path)
        .current_dir(&cwd)
        .output()
        .is_ok_and(|output| output.status.success());
    (!ignored).then(|| format!(
        "WORKER CHECK LOG: {} would dirty the checkout before the clean-commit gate. Use an ignored target/worker-check.log (create target/ first) or your task artifacts directory. Tracked and untracked source changes still require a commit.",
        path.display()
    ))
}

/// The Rust build a worker command would start, if any (cas-4cbb): a cargo
/// subcommand that compiles, `cargo-nextest`, `rustc`/`rustdoc`, the scoped
/// test wrapper, or a `make test*` target. Read-only cargo (fmt checks,
/// metadata, tree, version) is not a build. Commands passed to `sh -c` /
/// `bash -c` are inspected too; quoted text in other commands (a commit
/// message, an echo) is not.
fn worker_command_rust_build(command: &str, cwd: &Path) -> Option<String> {
    worker_command_rust_build_at_depth(command, cwd, 0)
}

fn worker_command_rust_build_at_depth(command: &str, cwd: &Path, depth: usize) -> Option<String> {
    for words in shell_statement_words(command) {
        if let Some(found) = rust_build_invocation(&words, cwd) {
            return Some(found);
        }
        if depth < 2 {
            // `env -S 'cargo …'` runs its split string as the command (cas-cfd6).
            if let Some(payload) = env_split_string_payload(&words) {
                if let Some(found) = worker_command_rust_build_at_depth(&payload, cwd, depth + 1) {
                    return Some(found);
                }
            }
        }
        if depth < 2 {
            // `sh -c '<script>'`, `bash -lc "<script>"`
            for (index, word) in words.iter().enumerate() {
                let shell = matches!(shell_word_basename(word), "sh" | "bash" | "zsh" | "dash");
                if !shell {
                    continue;
                }
                let script = words[index + 1..]
                    .iter()
                    .position(|arg| {
                        arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c')
                    })
                    .and_then(|flag| words.get(index + 1 + flag + 1));
                if let Some(found) =
                    script.and_then(|script| worker_command_rust_build_at_depth(script, cwd, depth + 1))
                {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Cargo subcommands that compile the workspace or run compiled tests.
const CARGO_BUILD_SUBCOMMANDS: &[&str] = &[
    "build",
    "b",
    "check",
    "c",
    "test",
    "t",
    "nextest",
    "run",
    "r",
    "clippy",
    "bench",
    "doc",
    "d",
    "install",
    "rustc",
    "rustdoc",
    "fix",
    "miri",
    "llvm-cov",
    "tarpaulin",
    "udeps",
    "expand",
    "insta",
    "mutants",
    "hack",
];

fn rust_build_invocation(words: &[String], cwd: &Path) -> Option<String> {
    let mut index = executable_word_index(words)?;
    // Wrappers that run their argument as a command.
    loop {
        let word = shell_word_basename(&words[index]);
        let skip = match word {
            "timeout" => {
                // timeout [opts] DURATION cmd…; `-s SIG` / `-k DUR` take a value.
                let mut next = index + 1;
                while next < words.len() && words[next].starts_with('-') {
                    if matches!(
                        words[next].as_str(),
                        "-s" | "-k" | "--signal" | "--kill-after"
                    ) {
                        next += 1;
                    }
                    next += 1;
                }
                next + 1
            }
            "nice" | "ionice" | "stdbuf" | "chrt" | "taskset" => {
                let mut next = index + 1;
                while next < words.len() && words[next].starts_with('-') {
                    next += 1;
                    // `-n 10`, `-c 2` style values
                    if next < words.len()
                        && !words[next].starts_with('-')
                        && words[next].parse::<i64>().is_ok()
                    {
                        next += 1;
                    }
                }
                next
            }
            "nohup" | "setsid" | "time" | "exec" | "xargs" => index + 1,
            // `bash scripts/run-scoped-tests.sh …` runs the script itself;
            // `bash -c '…'` is inspected by the caller instead.
            "sh" | "bash" | "zsh" | "dash"
                if words
                    .get(index + 1)
                    .is_some_and(|arg| !arg.starts_with('-')) =>
            {
                index + 1
            }
            _ => break,
        };
        if skip >= words.len() {
            return None;
        }
        index = skip;
        index += executable_word_index(&words[index..])?;
    }
    let command = shell_word_basename(&words[index]);
    let args = &words[index + 1..];
    match command {
        "cargo" => {
            // Skip `+toolchain` and global options, including those that take
            // a value (`-C dir`, `--config k=v`, `-Z flag`, `--color always`).
            let mut rest = args.iter();
            let subcommand = loop {
                let arg = rest.next()?;
                if matches!(
                    arg.as_str(),
                    "-C" | "--config" | "-Z" | "--color" | "--manifest-path" | "--target-dir"
                ) {
                    rest.next();
                    continue;
                }
                if arg.starts_with('-') || arg.starts_with('+') {
                    continue;
                }
                break arg;
            };
            CARGO_BUILD_SUBCOMMANDS
                .contains(&subcommand.as_str())
                .then(|| format!("cargo {subcommand}"))
        }
        "cargo-nextest" => Some("cargo-nextest".to_string()),
        "rustc" | "rustdoc" => {
            let informational = args.iter().any(|arg| {
                matches!(arg.as_str(), "--version" | "-V" | "--help" | "-h" | "-vV")
                    || arg.starts_with("--print")
            });
            (!informational).then(|| command.to_string())
        }
        "run-scoped-tests.sh" | "run-verified-tests.sh" | "refresh-worker-build-cache.sh" => {
            Some(command.to_string())
        }
        "make" | "gmake" => {
            let mut dir = cwd.to_path_buf();
            let mut makefile = None;
            let mut targets = Vec::new();
            let mut rest = args.iter();
            while let Some(arg) = rest.next() {
                if arg == "-C" {
                    dir = dir.join(rest.next()?);
                    continue;
                }
                if arg == "-f" {
                    makefile = rest.next().cloned();
                    continue;
                }
                if arg == "-j" {
                    rest.next();
                    continue;
                }
                if arg.starts_with('-') || arg.contains('=') {
                    continue;
                }
                if arg.starts_with("test") || arg == "build" || arg == "check" {
                    targets.push(arg.clone());
                }
            }
            // cas-cf70: a test target whose rule, read from the Makefile
            // itself, runs only scripts (no Cargo, no sub-make, no variable
            // that could expand to either) is not a Rust build. Anything the
            // Makefile cannot prove stays refused.
            targets
                .into_iter()
                .find(|target| !make_target_is_script_only(&dir, makefile.as_deref(), target))
                .map(|target| format!("make {target}"))
        }
        _ => None,
    }
}

/// cas-cf70: whether `target`'s rule in the Makefile under `dir` (or `-f`
/// `makefile`) runs only scripts. Every recipe line of the target and of its
/// prerequisite rules must be free of Rust builds and sub-makes and expand no
/// variable; a prerequisite without a rule must be an existing file. A missing
/// Makefile, an unknown target, or a cycle beyond four levels is not proof.
fn make_target_is_script_only(dir: &Path, makefile: Option<&str>, target: &str) -> bool {
    let path = match makefile {
        Some(makefile) => dir.join(makefile),
        None => ["GNUmakefile", "makefile", "Makefile"]
            .iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| dir.join("Makefile")),
    };
    let Ok(source) = std::fs::read_to_string(&path) else {
        return false;
    };
    let rules = makefile_rules(&source);
    let mut seen = Vec::new();
    make_rule_is_script_only(dir, &rules, target, &mut seen)
}

/// Explicit rules as (targets, prerequisites, recipe lines), with `\`
/// continuations joined. Assignments, conditionals, includes and
/// target-specific variables are not rules.
fn makefile_rules(source: &str) -> Vec<(Vec<String>, Vec<String>, Vec<String>)> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    for line in source.lines() {
        if let Some(head) = line.strip_suffix('\\') {
            pending.push_str(head);
            pending.push(' ');
            continue;
        }
        pending.push_str(line);
        lines.push(std::mem::take(&mut pending));
    }
    let mut rules: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();
    let mut in_recipe = false;
    for line in lines {
        if let Some(recipe) = line.strip_prefix('\t') {
            if in_recipe && let Some(rule) = rules.last_mut() {
                rule.2.push(recipe.trim().to_string());
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        in_recipe = false;
        let Some((head, tail)) = line.split_once(':') else {
            continue;
        };
        let tail = tail.trim_start_matches(':');
        if head.contains('=') || tail.trim_start().starts_with('=') || tail.contains('=') {
            continue;
        }
        let targets: Vec<String> = head.split_whitespace().map(str::to_string).collect();
        if targets.is_empty() || targets.iter().any(|target| target.contains('$')) {
            continue;
        }
        let (prerequisites, inline) = match tail.split_once(';') {
            Some((prerequisites, inline)) => (prerequisites, Some(inline.trim().to_string())),
            None => (tail, None),
        };
        let prerequisites = prerequisites
            .split_whitespace()
            .filter(|word| *word != "|")
            .map(str::to_string)
            .collect();
        rules.push((targets, prerequisites, inline.into_iter().collect()));
        in_recipe = true;
    }
    rules
}

fn make_rule_is_script_only(
    dir: &Path,
    rules: &[(Vec<String>, Vec<String>, Vec<String>)],
    target: &str,
    seen: &mut Vec<String>,
) -> bool {
    if seen.iter().any(|known| known == target) {
        return true;
    }
    if seen.len() >= 4 {
        return false;
    }
    seen.push(target.to_string());
    let matching: Vec<_> = rules
        .iter()
        .filter(|(targets, _, _)| targets.iter().any(|known| known == target))
        .collect();
    if matching.is_empty() {
        // A plain file prerequisite has no recipe to run.
        return seen.len() > 1 && dir.join(target).is_file();
    }
    matching.into_iter().all(|(_, prerequisites, recipe)| {
        recipe.iter().all(|line| {
            let line = line.trim_start_matches(['@', '-', '+']);
            // Sub-makes are refused before classification so a recipe that
            // re-invokes make cannot recurse through this check.
            !line.contains('$')
                && !line.contains('`')
                && !shell_statement_words(line).iter().any(|words| {
                    words.iter().any(|word| matches!(shell_word_basename(word), "make" | "gmake"))
                })
                && worker_command_rust_build_at_depth(line, dir, 0).is_none()
        }) && prerequisites
            .iter()
            .all(|prerequisite| make_rule_is_script_only(dir, rules, prerequisite, seen))
    })
}

/// Detect formatter invocations that can mutate files outside a worker's scope.
///
/// `cargo fmt` always selects Cargo targets rather than individual source files,
/// while direct `rustfmt` follows `mod` declarations unless `skip_children` is
/// enabled. Checks and stdout-only runs are non-mutating and remain available.
fn worker_command_runs_dangerous_formatter(command: &str) -> bool {
    shell_statement_words(command)
        .iter()
        .any(|words| formatter_invocation_can_spill(words))
}

fn formatter_invocation_can_spill(words: &[String]) -> bool {
    let Some(command_index) = executable_word_index(words) else {
        return false;
    };
    let command = shell_word_basename(&words[command_index]);
    if command == "cargo" {
        let mut cargo_args = &words[command_index + 1..];
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

    if command != "rustfmt" {
        return false;
    }
    let rustfmt_args = &words[command_index + 1..];
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

// ── Worker commit guard helpers (cas-bea2, LAYER 1) ───────────────────────
//
// Detects history-changing Git commands from factory workers and denies them
// when HEAD is on a protected branch OR the effective cwd/git-dir/work-tree
// is outside the assigned worktree (CAS_CLONE_PATH).
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

/// Return true if `cmd` looks like a history-changing Git invocation.
///
/// Matches common forms:
/// - `git commit -m "msg"`
/// - `git -C /some/path commit`
/// - `git merge main`
/// - `git rebase main`
/// - `git reset --hard HEAD~1`
/// - `git checkout feature/my-worker`
/// - `git add -A`
/// - `git push origin HEAD:refs/heads/factory/my-worker`
/// - Commands with env-var prefixes like `GIT_AUTHOR_NAME=... git commit`
///
/// Intentionally conservative: false-negatives (missed commands) are safe
/// because LAYER 2 (pre-commit hook) is the hard floor.
pub(crate) fn looks_like_git_write_op(cmd: &str) -> bool {
    !git_write_targets(".", cmd).is_empty()
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

/// cas-6db4: the delivery mode a factory worker's `git push origin` is judged
/// by. A worker with a task in progress follows that task's `delivery_mode`,
/// which is what the close gate enforces: push_branch tasks must reach origin
/// for their merge check, so the hook may not refuse their push. Only a worker
/// with no task in progress falls back to the session's mode.
pub(crate) fn effective_worker_delivery_mode(
    session_mode: cas_types::DeliveryMode,
    active_task_modes: &[cas_types::DeliveryMode],
) -> cas_types::DeliveryMode {
    if active_task_modes.is_empty() {
        return session_mode;
    }
    if active_task_modes.contains(&cas_types::DeliveryMode::PushBranch) {
        cas_types::DeliveryMode::PushBranch
    } else {
        cas_types::DeliveryMode::LocalMerge
    }
}

/// Delivery modes of the in-progress tasks assigned to this worker, by its
/// registered id or its pane name. Unreadable task state yields none, which
/// leaves the session's mode in charge (the pre-cas-6db4 behaviour).
fn worker_active_task_delivery_modes(
    stores: &mut ToolHookStores<'_>,
    input: &HookInput,
) -> Vec<cas_types::DeliveryMode> {
    let agent_id = current_agent_id(input);
    let agent_name = std::env::var("CAS_AGENT_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty());
    stores
        .tasks()
        .and_then(|store| store.list(Some(cas_types::TaskStatus::InProgress)).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            task.assignee.as_deref().is_some_and(|assignee| {
                assignee == agent_id || agent_name.as_deref() == Some(assignee)
            })
        })
        .map(|task| task.delivery_mode)
        .collect()
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

#[derive(Debug, Clone)]
struct GitCommandTarget {
    cwd: std::path::PathBuf,
    git_dir: Option<std::path::PathBuf>,
    work_tree: Option<std::path::PathBuf>,
}

impl GitCommandTarget {
    fn from_cwd(cwd: &str) -> Self {
        let cwd = if cwd.trim().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        } else {
            std::path::PathBuf::from(cwd)
        };
        Self {
            cwd,
            git_dir: None,
            work_tree: None,
        }
    }

    fn path_for_scope(&self) -> &std::path::Path {
        self.work_tree
            .as_deref()
            .or(self.git_dir.as_deref())
            .unwrap_or(&self.cwd)
    }
}

fn resolve_git_option_path(base: &std::path::Path, value: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn git_history_write_operation(git_words: &[String]) -> Option<usize> {
    // Only the subcommand can change history, never a global option's value
    // or a read command's operand (e.g. `git log --grep commit`).
    let mut operation_index = 0;
    while let Some(word) = git_words.get(operation_index) {
        match word.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" => {
                operation_index += 2;
            }
            "-h" | "--help" | "-v" | "--version" | "--exec-path" | "--html-path"
            | "--man-path" | "--info-path" => return None,
            "--" => {
                operation_index += 1;
                break;
            }
            _ if word.starts_with('-') => operation_index += 1,
            _ => break,
        }
    }
    let operation = git_words.get(operation_index)?.as_str();
    let args = &git_words[operation_index + 1..];
    let is_write = match operation {
        "commit" | "merge" | "push" | "rebase" | "cherry-pick" => true,
        "reset" => args.iter().any(|arg| arg == "--hard" || arg == "-H"),
        "checkout" => args.iter().any(|arg| !arg.starts_with('-')),
        "add" => args.iter().any(|arg| arg == "-A" || arg == "--all"),
        _ => false,
    };
    is_write.then_some(operation_index)
}

/// Resolve the small subset of shell variable forms used in worker guidance
/// before applying the worktree containment check. Unknown variables remain
/// literal, which fails closed if they would resolve outside the worktree.
fn resolve_worker_cd_path(base: &std::path::Path, value: &str) -> std::path::PathBuf {
    let value = match value {
        "$CAS_CLONE_PATH" | "${CAS_CLONE_PATH}" => {
            std::env::var_os("CAS_CLONE_PATH").map_or_else(|| value.to_string(), |v| v.to_string_lossy().into_owned())
        }
        _ => value.to_string(),
    };
    resolve_git_option_path(base, &value)
}

/// Extract every history-changing git invocation's repository target from a
/// shell command. Git's `-C`, `--git-dir`, and `--work-tree` options are
/// parsed before the operation so a linked worktree is checked instead of the
/// hook's cwd. Explicit `cd` statements are carried forward across `&&`, `;`,
/// and `|` statements because the hook receives the whole shell command.
fn git_write_targets(cwd: &str, command: &str) -> Vec<GitCommandTarget> {
    let default_target = GitCommandTarget::from_cwd(cwd);
    let mut targets = Vec::new();
    let mut shell_cwd = default_target.cwd.clone();

    // Strip only literal heredoc data for this allowance. Expanding bodies
    // can run $(git ...); retain the raw subprocess scopes in that case.
    let expanding_heredoc = shell_command_without_heredoc_bodies(command).lines().any(|line| {
        heredoc_delimiters(line).iter().any(|delimiter| !delimiter.literal_body)
    });
    let statements = if expanding_heredoc {
        super::attribution::split_shell_statements(command)
    } else {
        shell_statement_words(command)
    };
    let mut pending_statements = std::collections::VecDeque::from(statements);
    while let Some(words) = pending_statements.pop_front() {
        let Some(command_index) = unwrapped_command_index(&words) else {
            continue;
        };
        if shell_word_basename(&words[command_index]) == "find" {
            // find runs each -exec/-execdir payload as a command. Inspect
            // these scopes before later statements can change shell_cwd.
            for (index, word) in words.iter().enumerate().rev() {
                if matches!(word.as_str(), "-exec" | "-execdir") {
                    let payload = &words[index + 1..];
                    let end = payload.iter()
                        .position(|word| word == ";" || word == "+")
                        .unwrap_or(payload.len());
                    pending_statements.push_front(payload[..end].to_vec());
                }
            }
            continue;
        }
        if words[command_index] == "cd" {
            if let Some(path) = words.get(command_index + 1).filter(|path| path.as_str() != "--") {
                shell_cwd = resolve_worker_cd_path(&shell_cwd, path);
            } else if words.get(command_index + 1).is_some_and(|path| path == "--") {
                if let Some(path) = words.get(command_index + 2) {
                    shell_cwd = resolve_worker_cd_path(&shell_cwd, path);
                }
            }
            continue;
        }
        if shell_word_basename(&words[command_index]) != "git" {
            continue;
        }
        let git_words = &words[command_index + 1..];
        let Some(write_index) = git_history_write_operation(git_words) else {
            continue;
        };

        let mut target = default_target.clone();
        target.cwd = shell_cwd.clone();
        let mut index = 0;
        while index < write_index {
            let word = &git_words[index];
            if word == "-C" {
                if let Some(path) = git_words.get(index + 1) {
                    target.cwd = resolve_git_option_path(&target.cwd, path);
                    index += 2;
                    continue;
                }
                break;
            }
            if let Some(path) = word.strip_prefix("-C").filter(|path| !path.is_empty()) {
                target.cwd = resolve_git_option_path(&target.cwd, path);
                index += 1;
                continue;
            }
            if word == "--git-dir" {
                if let Some(path) = git_words.get(index + 1) {
                    target.git_dir = Some(resolve_git_option_path(&target.cwd, path));
                    index += 2;
                    continue;
                }
                break;
            }
            if let Some(path) = word.strip_prefix("--git-dir=") {
                target.git_dir = Some(resolve_git_option_path(&target.cwd, path));
                index += 1;
                continue;
            }
            if word == "--work-tree" {
                if let Some(path) = git_words.get(index + 1) {
                    target.work_tree = Some(resolve_git_option_path(&target.cwd, path));
                    index += 2;
                    continue;
                }
                break;
            }
            if let Some(path) = word.strip_prefix("--work-tree=") {
                target.work_tree = Some(resolve_git_option_path(&target.cwd, path));
                index += 1;
                continue;
            }
            index += 1;
        }
        targets.push(target);
    }

    targets
}

fn git_command_at_target(target: &GitCommandTarget) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    if let Some(git_dir) = &target.git_dir {
        command.args(["--git-dir"]).arg(git_dir);
    } else {
        command.args(["-C"]).arg(&target.cwd);
    }
    if let Some(work_tree) = &target.work_tree {
        command.args(["--work-tree"]).arg(work_tree);
    }
    command
}

fn get_branch_at_git_target(target: &GitCommandTarget) -> Option<String> {
    let mut command = git_command_at_target(target);
    let output = command
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// During a rebase Git detaches HEAD, but records the branch being rebased in
/// one of these metadata files. Continuation commands must be checked against
/// that branch just like any other history-changing operation; a detached HEAD
/// without either file remains a hard refusal.
fn get_branch_at_rebase_git_target(target: &GitCommandTarget) -> Option<String> {
    for metadata_path in ["rebase-merge/head-name", "rebase-apply/head-name"] {
        let Ok(output) = git_command_at_target(target)
            .args(["rev-parse", "--git-path", metadata_path])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(&path);
        let path = if path.is_absolute() {
            path
        } else {
            target.path_for_scope().join(path)
        };
        let Ok(branch) = std::fs::read_to_string(path) else {
            continue;
        };
        let branch = branch.trim();
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch).trim();
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }
    None
}

fn check_worker_git_commit_scope_for_command(cwd: &str, command: &str) -> Option<String> {
    worker_git_scope_violation_for_command(cwd, command).map(|(_, message)| message)
}

fn worker_git_scope_violation_for_command(
    cwd: &str,
    command: &str,
) -> Option<(GitCommandTarget, String)> {
    worker_git_scope_violation_for_command_with_worktree(cwd, command, None)
}

fn worker_git_scope_violation_for_command_with_worktree(
    cwd: &str,
    command: &str,
    registered_worktree_root: Option<&std::path::Path>,
) -> Option<(GitCommandTarget, String)> {
    git_write_targets(cwd, command)
        .into_iter()
        .find_map(|target| {
            check_worker_git_commit_scope_at_target(&target, registered_worktree_root)
                .map(|message| (target, message))
        })
}

/// Check whether a factory worker's history-changing Git operation
/// should be denied.
///
/// Returns `Some(denial_message)` when:
/// - HEAD at `cwd` is a protected branch (`main`, `master`, `staging`) or detached.
/// - the registered worker worktree (or `CAS_CLONE_PATH`) exists AND `cwd` is
///   outside that worktree.
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
    check_worker_git_commit_scope_at_target(&GitCommandTarget::from_cwd(cwd), None)
}

fn check_worker_git_commit_scope_at_target(
    target: &GitCommandTarget,
    registered_worktree_root: Option<&std::path::Path>,
) -> Option<String> {
    let cwd = target.path_for_scope().display().to_string();
    let clone_path = registered_worktree_root
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| std::env::var("CAS_CLONE_PATH").ok());
    let is_isolated = clone_path
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // DENY: a history-changing command's effective cwd/git-dir/work-tree is
    // outside the assigned worktree. This applies to every factory worker
    // with a registered CAS_CLONE_PATH, including commands that first `cd`
    // into the primary checkout. Do not rely on string prefixes: sibling
    // paths such as `/worktree-sibling` are outside `/worktree`.
    if is_isolated {
        let clone_path = clone_path.as_deref().unwrap();
        let cwd_path = std::path::Path::new(&cwd);
        let worktree_path = std::path::Path::new(clone_path);

        let inside_worktree = canonicalize_for_containment(cwd_path)
            .zip(canonicalize_for_containment(worktree_path))
            .is_some_and(|(cwd, worktree)| cwd.starts_with(worktree));

        if !inside_worktree {
            let worker_name =
                std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());
            let lane = format!("factory/{worker_name}");
            let [change_dir, switch] = match crate::worktree::GitOperations::shell_remedies([
                ("cd", &[clone_path][..]),
                ("git", &["switch", &lane]),
            ]) {
                Ok(commands) => commands,
                Err(refusal) => return Some(refusal),
            };
            let cwd = crate::worktree::GitOperations::visible_git_label(&cwd);
            let clone_path = crate::worktree::GitOperations::visible_git_label(clone_path);
            return Some(format!(
                "🚫 WORKER COMMIT GUARD: Your current directory ({cwd}) is outside \
                your assigned worktree ({clone_path}).\n\n\
                Workers MUST commit inside their worktree. Switch first:\n  \
                {change_dir}\n  {switch}\n\n\
                Then retry your commit from there.\n\n\
                Note: --no-verify does NOT bypass this guard (it only skips git hooks,\n\
                not the Claude Code PreToolUse harness)."
            ));
        }
    }

    // Resolve HEAD once. Protected branches retain the established WORKER
    // COMMIT GUARD contract before identity-specific checks run. A rebase is
    // the one valid detached-HEAD case: Git records the original branch in
    // rebase-merge/head-name or rebase-apply/head-name, so continuation
    // commands can still be checked against the worker branch. A detached
    // HEAD without rebase metadata remains refused.
    let worker_name =
        std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());

    let branch = match get_branch_at_git_target(target)
        .or_else(|| get_branch_at_rebase_git_target(target))
    {
        None => {
            let lane = format!("factory/{worker_name}");
            let [switch] = match crate::worktree::GitOperations::git_remedies([&["switch", &lane]])
            {
                Ok(commands) => commands,
                Err(refusal) => return Some(refusal),
            };
            return Some(format!(
                "🚫 WORKER COMMIT GUARD: HEAD is detached — cannot determine branch.\n\n\
                Commits require a named branch. Switch to your work branch first:\n  \
                {switch}   # or your feature/fix branch\n\n\
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
            let lane = format!("factory/{worker_name}");
            let path = format!("../factory-{worker_name}");
            let [
                switch,
                commit,
                add,
                change_dir,
                create,
                push,
                trunk,
                trunk_push,
                ff,
            ] = match crate::worktree::GitOperations::shell_remedies([
                ("git", &["switch", &lane][..]),
                ("git", &["commit", "..."]),
                ("git", &["worktree", "add", &path, "-b", &lane]),
                ("cd", &[&path]),
                ("git", &["switch", "-c", &lane]),
                ("git", &["push", "-u", "origin", &lane]),
                ("git", &["switch", &branch]),
                ("git", &["push", "origin", &branch]),
                ("git", &["merge", "--ff-only"]),
            ]) {
                Ok(commands) => commands,
                Err(refusal) => return Some(refusal),
            };
            let branch = crate::worktree::GitOperations::visible_git_label(&branch);
            let remedy = if is_isolated {
                format!(
                    "Switch to your work branch and commit there:\n  \
                    {switch}   # or your feature branch\n  \
                    git commit ...\n"
                )
            } else {
                format!(
                    "\nYou are running without an isolated worktree (CAS_CLONE_PATH not set), so \
                    this is the SHARED checkout.\n\
                    Its HEAD is read by every other agent here and by the supervisor's \
                    merge/tag sequence — do NOT leave it pointing at a factory branch.\n\n\
                    PREFERRED — keep shared HEAD on '{branch}' and work in your own worktree:\n  \
                    {add}\n  \
                    {change_dir}   # commit and push from there\n\n\
                    FALLBACK — if you must branch in place, RESTORE TRUNK AFTER PUSH:\n  \
                    {create}\n  \
                    {commit} && {push}\n  \
                    {trunk}   # MANDATORY: a shared checkout left on factory/* \
                    makes `{ff}` and `{trunk_push}` silently \
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
                    &cwd,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellToken {
    Word(String),
    Operator(char),
}

fn worker_command_pattern_kill(command: &str) -> bool {
    worker_command_pattern_kill_at_depth(command, 0)
}

fn worker_command_pattern_kill_at_depth(command: &str, depth: usize) -> bool {
    let command = shell_command_without_heredoc_bodies(command);
    let mut group = Vec::new();
    let mut words = Vec::new();
    let mut piped = false;
    for token in factory_shell_tokens(&command) {
        match token {
            ShellToken::Word(word) => words.push(word),
            ShellToken::Operator(operator @ (';' | '&' | '|' | '(' | ')')) => {
                if !words.is_empty() {
                    group.push(std::mem::take(&mut words));
                }
                if matches!(operator, ';' | '&') {
                    if worker_pattern_kill_group(&group, piped, depth) {
                        return true;
                    }
                    group.clear();
                    piped = false;
                } else if operator == '|' {
                    piped = true;
                }
            }
            ShellToken::Operator(_) => {}
        }
    }
    if !words.is_empty() {
        group.push(words);
    }
    worker_pattern_kill_group(&group, piped, depth)
}

/// Keep a pipeline/substitution together, but separate independent commands:
/// a diagnostic `pgrep ...; kill 123` does not derive the signal's PID.
fn worker_pattern_kill_group(group: &[Vec<String>], piped: bool, depth: usize) -> bool {
    let mut searches = false;
    let mut signals = false;
    let mut dynamic_signal = false;
    for words in group {
        // env -S's executable lives inside its split string; the ordinary
        // wrapper resolver can legitimately return None for this statement.
        if depth < 4
            && let Some(script) = env_split_string_payload(words)
            && worker_command_pattern_kill_at_depth(&script, depth + 1)
        {
            return true;
        }
        let Some(index) = unwrapped_command_index(words) else { continue };
        let name = shell_word_basename(&words[index]);
        let args = &words[index + 1..];
        match name {
            "pkill" | "killall" => return true,
            "pgrep" => searches = true,
            "kill" => {
                signals = true;
                dynamic_signal |= args.iter().any(|arg| arg.contains(['$', '`']));
                for arg in args {
                    // Quotes retain the whole substitution in one word;
                    // unquoted substitutions are separate statements above.
                    let substitutions = arg.split("$(").skip(1)
                        .chain(arg.split('`').skip(1).step_by(2));
                    for payload in substitutions {
                        if shell_statement_words(payload).iter().any(|words| {
                            unwrapped_command_index(words)
                                .is_some_and(|i| shell_word_basename(&words[i]) == "pgrep")
                        }) {
                            return true;
                        }
                    }
                }
            }
            "sh" | "bash" | "dash" | "zsh" if depth < 4 => {
                if let Some(flag) = args.iter().position(|arg| {
                    arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c')
                })
                    && let Some(script) = args.get(flag + 1)
                    && worker_command_pattern_kill_at_depth(script, depth + 1)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    searches && signals && (piped || dynamic_signal)
}

/// Split just enough shell syntax to distinguish command arguments from
/// redirections. In particular, metacharacters inside a quoted commit message
/// stay inside one word and can never be mistaken for file-creation syntax.
fn factory_shell_tokens(command: &str) -> Vec<ShellToken> {
    factory_shell_tokens_with_literals(command).0
}

/// Preserve whether the outer shell can change a word's program structure.
/// Single-quoted words are literal; numeric status/PID parameters cannot inject
/// shell syntax. The effect collector still checks their use as commands/paths.
fn factory_shell_tokens_with_literals(command: &str) -> (Vec<ShellToken>, Vec<bool>) {
    let mut tokens = Vec::new();
    let mut literals = Vec::new();
    let mut word = String::new();
    let mut literal = true;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let push_word = |tokens: &mut Vec<ShellToken>, literals: &mut Vec<bool>, word: &mut String, literal: &mut bool| {
        if !word.is_empty() {
            tokens.push(ShellToken::Word(std::mem::take(word)));
            literals.push(*literal);
            *literal = true;
        }
    };
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if comment {
            if matches!(ch, '\n' | '\r') {
                comment = false;
                push_word(&mut tokens, &mut literals, &mut word, &mut literal);
                tokens.push(ShellToken::Operator(';'));
                literals.push(true);
            }
            continue;
        }
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        // A substitution has its own quoting context: its double quotes must
        // not close the outer word or turn quoted parentheses into scopes.
        // Preserve its source for the recursive effect collector, which still
        // judges both the commands it runs and how its output is used.
        if quote == Some('"') && ch == '$' && chars.peek() == Some(&'(') {
            let tail = format!("${}", chars.clone().collect::<String>());
            if let Some(end) = shell_effects::literal_cat_heredoc_end(&tail)
                .or_else(|| shell_effects::substitution_end(tail.as_bytes(), 2, 0))
            {
                let substitution = &tail[..=end];
                word.push_str(substitution);
                for _ in 1..substitution.chars().count() {
                    chars.next();
                }
                literal = false;
                continue;
            }
        }
        if quote != Some('\'') {
            if ch == '$' && chars.peek().is_some_and(|next| matches!(next, '?' | '$' | '!')) {
                word.push(ch);
                word.push(chars.next().unwrap());
                continue;
            }
            if matches!(ch, '$' | '`') || (quote.is_none() && matches!(ch, '*' | '?' | '[')) {
                literal = false;
            }
        }
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => word.push(ch),
            None => match ch {
                '\\' => escaped = true,
                '\'' | '"' => quote = Some(ch),
                '#' if word.is_empty() => comment = true,
                '>' | '<' | '|' | '&' | ';' | '(' | ')' => {
                    push_word(&mut tokens, &mut literals, &mut word, &mut literal);
                    tokens.push(ShellToken::Operator(ch));
                    literals.push(true);
                }
                '\n' | '\r' => {
                    push_word(&mut tokens, &mut literals, &mut word, &mut literal);
                    tokens.push(ShellToken::Operator(';'));
                    literals.push(true);
                }
                ch if ch.is_whitespace() => push_word(&mut tokens, &mut literals, &mut word, &mut literal),
                _ => word.push(ch),
            },
        }
    }
    if escaped {
        word.push('\\');
    }
    push_word(&mut tokens, &mut literals, &mut word, &mut literal);
    (tokens, literals)
}

/// Return shell statements with heredoc bodies and comments removed. Quoted
/// arguments remain one word, so a formatter name in a script string cannot be
/// mistaken for the executable command.
///
/// cas-b7a1: a `$(…)` that is an argument of a resolved command does not end
/// that command. Its body is a statement of its own, and the words after its
/// `)` stay in the enclosing statement (`echo $(cat f) sed -i x c` runs `echo`).
/// A substitution that may become the command (first word, after an
/// assignment or a wrapper such as `env` or `timeout`), a group or subshell,
/// and an unmatched `)` (a `case` pattern) still end the statement as before.
pub(super) fn shell_statement_words(command: &str) -> Vec<Vec<String>> {
    let shell_command = shell_command_without_heredoc_bodies(command);
    let tokens = factory_shell_tokens(&shell_command);
    let mut statements = Vec::new();
    let mut words: Vec<String> = Vec::new();
    // One entry per open `(`: the enclosing statement an argument
    // substitution returns to, or `None` when `)` starts a new statement.
    let mut open: Vec<Option<Vec<String>>> = Vec::new();
    for token in tokens {
        match token {
            ShellToken::Word(word) => words.push(word),
            ShellToken::Operator('(') => {
                let argument_substitution = words.last().is_some_and(|word| word.ends_with('$'))
                    && unwrapped_command_index(&words).is_some_and(|index| index + 1 < words.len());
                if argument_substitution {
                    open.push(Some(std::mem::take(&mut words)));
                } else {
                    if !words.is_empty() {
                        statements.push(std::mem::take(&mut words));
                    }
                    open.push(None);
                }
            }
            ShellToken::Operator(')') => {
                if !words.is_empty() {
                    statements.push(std::mem::take(&mut words));
                }
                if let Some(Some(enclosing)) = open.pop() {
                    words = enclosing;
                }
            }
            ShellToken::Operator(';') | ShellToken::Operator('|') | ShellToken::Operator('&') => {
                if !words.is_empty() {
                    statements.push(std::mem::take(&mut words));
                }
            }
            ShellToken::Operator(_) => {}
        }
    }
    if !words.is_empty() {
        statements.push(words);
    }
    // An unclosed argument substitution still reports its enclosing words.
    for enclosing in open.into_iter().flatten().rev() {
        if !enclosing.is_empty() {
            statements.push(enclosing);
        }
    }
    statements
}

fn shell_word_basename(word: &str) -> &str {
    std::path::Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(word)
}

/// Find the executable word after the small set of shell wrappers commonly
/// used by worker commands. This is deliberately not a shell evaluator.
/// `env` options whose value is the following word (cas-cfd6).
const ENV_VALUE_OPTIONS: &[&str] = &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"];

/// `sudo` options whose value is the following word (cas-cfd6).
const SUDO_VALUE_OPTIONS: &[&str] = &[
    "-u", "--user", "-g", "--group", "-C", "--close-from", "-D", "--chdir", "-h", "--host",
    "-p", "--prompt", "-r", "--role", "-t", "--type", "-T", "--command-timeout", "-U",
    "--other-user", "-R", "--chroot",
];

/// The command string an `env -S STRING` / `--split-string[=]STRING` runs,
/// for statements whose executable position is `env` (cas-cfd6).
fn env_split_string_payload(words: &[String]) -> Option<String> {
    // Only an `env` in command position: after leading assignments and the
    // wrappers the build guard already unwraps, never a quoted argument.
    let start = words.iter().position(|word| {
        !(word
            .split_once('=')
            .is_some_and(|(name, _)| is_shell_variable_name(name))
            || matches!(
                shell_word_basename(word),
                "sudo" | "command" | "nohup" | "setsid" | "time" | "exec" | "nice" | "timeout"
            )
            || word.starts_with('-')
            || word.parse::<f64>().is_ok())
    })?;
    if shell_word_basename(&words[start]) != "env" {
        return None;
    }
    let mut index = start + 1;
    while index < words.len() {
        let word = words[index].as_str();
        if let Some(value) = word.strip_prefix("--split-string=") {
            return Some(value.to_string());
        }
        if word == "-S" || word == "--split-string" {
            return words.get(index + 1).cloned();
        }
        if let Some(value) = word.strip_prefix("-S").filter(|value| !value.is_empty()) {
            return Some(value.to_string());
        }
        if word == "--" || !(word.starts_with('-') || word.contains('=')) {
            return None;
        }
        index += if ENV_VALUE_OPTIONS.contains(&word) { 2 } else { 1 };
    }
    None
}

pub(super) fn executable_word_index(words: &[String]) -> Option<usize> {
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        match shell_word_basename(word) {
            "!" | "if" | "then" | "else" | "elif" | "do" => index += 1,
            "env" => {
                // cas-cfd6: `-u NAME`, `-C DIR` and `-S STRING` take a separate
                // value; skipping only the flag made `env -u X cargo …` name
                // `X` as the executable and miss the worker Rust build guard.
                // An `-S` payload is itself inspected by the build guard.
                index += 1;
                while index < words.len() {
                    let word = words[index].as_str();
                    if word == "--" {
                        index += 1;
                        break;
                    } else if ENV_VALUE_OPTIONS.contains(&word) {
                        index += 2;
                    } else if word.starts_with('-')
                        || word
                            .split_once('=')
                            .is_some_and(|(name, _)| is_shell_variable_name(name))
                    {
                        index += 1;
                    } else {
                        break;
                    }
                }
            }
            "sudo" => {
                index += 1;
                while index < words.len() && words[index].starts_with('-') {
                    if words[index] == "--" {
                        index += 1;
                        break;
                    }
                    index += if SUDO_VALUE_OPTIONS.contains(&words[index].as_str()) {
                        2
                    } else {
                        1
                    };
                }
            }
            "command" => index += 1,
            _ if word
                .split_once('=')
                .is_some_and(|(name, _)| is_shell_variable_name(name)) =>
            {
                index += 1;
            }
            _ => return Some(index),
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum ScriptToken {
    Identifier(String),
    String(String),
    /// cas-becd (GH #922): a JavaScript template literal with `${...}`
    /// interpolation, raw. Its value is JavaScript-computed, never a shell
    /// word, so it is resolved by [`script_template_target`], not by the
    /// shell variable expander.
    Template(String),
    Punctuation(char),
}

/// Tokenize only enough Python/JavaScript syntax to identify known write APIs.
/// Comments and string contents are opaque; in particular, `</script>`, Vue
/// interpolation text, and template literals cannot become path tokens.
fn script_tokens(script: &str) -> Vec<ScriptToken> {
    let chars: Vec<char> = script.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        if ch == '#' {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            let quote = ch;
            let triple = quote != '`'
                && chars.get(index + 1) == Some(&quote)
                && chars.get(index + 2) == Some(&quote);
            let prefix = if triple { 3 } else { 1 };
            index += prefix;
            let start = index;
            while index < chars.len() {
                if chars[index] == '\\' {
                    index = (index + 2).min(chars.len());
                    continue;
                }
                let closed = if triple {
                    chars.get(index) == Some(&quote)
                        && chars.get(index + 1) == Some(&quote)
                        && chars.get(index + 2) == Some(&quote)
                } else {
                    chars[index] == quote
                };
                if closed {
                    break;
                }
                index += 1;
            }
            let value: String = chars[start..index].iter().collect();
            if quote == '`' && value.contains("${") {
                tokens.push(ScriptToken::Template(value));
            } else {
                tokens.push(ScriptToken::String(value));
            }
            index = (index + prefix).min(chars.len());
            continue;
        }
        if ch == '_' || ch.is_ascii_alphanumeric() || ch == '$' {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index] == '_'
                    || chars[index].is_ascii_alphanumeric()
                    || chars[index] == '$')
            {
                index += 1;
            }
            tokens.push(ScriptToken::Identifier(
                chars[start..index].iter().collect(),
            ));
            continue;
        }
        tokens.push(ScriptToken::Punctuation(ch));
        index += 1;
    }
    tokens
}

fn script_assignment_values(tokens: &[ScriptToken]) -> std::collections::HashMap<String, String> {
    let mut assignments = std::collections::HashMap::new();
    for index in 0..tokens.len() {
        let ScriptToken::Identifier(name) = &tokens[index] else {
            continue;
        };
        if !matches!(tokens.get(index + 1), Some(ScriptToken::Punctuation('='))) {
            continue;
        }
        match tokens.get(index + 2) {
            Some(ScriptToken::String(value)) => {
                assignments.insert(name.clone(), value.clone());
            }
            Some(ScriptToken::Identifier(path_type)) if path_type == "Path" => {
                if let Some(ScriptToken::String(value)) = tokens.get(index + 4) {
                    if matches!(tokens.get(index + 3), Some(ScriptToken::Punctuation('('))) {
                        assignments.insert(name.clone(), value.clone());
                    }
                }
            }
            _ => {}
        }
    }
    assignments
}

fn script_target_value(
    tokens: &[ScriptToken],
    index: usize,
    assignments: &std::collections::HashMap<String, String>,
) -> Option<String> {
    match tokens.get(index)? {
        ScriptToken::String(value) => Some(value.clone()),
        ScriptToken::Identifier(name) => assignments.get(name).cloned(),
        ScriptToken::Template(raw) => script_template_target(raw, assignments),
        _ => None,
    }
}

/// cas-becd (GH #922): the containment-checkable form of a template-literal
/// write target such as `${file}.${process.pid}.tmp`.
///
/// Interpolations naming a string the script assigned are substituted. A
/// target whose location is decided by an unknown value (it starts with an
/// unresolved interpolation) is as opaque as a bare unknown identifier and
/// yields `None`, exactly like `writeFileSync(file)`. Otherwise the static
/// prefix fixes the location (`/tmp/${name}` stays under /tmp, `out/${name}`
/// under the worktree), and each unresolved interpolation after it becomes a
/// neutral path segment so the shell expander never sees JavaScript `${}`.
fn script_template_target(
    raw: &str,
    assignments: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut resolved = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            let mut depth = 0usize;
            let mut end = None;
            for (offset, ch) in chars[index + 1..].iter().enumerate() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(index + 1 + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            // An unterminated interpolation is not a path we can reason about.
            let end = end?;
            let expression: String = chars[index + 2..end].iter().collect();
            match assignments.get(expression.trim()) {
                Some(value) => resolved.push_str(value),
                None if resolved.is_empty() => return None,
                None => resolved.push('_'),
            }
            index = end + 1;
            continue;
        }
        resolved.push(chars[index]);
        index += 1;
    }
    Some(resolved)
}

fn script_call_end(tokens: &[ScriptToken], open: usize) -> Option<usize> {
    if !matches!(tokens.get(open), Some(ScriptToken::Punctuation('('))) {
        return None;
    }
    let mut depth = 0;
    for index in open..tokens.len() {
        match tokens[index] {
            ScriptToken::Punctuation('(') => depth += 1,
            ScriptToken::Punctuation(')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn script_method_is_write(method: &str) -> bool {
    matches!(
        method,
        "write_text"
            | "write_bytes"
            | "writeFile"
            | "writeFileSync"
            | "appendFile"
            | "appendFileSync"
            | "createWriteStream"
    )
}

fn script_node_method_is_write(method: &str) -> bool {
    matches!(
        method,
        "writeFile" | "writeFileSync" | "appendFile" | "appendFileSync" | "createWriteStream"
    )
}

fn script_mode_is_write(mode: &str) -> bool {
    mode.starts_with('w') || mode.starts_with('a') || mode.starts_with('x')
}

fn script_write_targets(script: &str) -> Vec<String> {
    let tokens = script_tokens(script);
    let assignments = script_assignment_values(&tokens);
    let mut targets = Vec::new();
    for index in 0..tokens.len() {
        let ScriptToken::Identifier(name) = &tokens[index] else {
            continue;
        };
        let Some(ScriptToken::Punctuation('(')) = tokens.get(index + 1) else {
            continue;
        };
        let Some(end) = script_call_end(&tokens, index + 1) else {
            continue;
        };
        if name == "open" {
            let target = script_target_value(&tokens, index + 2, &assignments);
            let mode = script_target_value(&tokens, index + 4, &assignments);
            if mode.as_deref().is_some_and(script_mode_is_write) {
                if let Some(target) = target {
                    targets.push(target);
                }
            }
            continue;
        }
        if name == "Path" {
            let Some(ScriptToken::Punctuation(')')) = tokens.get(end) else {
                continue;
            };
            let Some(ScriptToken::Punctuation('.')) = tokens.get(end + 1) else {
                continue;
            };
            let Some(ScriptToken::Identifier(method)) = tokens.get(end + 2) else {
                continue;
            };
            if script_method_is_write(method) {
                if let Some(target) = script_target_value(&tokens, index + 2, &assignments) {
                    targets.push(target);
                }
            }
            continue;
        }
        if !script_node_method_is_write(name) {
            continue;
        }
        if let Some(target) = script_target_value(&tokens, index + 2, &assignments) {
            targets.push(target);
        }
    }

    // `path.write_text(...)` and `path.open("w")` use the path object on the
    // left of the method call rather than passing the path as an argument.
    for index in 2..tokens.len() {
        let (Some(ScriptToken::Punctuation('.')), Some(ScriptToken::Identifier(method))) =
            (tokens.get(index - 1), tokens.get(index))
        else {
            continue;
        };
        if !script_method_is_write(method) && method != "open" {
            continue;
        }
        let ScriptToken::Identifier(object) = &tokens[index - 2] else {
            continue;
        };
        let Some(ScriptToken::Punctuation('(')) = tokens.get(index + 1) else {
            continue;
        };
        if method == "open"
            && !script_target_value(&tokens, index + 2, &assignments)
                .as_deref()
                .is_some_and(script_mode_is_write)
        {
            continue;
        }
        if let Some(target) = assignments.get(object) {
            targets.push(target.clone());
        }
    }
    targets
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
            // Shell assignments expand their RHS using values already bound:
            // `R=/artifacts; P=$R/task; mkdir -p $P` must not leave `$R`
            // inside the target. Unknown values still remain unresolved.
            let expanded = expand_factory_shell_word(value, &values);
            values.insert(name.to_string(), expanded);
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

#[derive(Debug, PartialEq, Eq)]
struct HeredocDelimiter {
    value: String,
    strip_leading_tabs: bool,
    literal_body: bool,
}

/// Find heredoc delimiters on one shell command line without interpreting
/// quoted `<<` text as shell syntax. The returned order matches the shell's
/// order for multiple heredocs on the same line.
fn heredoc_delimiters(line: &str) -> Vec<HeredocDelimiter> {
    let chars: Vec<char> = line.chars().collect();
    let mut delimiters = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '\\' => index = (index + 2).min(chars.len()),
            '\'' | '"' => {
                let quote = chars[index];
                index += 1;
                while index < chars.len() && chars[index] != quote {
                    if chars[index] == '\\' {
                        index = (index + 2).min(chars.len());
                    } else {
                        index += 1;
                    }
                }
                if index < chars.len() {
                    index += 1;
                }
            }
            '<' if chars.get(index + 1) == Some(&'<') => {
                // `<<<` is a Bash here-string, not a heredoc declaration.
                if chars.get(index + 2) == Some(&'<') {
                    index += 3;
                    continue;
                }
                index += 2;
                let strip_leading_tabs = chars.get(index) == Some(&'-');
                if strip_leading_tabs {
                    index += 1;
                }
                while chars.get(index).is_some_and(|ch| ch.is_whitespace()) {
                    index += 1;
                }
                let Some(&first) = chars.get(index) else {
                    continue;
                };
                let (value, next_index) = if first == '\'' || first == '"' {
                    index += 1;
                    let start = index;
                    while index < chars.len() && chars[index] != first {
                        index += 1;
                    }
                    if index >= chars.len() {
                        continue;
                    }
                    (chars[start..index].iter().collect::<String>(), index + 1)
                } else {
                    let start = index;
                    while index < chars.len()
                        && !chars[index].is_whitespace()
                        && !matches!(chars[index], ';' | '|' | '&' | '<' | '>')
                    {
                        index += 1;
                    }
                    (chars[start..index].iter().collect(), index)
                };
                if !value.is_empty() {
                    delimiters.push(HeredocDelimiter {
                        value,
                        strip_leading_tabs,
                        literal_body: matches!(first, '\'' | '"'),
                    });
                }
                index = next_index;
            }
            _ => index += 1,
        }
    }

    delimiters
}

/// Remove heredoc bodies before shell-token extraction. Heredoc contents are
/// data for the command receiving them, not shell commands or redirect
/// operands. The declaration line and any shell commands after the delimiter
/// remain available to the workspace guard.
fn shell_command_without_heredoc_bodies(command: &str) -> String {
    let mut pending: std::collections::VecDeque<HeredocDelimiter> =
        std::collections::VecDeque::new();
    let mut shell_command = String::new();

    for line in command.split_inclusive('\n') {
        if let Some(delimiter) = pending.front() {
            let candidate = line.trim_end_matches('\n').trim_end_matches('\r');
            let candidate = if delimiter.strip_leading_tabs {
                candidate.trim_start_matches('\t')
            } else {
                candidate
            };
            if candidate == delimiter.value {
                pending.pop_front();
            }
            continue;
        }

        shell_command.push_str(line);
        pending.extend(heredoc_delimiters(line));
    }

    shell_command
}

pub(super) fn shell_heredoc_bodies(command: &str) -> Vec<String> {
    let mut pending: std::collections::VecDeque<(HeredocDelimiter, String)> =
        std::collections::VecDeque::new();
    let mut bodies = Vec::new();
    for line in command.split_inclusive('\n') {
        if let Some((delimiter, body)) = pending.front_mut() {
            let candidate = line.trim_end_matches('\n').trim_end_matches('\r');
            let candidate = if delimiter.strip_leading_tabs {
                candidate.trim_start_matches('\t')
            } else {
                candidate
            };
            let is_delimiter = candidate == delimiter.value;
            if is_delimiter {
                let (_, body) = pending.pop_front().expect("heredoc body is pending");
                bodies.push(body);
            } else {
                body.push_str(line);
            }
            continue;
        }
        pending.extend(
            heredoc_delimiters(line)
                .into_iter()
                .map(|delimiter| (delimiter, String::new())),
        );
    }
    bodies
}

/// Extract targets from known script write APIs only. The script itself is
/// opaque to shell tokenization; this bounded parser recognizes Python/Node
/// invocations and never treats arbitrary source strings as filesystem paths.
fn script_write_targets_from_command(command: &str) -> Vec<String> {
    let mut heredoc_bodies = shell_heredoc_bodies(command).into_iter();
    let mut targets = Vec::new();
    for words in shell_statement_words(command) {
        let Some(command_index) = executable_word_index(&words) else {
            continue;
        };
        let executable = shell_word_basename(&words[command_index]);
        if !matches!(executable, "python" | "python3" | "node") {
            continue;
        }
        let args = &words[command_index + 1..];
        let script = args.windows(2).find_map(|pair| {
            if matches!(pair[0].as_str(), "-c" | "-e") {
                Some(pair[1].clone())
            } else {
                None
            }
        });
        let script = script.or_else(|| {
            args.iter()
                .position(|arg| arg == "-")
                .and_then(|_| heredoc_bodies.next())
        });
        if let Some(script) = script {
            targets.extend(script_write_targets(&script));
        }
    }
    targets
}

/// cas-a673: the command position a `)` returns to. A `$(…)` substitution
/// closes back into the position its `$` word started in: inside an argument
/// (`echo $(cat f) touch x`) the following words are still arguments, while
/// at command position (`$(true) touch x`, which runs `touch` when the
/// substitution expands to nothing) they may be the command. A bare group or
/// subshell, and an unmatched `)` such as a `case` pattern, return to command
/// position as before.
#[derive(Default)]
struct ParenPositions {
    open: Vec<bool>,
    /// Command position in effect when the previous word token began.
    word_entry: Option<bool>,
}

impl ParenPositions {
    fn word(&mut self, command_position: bool) {
        self.word_entry = Some(command_position);
    }

    fn open(&mut self, tokens: &[ShellToken], index: usize) {
        let substitution = index
            .checked_sub(1)
            .and_then(|previous| tokens.get(previous))
            .is_some_and(|token| matches!(token, ShellToken::Word(word) if word.ends_with('$')));
        let resume = match self.word_entry {
            Some(entry) if substitution => entry,
            _ => true,
        };
        self.open.push(resume);
        self.word_entry = None;
    }

    fn close(&mut self) -> bool {
        self.word_entry = None;
        self.open.pop().unwrap_or(true)
    }

    fn operator(&mut self) {
        self.word_entry = None;
    }
}

/// Identify shell words that are actual output targets, without treating every
/// path-shaped argument as a write. This deliberately handles only the small
/// set of write forms guarded by the factory workspace contract; unrecognised
/// shell syntax is left to the shell rather than guessed at.
fn bash_write_targets(command: &str) -> Vec<String> {
    let shell_command = shell_command_without_heredoc_bodies(command);
    let tokens = factory_shell_tokens(&shell_command);
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
    for target in script_write_targets_from_command(command) {
        add_target(&target);
    }

    // Cover the explicit file-creation commands guarded by the original
    // workspace contract. Only a command-position word is considered; prose
    // passed to `git commit -m` or `git merge -m` is never reinterpreted as a
    // command. This is intentionally a small recognizer, not a shell parser.
    let mut index = 0;
    let mut command_position = true;
    let mut parens = ParenPositions::default();
    while index < tokens.len() {
        match &tokens[index] {
            ShellToken::Operator('(') => {
                parens.open(&tokens, index);
                command_position = true;
                index += 1;
            }
            ShellToken::Operator(')') => {
                command_position = parens.close();
                index += 1;
            }
            ShellToken::Operator(';') | ShellToken::Operator('|') | ShellToken::Operator('&') => {
                parens.operator();
                command_position = true;
                index += 1;
            }
            ShellToken::Operator(_) => {
                parens.operator();
                index += 1;
            }
            ShellToken::Word(command) if command_position => {
                parens.word(true);
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
                // cas-cf4f: `rm` is a deletion, judged by bash_delete_targets.
                if !matches!(
                    command,
                    "touch" | "mkdir" | "tee" | "cp" | "mv" | "install"
                ) {
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
                if matches!(command, "cp" | "mv" | "install") {
                    if let Some(destination) = operands.pop() {
                        add_target(&destination);
                    }
                } else {
                    for operand in operands {
                        add_target(&operand);
                    }
                }
                // The operands were consumed here; a `$(` among them resumes
                // at command position, as before (conservative).
                parens.operator();
                index = cursor;
            }
            ShellToken::Word(_) => {
                parens.word(false);
                command_position = false;
                index += 1;
            }
        }
    }
    targets
}

/// Claude Code advertises this exact per-session ephemeral root to agents:
/// `/tmp/claude-<uid>/<project-slug>/<session-id>/scratchpad/...` (or the
/// macOS canonical spelling `/private/tmp/...`).
/// Bind the exemption to the hook's own session ID and reject traversal so it
/// cannot become a general `/tmp` escape hatch.
pub(crate) fn is_harness_session_scratchpad(path: &std::path::Path, session_id: &str) -> bool {
    use std::path::Component;

    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return false;
    }
    match components.next() {
        Some(Component::Normal(component)) if component == std::ffi::OsStr::new("tmp") => {}
        Some(Component::Normal(component)) if component == std::ffi::OsStr::new("private") => {
            if components.next() != Some(Component::Normal(std::ffi::OsStr::new("tmp"))) {
                return false;
            }
        }
        _ => return false,
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

/// cas-cf4f: the operands of `rm`, `rmdir` and `unlink` in command position,
/// each with whether its command was recursive (`-r`, `-R`, `--recursive`).
/// Mirrors the small command recognizer in [`bash_write_targets`].
fn bash_delete_targets(command: &str) -> Vec<(String, bool)> {
    let shell_command = shell_command_without_heredoc_bodies(command);
    let tokens = factory_shell_tokens(&shell_command);
    let variable_values = factory_shell_variable_values(&tokens);
    let mut targets = Vec::new();
    let mut index = 0;
    let mut command_position = true;
    // cas-a673: a `)` closing an argument substitution keeps argument position.
    let mut parens = ParenPositions::default();
    while index < tokens.len() {
        match &tokens[index] {
            ShellToken::Operator('(') => {
                parens.open(&tokens, index);
                command_position = true;
                index += 1;
            }
            ShellToken::Operator(')') => {
                command_position = parens.close();
                index += 1;
            }
            ShellToken::Operator(';') | ShellToken::Operator('|') | ShellToken::Operator('&') => {
                parens.operator();
                command_position = true;
                index += 1;
            }
            ShellToken::Operator(_) => {
                parens.operator();
                index += 1;
            }
            ShellToken::Word(word) if command_position => {
                parens.word(true);
                if matches!(word.as_str(), "do" | "then" | "else" | "elif")
                    || word.starts_with('-')
                    || word.contains('=')
                    || matches!(word.as_str(), "command" | "env" | "sudo")
                {
                    index += 1;
                    continue;
                }
                command_position = false;
                let name = std::path::Path::new(word)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(word);
                if !matches!(name, "rm" | "rmdir" | "unlink") {
                    index += 1;
                    continue;
                }
                let mut recursive = false;
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
                        ShellToken::Word(flag) if flag == "--recursive" => recursive = true,
                        ShellToken::Word(flag) if flag.starts_with("--") => {}
                        ShellToken::Word(flag) if flag.starts_with('-') => {
                            recursive |= flag.contains('r') || flag.contains('R');
                        }
                        ShellToken::Word(operand) => operands.push(operand.clone()),
                    }
                    cursor += 1;
                }
                for operand in operands {
                    for expanded in expand_factory_shell_word(&operand, &variable_values) {
                        targets.push((expanded, recursive));
                    }
                }
                parens.operator();
                index = cursor;
            }
            ShellToken::Word(_) => {
                parens.word(false);
                command_position = false;
                index += 1;
            }
        }
    }
    targets
}

/// cas-7d68: one link a shell command creates, with its operands as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellLink {
    /// Where the link is created.
    link: String,
    /// What it points at (symbolic) or the file it shares (hard).
    destination: String,
    symbolic: bool,
    /// `ln -r`: a symbolic destination is read relative to the cwd, not to
    /// the link's directory.
    destination_from_cwd: bool,
}

/// cas-7d68: every link `ln`, `link` (and `cp -s`/`cp -l`) creates in a shell
/// command, however wrapped (see [`shell_invocations`]). A link does not
/// exist when the hook runs, so the caller judges its destination and the
/// writes made through it, instead of resolving the link on disk.
fn bash_link_creations(command: &str, cwd: &Path) -> Vec<ShellLink> {
    let mut links = Vec::new();
    for (name, args) in shell_invocations(command) {
        if !matches!(name.as_str(), "ln" | "link" | "cp") {
            continue;
        }
        let mut symbolic = false;
        let mut hard = name != "cp";
        let mut relative = false;
        let mut no_target_directory = false;
        let mut target_directory: Option<String> = None;
        let mut operands = Vec::new();
        let mut index = 0;
        let mut options_done = false;
        while index < args.len() {
            let arg = &args[index];
            index += 1;
            if options_done || arg == "-" || !arg.starts_with('-') {
                operands.push(arg.clone());
                continue;
            }
            if arg == "--" {
                options_done = true;
                continue;
            }
            if let Some(long) = arg.strip_prefix("--") {
                let (key, value) = match long.split_once('=') {
                    Some((key, value)) => (key, Some(value.to_string())),
                    None => (long, None),
                };
                match key {
                    "symbolic" | "symbolic-link" => symbolic = true,
                    "link" => hard = true,
                    "relative" => relative = true,
                    "no-target-directory" => no_target_directory = true,
                    "target-directory" => {
                        target_directory = value.or_else(|| {
                            index += 1;
                            args.get(index - 1).cloned()
                        });
                    }
                    "suffix" if value.is_none() => index += 1,
                    _ => {}
                }
                continue;
            }
            for (position, flag) in arg.char_indices().skip(1) {
                match flag {
                    's' => symbolic = true,
                    'l' if name == "cp" => hard = true,
                    'r' if name == "ln" => relative = true,
                    'T' => no_target_directory = true,
                    't' | 'S' => {
                        let attached = &arg[position + 1..];
                        let value = if attached.is_empty() {
                            index += 1;
                            args.get(index - 1).cloned()
                        } else {
                            Some(attached.to_string())
                        };
                        if flag == 't' {
                            target_directory = value;
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
        if !symbolic && !hard {
            continue;
        }
        let basename = |destination: &str| {
            Path::new(destination)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(destination)
                .to_string()
        };
        let into = |directory: &str, destination: &str| {
            Path::new(directory).join(basename(destination)).display().to_string()
        };
        let pairs: Vec<(String, String)> = if let Some(directory) = target_directory {
            operands.iter().map(|destination| (into(&directory, destination), destination.clone())).collect()
        } else if operands.len() == 1 {
            vec![(basename(&operands[0]), operands[0].clone())]
        } else if let Some(last) = operands.pop() {
            if operands.len() > 1 || (!no_target_directory && cwd.join(&last).is_dir()) {
                operands.iter().map(|destination| (into(&last, destination), destination.clone())).collect()
            } else {
                operands.into_iter().map(|destination| (last.clone(), destination)).collect()
            }
        } else {
            Vec::new()
        };
        links.extend(pairs.into_iter().map(|(link, destination)| ShellLink {
            link,
            destination,
            symbolic,
            destination_from_cwd: relative || !symbolic,
        }));
    }
    links
}

/// cas-7d68: `path` (absolute, not yet normalized) as the filesystem will
/// resolve it, given the links `aliases` that the same command creates first
/// (link path, already-resolved destination). A `..` after a symlink leaves
/// the symlink's real destination, not its lexical parent, so an existing
/// symlink is resolved before `..` pops it. `None` when such a symlink cannot
/// be resolved (dangling): the write is not attributable to any root.
fn resolve_through_links(path: &Path, aliases: &[(std::path::PathBuf, std::path::PathBuf)]) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if std::fs::symlink_metadata(&current)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    current = current.canonicalize().ok()?;
                }
                current.pop();
            }
            Component::Normal(name) => {
                current.push(name);
                if let Some((_, destination)) = aliases.iter().rev().find(|(link, _)| *link == current) {
                    current = destination.clone();
                }
            }
        }
    }
    Some(current)
}

/// cas-cf4f / cas-aa4e: a Cassy runtime file under `~/.cas` (a socket, lock
/// or pid file, or a session record under `~/.cas/sessions`) left behind by
/// a throwaway factory. Files only. Any agent may delete one once it is
/// stale; a live one is refused with the reason.
/// cas-aa4e: what a delete target under `~/.cas` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeLeftover {
    /// Not a Cassy runtime file (other path, a directory, another name).
    NotLeftover,
    /// A runtime file nothing is using: deletable.
    Stale,
    /// A runtime file still in use, with the reason it is live.
    Live(&'static str),
}

/// cas-aa4e: classify a `~/.cas` socket, lock, pid file or session record by
/// whether something still uses it. Unknown state counts as live: a delete
/// can always wait, a deleted live socket or lock cannot be undone.
fn cas_runtime_leftover(path: &std::path::Path, home: Option<&std::path::Path>) -> RuntimeLeftover {
    let Some(cas_home) = home.and_then(|home| canonicalize_for_containment(&home.join(".cas")))
    else {
        return RuntimeLeftover::NotLeftover;
    };
    if !path.starts_with(&cas_home) || path == cas_home || path.is_dir() {
        return RuntimeLeftover::NotLeftover;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let session_record = path.parent() == Some(cas_home.join("sessions").as_path());
    if name.ends_with(".sock") {
        socket_staleness(path)
    } else if name.ends_with(".lock") {
        lock_staleness(path)
    } else if name.ends_with(".pid") || session_record {
        match recorded_pid(path) {
            Some(pid) if crate::mcp::daemon::pid_alive(pid) => {
                RuntimeLeftover::Live("its recorded pid is alive")
            }
            _ => RuntimeLeftover::Stale,
        }
    } else {
        RuntimeLeftover::NotLeftover
    }
}

/// A socket is stale only when nothing accepts on it.
#[cfg(unix)]
fn socket_staleness(path: &std::path::Path) -> RuntimeLeftover {
    use std::os::unix::fs::FileTypeExt;
    // A regular file named *.sock (a 0-byte leftover) has no listener.
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_socket() => return RuntimeLeftover::Stale,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RuntimeLeftover::Stale,
        _ => {}
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => RuntimeLeftover::Live("the socket has a listener"),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            RuntimeLeftover::Stale
        }
        Err(_) => RuntimeLeftover::Live("the socket state could not be determined"),
    }
}

#[cfg(not(unix))]
fn socket_staleness(_path: &std::path::Path) -> RuntimeLeftover {
    RuntimeLeftover::Live("the socket state could not be determined")
}

/// A lock is stale when no process holds it and no pid it records is alive.
fn lock_staleness(path: &std::path::Path) -> RuntimeLeftover {
    use fs2::FileExt;
    let Ok(file) = std::fs::File::open(path) else {
        return if path.exists() {
            RuntimeLeftover::Live("the lock could not be opened")
        } else {
            RuntimeLeftover::Stale
        };
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
        }
        Err(_) => return RuntimeLeftover::Live("the lock is held"),
    }
    match recorded_pid(path) {
        Some(pid) if crate::mcp::daemon::pid_alive(pid) => {
            RuntimeLeftover::Live("the lock's recorded pid is alive")
        }
        _ => RuntimeLeftover::Stale,
    }
}

/// The pid a runtime file records: the whole file as a number, a `pid=` or
/// `pid:` line, or a JSON `pid`/`daemon_pid` field. Only plausible user pids
/// (2..=i32::MAX) count; anything else is no recorded pid.
fn recorded_pid(path: &std::path::Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    let plausible = |pid: u64| (2..=i32::MAX as u64).contains(&pid).then_some(pid as u32);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(pid) = ["daemon_pid", "pid"]
            .iter()
            .find_map(|key| value.get(key).and_then(serde_json::Value::as_u64))
        {
            return plausible(pid);
        }
        if let Some(pid) = value.as_u64() {
            return plausible(pid);
        }
    }
    text.lines().find_map(|line| {
        let line = line.trim();
        let value = line
            .strip_prefix("pid=")
            .or_else(|| line.strip_prefix("pid:"))
            .unwrap_or(line)
            .trim();
        value.parse::<u64>().ok().and_then(plausible)
    })
}

/// cas-cf4f: judge one deletion target. Never `/`, `$HOME` or an ancestor of
/// it, `/tmp` itself, or the worktree root or any ancestor of it (which
/// includes the main checkout). Otherwise allowed inside the sanctioned write
/// roots, for stale Cassy runtime files under `~/.cas`, and, for the
/// supervisor, below `/tmp` (cache cleanup, cas-a3af).
fn factory_delete_violation(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
    registered_worktree_root: Option<&std::path::Path>,
    raw_path: &str,
) -> Option<FactoryWriteViolation> {
    let violation = |resolved_path: std::path::PathBuf, matched_rule: &'static str| FactoryWriteViolation {
        evaluated_path: raw_path.to_string(),
        resolved_path,
        matched_rule,
    };
    if raw_path.contains('$') {
        return Some(violation(
            std::path::PathBuf::from(raw_path),
            "deletion with an unresolved shell variable",
        ));
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let expanded = if raw_path == "~" {
        home.clone()?
    } else if let Some(suffix) = raw_path.strip_prefix("~/") {
        home.as_ref()?.join(suffix)
    } else {
        std::path::PathBuf::from(raw_path)
    };
    let path = if expanded.is_absolute() {
        lexically_normalize_path(expanded)
    } else {
        lexically_normalize_path(std::path::PathBuf::from(&input.cwd).join(expanded))
    };
    let Some(resolved) = canonicalize_for_containment(&path) else {
        return Some(violation(path, "deletion outside sanctioned roots (unresolvable path)"));
    };
    let canonical = |path: &std::path::Path| canonicalize_for_containment(path);
    let worktree_root = registered_worktree_root
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::var_os("CAS_CLONE_PATH").filter(|v| !v.is_empty()).map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from(&input.cwd));
    let own_worktree_root = worktree_root.clone();
    let is_root_or_ancestor_of = |protected: Option<std::path::PathBuf>| {
        protected
            .and_then(|protected| canonical(&protected))
            .is_some_and(|protected| protected.starts_with(&resolved))
    };
    if resolved.parent().is_none()
        || is_root_or_ancestor_of(home.clone())
        || is_root_or_ancestor_of(Some(worktree_root))
        || ["/tmp", "/private/tmp"]
            .iter()
            .any(|tmp| canonical(std::path::Path::new(tmp)).is_some_and(|tmp| tmp == resolved))
    {
        return Some(violation(resolved, "deletion of a protected root ($HOME, /, /tmp, the repository or the worktree)"));
    }
    if unsanctioned_factory_path_with_worktree(
        input,
        configured_artifacts_root,
        configured_scratch_root,
        is_supervisor,
        raw_path,
        registered_worktree_root,
    )
    .is_none()
    {
        return None;
    }
    // cas-39f3: a worker's own gitdir index.lock, when a killed git left it
    // empty and nothing holds it open.
    match own_gitdir_index_lock(&resolved, &own_worktree_root) {
        OwnGitdirLock::Stale => return None,
        OwnGitdirLock::Live(rule) => return Some(violation(resolved, rule)),
        OwnGitdirLock::NotOwnLock => {}
    }
    // cas-aa4e: a ~/.cas runtime file is deletable only when it is stale.
    match cas_runtime_leftover(&resolved, home.as_deref()) {
        RuntimeLeftover::Stale => return None,
        RuntimeLeftover::Live(reason) => {
            return Some(violation(resolved, live_runtime_rule(reason)));
        }
        RuntimeLeftover::NotLeftover => {}
    }
    if is_supervisor
        && ["/tmp", "/private/tmp"].iter().any(|tmp| {
            canonical(std::path::Path::new(tmp)).is_some_and(|tmp| resolved.starts_with(tmp))
        })
    {
        return None;
    }
    Some(violation(resolved, "deletion outside sanctioned roots"))
}

/// cas-39f3: classification of a deletion target against the caller's own
/// linked-worktree gitdir (`<repo>/.git/worktrees/<name>/`).
enum OwnGitdirLock {
    /// Not `index.lock` in the caller's own gitdir: other rules decide.
    NotOwnLock,
    /// Zero bytes and not open in any process: a killed git's leftover.
    Stale,
    /// The caller's own lock, but possibly in use; the refusal rule.
    Live(&'static str),
}

fn own_gitdir_index_lock(resolved: &Path, worktree_root: &Path) -> OwnGitdirLock {
    if resolved.file_name().is_none_or(|name| name != "index.lock") {
        return OwnGitdirLock::NotOwnLock;
    }
    // A linked worktree's `.git` is a file naming its private gitdir.
    let Ok(dot_git) = std::fs::read_to_string(worktree_root.join(".git")) else {
        return OwnGitdirLock::NotOwnLock;
    };
    let Some(gitdir) = dot_git
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(|gitdir| std::path::PathBuf::from(gitdir.trim()))
    else {
        return OwnGitdirLock::NotOwnLock;
    };
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        worktree_root.join(gitdir)
    };
    let Ok(gitdir) = gitdir.canonicalize() else {
        return OwnGitdirLock::NotOwnLock;
    };
    if gitdir
        .parent()
        .and_then(Path::file_name)
        .is_none_or(|name| name != "worktrees")
        || resolved != gitdir.join("index.lock")
    {
        return OwnGitdirLock::NotOwnLock;
    }
    let Ok(metadata) = std::fs::symlink_metadata(resolved) else {
        return OwnGitdirLock::NotOwnLock;
    };
    if !metadata.file_type().is_file() {
        return OwnGitdirLock::Live("deletion of a git index.lock: not a regular file");
    }
    if metadata.len() != 0 {
        return OwnGitdirLock::Live("deletion of a git index.lock: the lock has content");
    }
    if git_lock_open_by_a_process(resolved, &metadata) {
        return OwnGitdirLock::Live("deletion of a git index.lock: the lock is held open by a process");
    }
    OwnGitdirLock::Stale
}

/// Whether any process holds `lock` open. git keeps its lockfile open from
/// creation until commit or rollback, so an open descriptor means a live
/// writer. Linux scans `/proc/*/fd`; elsewhere a lock modified in the last
/// minute is treated as held.
fn git_lock_open_by_a_process(lock: &Path, metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "linux")]
    {
        let _ = metadata;
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return true;
        };
        for process in processes.flatten() {
            let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if std::fs::read_link(fd.path()).is_ok_and(|target| target == lock) {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = lock;
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_none_or(|age| age < std::time::Duration::from_secs(60))
    }
}

/// cas-aa4e: the static refusal rule for a live runtime file's reason.
fn live_runtime_rule(reason: &'static str) -> &'static str {
    match reason {
        "the socket has a listener" => "deletion of a live Cassy runtime file: the socket has a listener",
        "the socket state could not be determined" => {
            "deletion of a live Cassy runtime file: the socket state could not be determined"
        }
        "the lock could not be opened" => "deletion of a live Cassy runtime file: the lock could not be opened",
        "the lock is held" => "deletion of a live Cassy runtime file: the lock is held",
        "the lock's recorded pid is alive" => {
            "deletion of a live Cassy runtime file: the lock's recorded pid is alive"
        }
        _ => "deletion of a live Cassy runtime file: its recorded pid is alive",
    }
}

/// cas-3147: one out-of-tree change admitted by an operator write root or
/// task grant. Every one is audited.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WriteRootUse {
    path: std::path::PathBuf,
    mode: super::write_roots::WriteMode,
    root: std::path::PathBuf,
    task_id: Option<String>,
}

/// cas-3147: the workspace contract's verdict for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FactoryWriteDecision {
    /// Every target is inside the default sanctioned set.
    Allowed,
    /// Some targets are outside it but admitted by operator write roots.
    AllowedByRoot(Vec<WriteRootUse>),
    Denied(FactoryWriteViolation),
}

/// cas-3147: [`factory_write_violation`] with the operator's write roots and
/// the current agent's task grants. An empty policy decides exactly as the
/// default contract does.
fn factory_write_decision(
    input: &HookInput,
    configured_artifacts_root: &Option<String>,
    configured_scratch_root: Option<&str>,
    is_supervisor: bool,
    registered_worktree_root: Option<&std::path::Path>,
    policy: &super::write_roots::WritePolicy,
    task_ids: &std::collections::HashSet<String>,
) -> FactoryWriteDecision {
    use super::write_roots::WriteMode;
    let Some(tool) = input.tool_name.as_deref() else {
        return FactoryWriteDecision::Allowed;
    };
    // cas-3147: the operator policy directory is never writable by an agent,
    // whatever sanctions or roots would otherwise cover it.
    // A delete also may not remove an ancestor of it (`rm -rf .cas`).
    let protected_violation = |raw_path: &str, ancestors_too: bool| -> Option<FactoryWriteViolation> {
        if policy.protected.is_empty() {
            return None;
        }
        let expanded = match raw_path.strip_prefix("~/") {
            Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest),
            None => std::path::PathBuf::from(raw_path),
        };
        let path = if expanded.is_absolute() {
            lexically_normalize_path(expanded)
        } else {
            lexically_normalize_path(std::path::PathBuf::from(&input.cwd).join(expanded))
        };
        // cas-7d68: any project's policy path, judged lexically as well as
        // at its resolved location.
        let lexical_policy = super::write_roots::is_cas_policy_path(&path);
        let resolved = canonicalize_for_containment(&path).unwrap_or(path);
        (lexical_policy
            || super::write_roots::is_cas_policy_path(&resolved)
            || policy
                .protected
                .iter()
                .filter_map(|dir| canonicalize_for_containment(dir))
                .any(|dir| resolved.starts_with(&dir) || (ancestors_too && dir.starts_with(&resolved))))
            .then(|| FactoryWriteViolation {
                evaluated_path: raw_path.to_string(),
                resolved_path: resolved,
                matched_rule: "the operator write policy and project config are operator-only",
            })
    };
    let Some(tool_input) = input.tool_input.as_ref() else {
        return FactoryWriteDecision::Allowed;
    };
    let codex_missing_workdir = tool == "Bash"
        && cas_core::env_overlay::var("CAS_HOOK_HARNESS")
            .is_ok_and(|harness| harness.eq_ignore_ascii_case("codex"))
        && !tool_input.get("workdir").and_then(|value| value.as_str()).is_some_and(|path| !path.is_empty());
    // cas-7d68: a link the command creates does not exist when this hook
    // runs, so resolving paths on disk would judge a write through it at the
    // link's own location. Resolve every link the command creates to its real
    // destination first. A link may never point at, into or above a Cassy
    // policy path; a hard link is a write to the file it shares; the link
    // path is itself a write; and every other path the command writes is
    // judged where it lands through those links.
    let cwd = std::path::PathBuf::from(&input.cwd);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let absolute = |raw: &str, base: &std::path::Path| -> Option<std::path::PathBuf> {
        let expanded = if raw == "~" {
            home.clone()?
        } else if let Some(rest) = raw.strip_prefix("~/") {
            home.as_ref()?.join(rest)
        } else {
            std::path::PathBuf::from(raw)
        };
        Some(if expanded.is_absolute() { expanded } else { base.join(expanded) })
    };
    let unresolved_word = |raw: &str| raw.contains('$') || raw.contains('`');
    let link_violation = |evaluated: String, resolved: std::path::PathBuf, matched_rule: &'static str| {
        FactoryWriteDecision::Denied(FactoryWriteViolation {
            evaluated_path: evaluated,
            resolved_path: resolved,
            matched_rule,
        })
    };
    let protected_dirs: Vec<std::path::PathBuf> = policy
        .protected
        .iter()
        .filter_map(|dir| canonicalize_for_containment(dir))
        .collect();
    let shell_effects = if tool == "Bash" {
        match shell_effects::collect(tool_input.get("command").and_then(|value| value.as_str()).unwrap_or(""), &cwd) {
            Ok(effects) => effects,
            Err(rule) => return link_violation(input.cwd.clone(), cwd.clone(), rule),
        }
    } else {
        shell_effects::ShellEffects::default()
    };
    let mut aliases: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut link_targets: Vec<(String, Option<WriteMode>)> = Vec::new();
    if tool == "Bash" {
        for (link, link_cwd) in &shell_effects.links {
            let evaluated = format!("{} -> {}", link.link, link.destination);
            if unresolved_word(&link.link) || unresolved_word(&link.destination) {
                return link_violation(evaluated, std::path::PathBuf::from(&link.link), "link with an unresolved shell variable");
            }
            let Some(link_path) = absolute(&link.link, link_cwd).and_then(|path| resolve_through_links(&path, &aliases)) else {
                return link_violation(evaluated, std::path::PathBuf::from(&link.link), "link through a symlink that cannot be resolved");
            };
            let base = if link.destination_from_cwd {
                link_cwd.clone()
            } else {
                link_path.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| link_cwd.clone())
            };
            let Some(destination) = absolute(&link.destination, &base).and_then(|path| resolve_through_links(&path, &aliases)) else {
                return link_violation(evaluated, link_path, "link through a symlink that cannot be resolved");
            };
            let destination = canonicalize_for_containment(&destination).unwrap_or(destination);
            let reaches_policy = super::write_roots::is_cas_policy_path(&destination)
                || destination.file_name().is_some_and(|name| name == ".cas")
                || protected_dirs
                    .iter()
                    .any(|dir| dir.starts_with(&destination) || destination.starts_with(dir));
            if reaches_policy {
                return link_violation(evaluated, destination, "link to, into or above a Cassy policy path");
            }
            link_targets.push((link_path.display().to_string(), None));
            if !link.symbolic {
                link_targets.push((destination.display().to_string(), Some(WriteMode::Edit)));
            }
            aliases.push((link_path, destination));
        }
    }
    // The path a raw target names once the command's links exist and any
    // existing symlink before a `..` is followed. Unchanged when neither
    // applies; a target with a shell variable is judged as before.
    let physical = |raw: &str| -> Result<String, FactoryWriteViolation> {
        if unresolved_word(raw) {
            return Ok(raw.to_string());
        }
        let Some(path) = absolute(raw, &cwd) else {
            return Ok(raw.to_string());
        };
        match resolve_through_links(&path, &aliases) {
            Some(resolved) if resolved == lexically_normalize_path(path.clone()) => Ok(raw.to_string()),
            Some(resolved) => Ok(resolved.display().to_string()),
            None => Err(FactoryWriteViolation {
                evaluated_path: raw.to_string(),
                resolved_path: path,
                matched_rule: "a path through a symlink that cannot be resolved",
            }),
        }
    };
    let mut uses = Vec::new();
    let admit = |resolved: std::path::PathBuf, mode: WriteMode, uses: &mut Vec<WriteRootUse>| {
        policy.matching(&resolved, mode, task_ids).map(|root| {
            uses.push(WriteRootUse {
                path: resolved.clone(),
                mode,
                root: root.path.clone(),
                task_id: root.task_id.clone(),
            });
        })
    };
    // cas-cf4f: Bash deletions are judged as deletions, before creation.
    if tool == "Bash" {
        for (raw_path, _recursive) in &shell_effects.deletes {
            let raw_path = match physical(&raw_path) {
                Ok(path) => path,
                Err(violation) => return FactoryWriteDecision::Denied(violation),
            };
            if let Some(violation) = protected_violation(&raw_path, true) {
                return FactoryWriteDecision::Denied(violation);
            }
            let Some(violation) = factory_delete_violation(
                input,
                configured_artifacts_root,
                configured_scratch_root,
                is_supervisor,
                registered_worktree_root,
                &raw_path,
            ) else {
                continue;
            };
            // Only an ordinary out-of-root delete can be admitted; protected
            // roots and live runtime files stay refused whatever the policy.
            let admitted = violation.matched_rule == "deletion outside sanctioned roots"
                && admit(violation.resolved_path.clone(), WriteMode::Delete, &mut uses).is_some();
            if !admitted {
                return FactoryWriteDecision::Denied(violation);
            }
        }
    }
    // GH #1182: NotebookEdit names its file `notebook_path`; the protected
    // policy files are refused for it too. (The default contract judges only
    // `file_path`/`path`, unchanged here.)
    if tool == "NotebookEdit"
        && let Some(path) = tool_input.get("notebook_path").and_then(|value| value.as_str())
    {
        let path = match physical(path) {
            Ok(path) => path,
            Err(violation) => return FactoryWriteDecision::Denied(violation),
        };
        if let Some(violation) = protected_violation(&path, false) {
            return FactoryWriteDecision::Denied(violation);
        }
    }
    let targets: Vec<(String, Option<WriteMode>, Option<String>)> = match tool {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => tool_input
            .get("file_path")
            .or_else(|| tool_input.get("path"))
            .and_then(|value| value.as_str())
            .map(|path| vec![(path.to_string(), None, None)])
            .unwrap_or_default(),
        "Bash" => {
            // cas-3147 (GH #1169 item 4): once the operator has configured
            // roots, in-place edits and rename sources are judged like any
            // other write. Without them the default contract stays as it
            // was, except that the protected policy files are never edited
            // in place or renamed (GH #1182).
            let mut in_place = Vec::new();
            for raw_path in &shell_effects.edits {
                match physical(raw_path) {
                    Ok(path) => in_place.push(path),
                    Err(violation) => return FactoryWriteDecision::Denied(violation),
                }
            }
            for raw_path in &in_place {
                if let Some(violation) = protected_violation(raw_path, false) {
                    return FactoryWriteDecision::Denied(violation);
                }
            }
            if !policy.configured {
                in_place.clear();
            }
            shell_effects.writes
                .into_iter()
                .map(|(path, redirection)| (path, None, if codex_missing_workdir { redirection } else { None }))
                .chain(in_place.into_iter().map(|path| (path, Some(WriteMode::Edit), None)))
                .collect()
        }
        // cas-49c0: Codex file edits arrive as `apply_patch`, with the patch
        // text in `tool_input.command`.
        "apply_patch" => {
            let Some(patch) = tool_input.get("command").and_then(|value| value.as_str()) else {
                return FactoryWriteDecision::Allowed;
            };
            apply_patch_write_targets_with_modes(patch)
                .into_iter()
                .map(|(path, mode)| (path, Some(mode), None))
                .collect()
        }
        _ => return FactoryWriteDecision::Allowed,
    };
    let mut resolved_targets = Vec::with_capacity(targets.len() + link_targets.len());
    for (raw_path, mode, relative_redirection) in targets {
        match physical(&raw_path) {
            Ok(path) => resolved_targets.push((path, mode, relative_redirection)),
            Err(violation) => return FactoryWriteDecision::Denied(violation),
        }
    }
    // Link paths are already resolved; never alias a link to itself.
    resolved_targets.extend(link_targets.into_iter().map(|(path, mode)| (path, mode, None)));
    for (raw_path, mode, relative_redirection) in resolved_targets {
        if let Some(violation) = protected_violation(&raw_path, false) {
            return FactoryWriteDecision::Denied(violation);
        }
        // cas-6ada: a dynamic suffix does not make a known creation root
        // unknown. Check both the static prefix and its containing directory:
        // `/scratch/run-$$` is rooted, but `/scratch$ROOT_SUFFIX` is not.
        // Keep root variables, literal parent traversal and symlink escapes
        // fail-closed. Deletions were checked separately above.
        if raw_path.contains('$') {
            let prefix = raw_path.split('$').next().unwrap_or_default();
            let parent = prefix.rsplit_once('/').map(|(parent, _)| {
                if parent.is_empty() { "/" } else { parent }
            }).unwrap_or(".");
            let traverses_parent = std::path::Path::new(&raw_path).components()
                .any(|component| component == std::path::Component::ParentDir);
            if !prefix.is_empty() && !traverses_parent && [prefix, parent].iter().all(|path| {
                unsanctioned_factory_path_with_worktree(input, configured_artifacts_root,
                    configured_scratch_root, is_supervisor, path, registered_worktree_root).is_none()
            }) {
                continue;
            }
            return FactoryWriteDecision::Denied(FactoryWriteViolation {
                evaluated_path: raw_path.clone(),
                resolved_path: std::path::PathBuf::from(&raw_path),
                matched_rule: "unresolved shell variable",
            });
        }
        let Some(resolved) = unsanctioned_factory_path_with_worktree(
            input,
            configured_artifacts_root,
            configured_scratch_root,
            is_supervisor,
            &raw_path,
            registered_worktree_root,
        ) else {
            continue;
        };
        let mode = mode.unwrap_or(if std::fs::symlink_metadata(&resolved).is_ok() {
            WriteMode::Edit
        } else {
            WriteMode::Create
        });
        if admit(resolved.clone(), mode, &mut uses).is_none() {
            // Preserve ordinary admission, including inside relative logs and
            // operator roots. Only an otherwise-refused relative redirection
            // has an uncertain destination when Codex omitted its workdir.
            let (evaluated_path, matched_rule) = match relative_redirection {
                Some(path) => (path, "Codex hook input omits workdir"),
                None => (raw_path, "none"),
            };
            return FactoryWriteDecision::Denied(FactoryWriteViolation {
                evaluated_path,
                resolved_path: resolved,
                matched_rule,
            });
        }
    }
    if uses.is_empty() {
        FactoryWriteDecision::Allowed
    } else {
        FactoryWriteDecision::AllowedByRoot(uses)
    }
}

/// cas-0d4f0: refusal for an agent tool call that writes, edits, moves over
/// or deletes a Cassy config file (`.cas/config.toml` or `config.yaml`, or
/// the global `~/.config/cas/` one) directly. Agents change ordinary keys with
/// `cas config set`; security-relevant keys are the operator's.
fn cas_config_file_write_denial(input: &HookInput) -> Option<String> {
    let tool = input.tool_name.as_deref()?;
    let tool_input = input.tool_input.as_ref()?;
    let raw_targets: Vec<String> = match tool {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => tool_input
            .get("file_path")
            .or_else(|| tool_input.get("path"))
            .and_then(|value| value.as_str())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
        "Bash" => {
            let command = tool_input.get("command").and_then(|value| value.as_str())?;
            bash_write_targets(command)
                .into_iter()
                .chain(bash_in_place_edit_targets(command))
                .chain(bash_delete_targets(command).into_iter().map(|(path, _)| path))
                // cas-7d68: `ln -sf forged.toml .cas/config.toml` replaces it.
                .chain(
                    bash_link_creations(command, Path::new(&input.cwd))
                        .into_iter()
                        .map(|link| link.link),
                )
                .collect()
        }
        "apply_patch" => tool_input
            .get("command")
            .or_else(|| tool_input.get("patch"))
            .or_else(|| tool_input.get("input"))
            .and_then(|value| value.as_str())
            .map(|patch| {
                apply_patch_write_targets_with_modes(patch)
                    .into_iter()
                    .map(|(path, _)| path)
                    .collect()
            })
            .unwrap_or_default(),
        _ => return None,
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let global = crate::config::global_cas_dir();
    let target = raw_targets.into_iter().find(|raw| {
        let expanded = match (raw.strip_prefix("~/"), home.as_ref()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => std::path::PathBuf::from(raw),
        };
        let path = lexically_normalize_path(if expanded.is_absolute() {
            expanded
        } else {
            std::path::PathBuf::from(&input.cwd).join(expanded)
        });
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let is_config = matches!(name, "config.toml" | "config.yaml" | "config.yml");
        let parent = path.parent();
        let in_cas = parent.and_then(|dir| dir.file_name()).is_some_and(|dir| dir == ".cas");
        (is_config || (name == "proxy.toml" && in_cas))
            && (in_cas || parent.is_some_and(|dir| global.as_deref() == Some(dir)))
    })?;
    if target.ends_with("proxy.toml") {
        return Some(format!(
            "🚫 CASSY PROXY CONFIG (cas-0d4f0): agents do not write {target} directly. Manage MCP servers with `system action=proxy_add|proxy_remove` (or `cas mcp add|remove`); servers on factory.supervisor_only_mcp are operator-only."
        ));
    }
    Some(format!(
        "🚫 CASSY CONFIG (cas-0d4f0): agents do not write {target} directly. Change an ordinary key with `cas config set <key> <value>`; keys that relax a Cassy guard (slack.transport, verification.force_bypass_allowed, factory.supervisor_only_mcp, the qa gates, …) are operator-only, so ask the supervisor to request the operator's approval."
    ))
}

/// The arguments of every `cas` invocation in a shell command, in order.
/// Shared by the operator-only command checks (cas-3147, cas-0d4f0,
/// cas-1b94, cas-3c26, GH #1182). Every statement is unwrapped through
/// command wrappers (`timeout`, `nice`, `nohup`, `env`, `command`, `exec`,
/// `stdbuf`, `setsid`, `xargs`, `time`, `chrt`, `ionice`, `sudo`, `flock`,
/// leading `VAR=val`; see [`unwrapped_command_index`]), `$(which cas)` style
/// substitutions count as `cas`, and `sh -c` / `eval` payloads are inspected
/// recursively.
fn cas_invocations(command: &str) -> Vec<Vec<String>> {
    shell_invocations(&normalize_cas_substitutions(command))
        .into_iter()
        .filter(|(name, _)| name == "cas")
        .map(|(_, args)| args)
        .collect()
}

/// cas-d757: every command a shell command line runs, as `(basename, args)`,
/// through the same unwrapping as [`cas_invocations`]: wrappers and leading
/// `VAR=val`, and `sh -c` / `eval` payloads recursively. Shared with the
/// worker-denied CLI guard (GH #1219).
pub(super) fn shell_invocations(command: &str) -> Vec<(String, Vec<String>)> {
    let mut found = Vec::new();
    collect_shell_invocations(command, 0, &mut found);
    found
}

fn collect_shell_invocations(command: &str, depth: usize, found: &mut Vec<(String, Vec<String>)>) {
    if depth > 3 {
        return;
    }
    for words in shell_statement_words(command) {
        if let Some(payload) = env_split_string_payload(&words) {
            collect_shell_invocations(&payload, depth + 1, found);
        }
        let Some(index) = unwrapped_command_index(&words) else {
            continue;
        };
        let args = &words[index + 1..];
        let name = shell_word_basename(&words[index]);
        match name {
            // Wrappers whose option value is itself a shell command.
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "script" | "su" | "runuser" => {
                for (position, arg) in args.iter().enumerate() {
                    let flags_c =
                        arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c');
                    if (flags_c || arg == "--command")
                        && let Some(payload) = args.get(position + 1)
                    {
                        collect_shell_invocations(payload, depth + 1, found);
                    }
                }
            }
            "eval" => collect_shell_invocations(&args.join(" "), depth + 1, found),
            _ => {}
        }
        found.push((name.to_string(), args.to_vec()));
    }
}

/// The refusal for an agent shell command that would change write roots or
/// grants (cas-3147, GH #1182).
const OPERATOR_POLICY_COMMAND_DENIAL: &str = "🚫 OPERATOR-ONLY (cas-3147, GH #1182): write roots and per-task write grants can only be changed by the operator, never by an agent. Ask the supervisor to request the operator's approval; the operator runs `cas config set factory.write_roots …` (from their terminal or as `! cas …` in a pane) or `cas config grant-write …` themselves.";

/// cas-3147, cas-0d4f0, cas-3c26, GH #1182: refusal for an agent shell
/// command that runs an operator-only `cas` action or changes an
/// operator-only setting. Setting write roots no longer refuses agent
/// environment in the CLI, so this hook is the gate for them. Two layers:
///
/// 1. Structural: every `cas` invocation [`cas_invocations`] finds, however
///    wrapped, is judged by the CLI parser for operator-only actions
///    (cas-3c26), for setting or resetting a guard-relaxing key (cas-0d4f0),
///    and for setting write roots or changing grants (cas-3147).
/// 2. Textual catch-all, for write roots and grants: a command whose text
///    names `factory.write_roots` together with a standalone `set` word, or
///    `grant-write` / `revoke-write`, is refused unless every statement is a
///    read-only command (grep, cat, git log, `cas config get`, …). This
///    covers interpreters (`python -c`, `perl -e`, heredocs) the structural
///    pass cannot read.
///
/// Guardrail-grade: a command hidden in a script file the agent runs, or
/// assembled at run time, is out of reach of a tool-call hook.
fn operator_policy_command_denial(command: &str) -> Option<String> {
    for args in cas_invocations(command) {
        // cas-3c26: operator-only actions, judged by the CLI parser.
        if let Some(action) = crate::config::operator_policy::operator_only_cas_invocation(&args) {
            return Some(format!(
                "🚫 OPERATOR-ONLY (cas-3c26): {action} is the operator's to do, from their own terminal, never an agent's. Ask the supervisor to request it from the operator."
            ));
        }
        let config = args.iter().position(|arg| arg == "config");
        let refused = config.is_some_and(|at| {
            let rest = &args[at + 1..];
            rest.first()
                .is_some_and(|sub| sub == "grant-write" || sub == "revoke-write")
                || (rest.first().is_some_and(|sub| sub == "set")
                    && rest.get(1).is_some_and(|key| key == "factory.write_roots"))
        });
        // cas-0d4f0: security-relevant keys, set or reset.
        let guarded_key = config.and_then(|at| {
            let rest = &args[at + 1..];
            rest.first()
                .filter(|sub| *sub == "set" || *sub == "reset")
                .and_then(|_| rest.get(1))
                .filter(|key| {
                    key.as_str() != "factory.write_roots"
                        && crate::config::operator_policy::is_operator_only_config_key(key)
                })
        });
        if let Some(key) = guarded_key {
            return Some(format!(
                "🚫 OPERATOR-ONLY (cas-0d4f0): {key} relaxes a Cassy guard, so only the operator changes it, from their own terminal, never an agent. Ask the supervisor to request the operator's approval; the operator runs `cas config set {key} …` themselves. Ordinary keys stay settable with `cas config set`."
            ));
        }
        if refused {
            return Some(OPERATOR_POLICY_COMMAND_DENIAL.to_string());
        }
    }
    operator_policy_command_textual(command).then(|| OPERATOR_POLICY_COMMAND_DENIAL.to_string())
}

/// Replace command substitutions that resolve the `cas` binary
/// (`$(which cas)`, `$(command -v cas)`, `$(type -P cas)`, backticks) with
/// `cas`, so the structural pass sees `cas` as the executable.
fn normalize_cas_substitutions(command: &str) -> String {
    static SUBSTITUTION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    SUBSTITUTION
        .get_or_init(|| {
            regex::Regex::new(
                r#"\$\(\s*(?:which|command\s+-[vV]|type\s+-[pP])\s+cas\s*\)|`\s*(?:which|command\s+-[vV]|type\s+-[pP])\s+cas\s*`"#,
            )
            .expect("valid cas substitution pattern")
        })
        .replace_all(command, "cas")
        .into_owned()
}

/// Options taking a separate value, for each wrapper the structural pass
/// unwraps. Duration words belong only to timeout; positional values are
/// consumed only by wrappers that define one.
fn wrapper_value_options(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "timeout" => &["-s", "--signal", "-k", "--kill-after"],
        "nice" => &["-n", "--adjustment"],
        // cas-b7b2: GNU `--eof`, `--max-lines` and `--replace` take an
        // optional value that can only be attached (`--replace=X`); as bare
        // words they are flags, and the next word is the command.
        "xargs" => &[
            "-I", "-n", "-P", "-L", "-s", "-d", "-E", "-a", "--max-args", "--max-procs",
            "--delimiter", "--arg-file", "--max-chars", "--process-slot-var",
        ],
        "stdbuf" => &["-i", "-o", "-e", "--input", "--output", "--error"],
        "ionice" => &["-c", "-n", "-p", "-P", "-u", "--class", "--classdata"],
        "exec" => &["-a"],
        "time" => &["-f", "--format", "-o", "--output"],
        "flock" => &["-w", "--timeout", "-E", "--conflict-exit-code"],
        "doas" => &["-u", "-C"],
        _ => &[],
    }
}

const COMMAND_WRAPPERS: &[&str] = &[
    "timeout", "nice", "nohup", "setsid", "time", "exec", "xargs", "stdbuf", "ionice", "chrt",
    "flock", "doas", "builtin", "unbuffer",
];

/// The index of the command a wrapper chain runs, or `None` when the
/// statement has no command word.
pub(super) fn unwrapped_command_index(words: &[String]) -> Option<usize> {
    let mut index = executable_word_index(words)?;
    for _ in 0..16 {
        let name = shell_word_basename(&words[index]);
        if !COMMAND_WRAPPERS.contains(&name) {
            return Some(index);
        }
        let value_options = wrapper_value_options(name);
        let mut next = index + 1;
        // File/FD, duration and scheduling priority, respectively.
        let mut positional = matches!(name, "flock" | "timeout" | "chrt");
        while next < words.len() {
            let word = words[next].as_str();
            if word == "--" {
                next += 1;
                break;
            }
            if value_options.contains(&word) {
                next += 2;
            } else if name == "xargs" && xargs_short_option_needs_value(word, value_options) {
                next += 2;
            } else if word.starts_with('-') || (name == "timeout" && is_duration_word(word)) {
                if name == "timeout" && is_duration_word(word) && !word.starts_with('-') {
                    positional = false;
                }
                next += 1;
            } else if positional {
                positional = false;
                next += 1;
            } else {
                break;
            }
        }
        let rest = words.get(next..)?;
        index = next + executable_word_index(rest)?;
    }
    Some(index)
}

/// A required short-option value is either attached or the next word. Stop
/// at the first value-taking option; the remaining letters are its value.
fn xargs_short_option_needs_value(word: &str, options: &[&str]) -> bool {
    if !word.starts_with('-') || word.starts_with("--") || word.len() <= 2 {
        return false;
    }
    let mut flags = word[1..].chars().peekable();
    while let Some(flag) = flags.next() {
        // These GNU short options take only an optional attached value.
        if matches!(flag, 'i' | 'e' | 'l') {
            return false;
        }
        if options.iter().any(|option| *option == format!("-{flag}")) {
            return flags.peek().is_none();
        }
    }
    false
}

/// `30`, `1.5`, `30s`, `2m`: a number or `timeout` duration.
fn is_duration_word(word: &str) -> bool {
    let digits = word.trim_end_matches(['s', 'm', 'h', 'd']);
    !digits.is_empty() && digits.parse::<f64>().is_ok()
}

/// The textual catch-all: the command names the write-roots key with a
/// standalone `set`, or a grant command, and runs anything that is not a
/// read-only command.
fn operator_policy_command_textual(command: &str) -> bool {
    static SET_WORD: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let set_word = SET_WORD.get_or_init(|| {
        regex::Regex::new(r"(?:^|[^A-Za-z0-9_.\-])set(?:[^A-Za-z0-9_.\-]|$)").expect("valid set pattern")
    });
    let names_policy = (command.contains("factory.write_roots") && set_word.is_match(command))
        || command.contains("grant-write")
        || command.contains("revoke-write");
    if !names_policy {
        return false;
    }
    let statements = shell_statement_words(command);
    statements.is_empty()
        || !statements
            .iter()
            .all(|words| !runs_substitution(words) && read_only_statement(words))
}

/// cas-b7b2: a word that runs a command when the shell expands it: `$(…)`,
/// backticks, or process substitution. A read-only command with such an
/// argument still runs whatever is inside it, so the statement is not read
/// only. Quoting is not inspected: a single-quoted `$(` is refused too, which
/// costs a rare search and never admits a run.
fn runs_substitution(words: &[String]) -> bool {
    words
        .iter()
        .any(|word| word.contains("$(") || word.contains('`') || word.contains("<(") || word.contains(">("))
}

/// A statement that only reads or prints: it cannot change the policy by
/// itself (a printed command piped into a shell has the shell as another,
/// non-read-only statement).
fn read_only_statement(words: &[String]) -> bool {
    let Some(index) = unwrapped_command_index(words) else {
        return words.is_empty();
    };
    let args = &words[index + 1..];
    if words.first().is_some_and(|word| word == "xargs") {
        return policy_xargs_search(words);
    }
    // Eligibility is for bare command names, never a path's basename.
    match words[index].as_str() {
        name @ ("grep" | "egrep" | "fgrep" | "rg" | "cat" | "head" | "tail" | "wc" | "sort"
        | "uniq" | "cut" | "ls" | "echo" | "printf" | "cd" | "pwd" | "true" | "false"
        | "test" | "[") => index == 0 && policy_reader_flags(name, args),
        "set" => index == 0 && policy_set_flags(args),
        "git" => index == 0 && policy_git_flags(args),
        "cas" => {
            index == 0
                && matches!(args, [verb, action, key]
            if verb == "config" && action == "get" && !key.is_empty() && !key.starts_with('-'))
        }
        // cas-b7b2: searches that cas-1768 QA found refused, judged by
        // POSITIVE allowlists that fail closed. No environment assignment or
        // env/sudo-style prefix may precede them: awk and find read
        // environment that can load programs. gh is never admitted here: a
        // configured pager, browser or alias can run a command from what
        // looks like `gh <noun> view`, and that cannot be proved from the
        // statement text.
        // Security round 2: only as a direct invocation. Under xargs the
        // hook cannot see the arguments stdin supplies (a find expression,
        // or the awk program in replace mode).
        "awk" | "gawk" | "mawk" | "nawk" => index == 0 && read_only_awk(args),
        "find" => index == 0 && read_only_find(args),
        "jq" => index == 0 && read_only_jq(args),
        _ => false,
    }
}

/// Positive flag sets only. Operands are data, including words after `--`.
/// Short clusters contain only listed switches; value options stay separate
/// (or use `=`). The legacy numeric count is limited to tools that define it.
fn policy_output_flags(
    args: &[String],
    switches: &[&str],
    values: &[&str],
    numeric_count: bool,
) -> bool {
    let mut index = 0;
    while let Some(word) = args.get(index) {
        let word = word.as_str();
        if word == "--" {
            return true;
        }
        if switches.contains(&word) {
            index += 1;
            continue;
        }
        if values.contains(&word) {
            if args.get(index + 1).is_none() {
                return false;
            }
            index += 2;
            continue;
        }
        if word
            .split_once('=')
            .is_some_and(|(flag, _)| values.contains(&flag))
        {
            index += 1;
            continue;
        }
        if word.starts_with('-') && word != "-" {
            let short = &word[1..];
            let count = numeric_count
                && !short.is_empty()
                && short.bytes().all(|byte| byte.is_ascii_digit());
            let cluster = !short.is_empty()
                && short
                    .chars()
                    .all(|flag| switches.contains(&format!("-{flag}").as_str()));
            if !count && !cluster {
                return false;
            }
        }
        index += 1;
    }
    true
}

/// Keep a small output-only set per reader. Pagers and configurable search
/// frontends are intentionally absent rather than treated as generic reads.
fn policy_reader_flags(name: &str, args: &[String]) -> bool {
    let (switches, values, count): (&[&str], &[&str], bool) = match name {
        "grep" | "egrep" | "fgrep" => (
            &[
                "-n",
                "-i",
                "-r",
                "-R",
                "-l",
                "-c",
                "-E",
                "-F",
                "-w",
                "-v",
                "-h",
                "-H",
                "-q",
                "-s",
                "--line-number",
                "--ignore-case",
                "--recursive",
                "--files-with-matches",
                "--count",
                "--extended-regexp",
                "--fixed-strings",
                "--word-regexp",
                "--invert-match",
                "--color",
            ],
            &["-e", "--regexp", "--color"],
            false,
        ),
        "rg" => (
            &[
                "-n",
                "-i",
                "-l",
                "-c",
                "-F",
                "-w",
                "-v",
                "-H",
                "-s",
                "-S",
                "--line-number",
                "--ignore-case",
                "--files-with-matches",
                "--count",
                "--fixed-strings",
                "--word-regexp",
                "--invert-match",
                "--hidden",
                "--files",
                "--no-heading",
                "--heading",
                "--json",
            ],
            &[
                "-e",
                "--regexp",
                "-g",
                "--glob",
                "-t",
                "--type",
                "-T",
                "--type-not",
                "--color",
            ],
            false,
        ),
        "cat" => (
            &[
                "-n",
                "-b",
                "-s",
                "-E",
                "-T",
                "-A",
                "-v",
                "--number",
                "--number-nonblank",
                "--squeeze-blank",
                "--show-ends",
                "--show-tabs",
                "--show-all",
            ],
            &[],
            false,
        ),
        "head" | "tail" => (
            &["-q", "-v", "--quiet", "--verbose"],
            &["-n", "--lines", "-c", "--bytes"],
            true,
        ),
        "wc" => (
            &[
                "-l",
                "-w",
                "-c",
                "-m",
                "-L",
                "--lines",
                "--words",
                "--bytes",
                "--chars",
                "--max-line-length",
            ],
            &[],
            false,
        ),
        "sort" => (
            &[
                "-n",
                "-r",
                "-u",
                "-f",
                "-b",
                "-h",
                "-V",
                "--numeric-sort",
                "--reverse",
                "--unique",
                "--ignore-case",
                "--ignore-leading-blanks",
                "--human-numeric-sort",
                "--version-sort",
            ],
            &["-k", "--key", "-t", "--field-separator"],
            false,
        ),
        "uniq" => (
            &[
                "-c",
                "-d",
                "-u",
                "-i",
                "--count",
                "--repeated",
                "--unique",
                "--ignore-case",
            ],
            &[
                "-f",
                "--skip-fields",
                "-s",
                "--skip-chars",
                "-w",
                "--check-chars",
            ],
            false,
        ),
        "cut" => (
            &["-s", "--only-delimited", "--complement"],
            &[
                "-b",
                "--bytes",
                "-c",
                "--characters",
                "-f",
                "--fields",
                "-d",
                "--delimiter",
            ],
            false,
        ),
        "ls" => (
            &[
                "-l",
                "-a",
                "-A",
                "-h",
                "-R",
                "-d",
                "-1",
                "--all",
                "--almost-all",
                "--human-readable",
                "--recursive",
                "--directory",
            ],
            &[],
            false,
        ),
        "echo" => (&["-n", "-e", "-E"], &[], false),
        "printf" | "true" | "false" => (&[], &[], false),
        "cd" | "pwd" => (&["-L", "-P"], &[], false),
        "test" | "[" => (
            &[
                "-n", "-z", "-e", "-f", "-d", "-r", "-w", "-x", "-s", "-L", "-h", "-a", "-o",
                "-eq", "-ne", "-lt", "-le", "-gt", "-ge",
            ],
            &[],
            false,
        ),
        _ => return false,
    };
    policy_output_flags(args, switches, values, count)
}

fn policy_set_flags(args: &[String]) -> bool {
    let mut index = 0;
    while let Some(word) = args.get(index) {
        let Some(short) = word.strip_prefix('-').filter(|short| !short.is_empty()) else {
            return false;
        };
        let mut flags = short.chars().peekable();
        while let Some(flag) = flags.next() {
            if flag == 'o'
                && flags.peek().is_none()
                && args.get(index + 1).is_some_and(|value| value == "pipefail")
            {
                index += 1;
            } else if !"eufxC".contains(flag) {
                return false;
            }
        }
        index += 1;
    }
    true
}

/// The one wrapper exception: direct xargs, known separate-word options,
/// then bare grep whose own flags pass. No generic wrapper unwrapping here.
fn policy_xargs_search(words: &[String]) -> bool {
    let mut index = 1;
    while let Some(word) = words.get(index) {
        match word.as_str() {
            "-0" | "--null" | "-r" | "--no-run-if-empty" => index += 1,
            "-n" | "-P" => {
                if !words
                    .get(index + 1)
                    .is_some_and(|number| number.parse::<u32>().is_ok())
                {
                    return false;
                }
                index += 2;
            }
            flag if flag.starts_with('-') => return false,
            _ => break,
        }
    }
    words
        .get(index)
        .is_some_and(|word| word == "grep" && policy_reader_flags("grep", &words[index + 1..]))
}

fn policy_git_flags(args: &[String]) -> bool {
    let Some((sub, rest)) = args.split_first() else {
        return false;
    };
    let (switches, values, count): (&[&str], &[&str], bool) = match sub.as_str() {
        "log" => (
            &[
                "--oneline",
                "--stat",
                "--name-only",
                "--name-status",
                "--graph",
                "--all",
                "--reverse",
                "--no-merges",
                "--no-color",
            ],
            &[
                "-n",
                "--max-count",
                "--grep",
                "--author",
                "--since",
                "--until",
            ],
            true,
        ),
        "show" => (
            &[
                "--oneline",
                "--stat",
                "--name-only",
                "--name-status",
                "--summary",
                "--no-patch",
                "-s",
                "--no-color",
            ],
            &[],
            false,
        ),
        "diff" => (
            &[
                "--stat",
                "--name-only",
                "--name-status",
                "--numstat",
                "--shortstat",
                "--summary",
                "--no-patch",
                "--cached",
                "--staged",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
            ],
            &[],
            false,
        ),
        "status" => (
            &["-s", "-b", "--short", "--branch", "--porcelain"],
            &[],
            false,
        ),
        "grep" => return policy_reader_flags("grep", rest),
        "commit" => return policy_git_commit_message(rest),
        _ => return false,
    };
    policy_output_flags(rest, switches, values, count)
}

/// Approved message-data exception, without wrappers or global git options.
fn policy_git_commit_message(args: &[String]) -> bool {
    let mut index = 0;
    let mut message = false;
    while let Some(word) = args.get(index) {
        match word.as_str() {
            "-q" | "--quiet" | "-s" | "--signoff" | "--allow-empty" => index += 1,
            "-m" | "--message" => {
                if args.get(index + 1).is_none() {
                    return false;
                }
                message = true;
                index += 2;
            }
            value if value.starts_with("--message=") || value.starts_with("-m=") => {
                message = true;
                index += 1;
            }
            _ => return false,
        }
    }
    message
}

/// cas-b7b2: a jq run with only known flags (security round 2, L1). Module
/// search paths, program and data files, and positional-argument modes fail
/// closed. jq has no way to run a command or write a file itself.
fn read_only_jq(args: &[String]) -> bool {
    const FLAGS: &[&str] = &[
        "-r", "-j", "-c", "-n", "-s", "-e", "-S", "-C", "-M", "-a", "-R", "--raw-output",
        "--join-output", "--compact-output", "--null-input", "--slurp", "--exit-status",
        "--sort-keys", "--color-output", "--monochrome-output", "--ascii-output",
        "--raw-input", "--tab", "--stream", "--seq",
    ];
    let mut index = 0;
    while let Some(word) = args.get(index) {
        let word = word.as_str();
        index += match word {
            "--indent" => 2,
            "--arg" | "--argjson" => 3,
            "--" => break,
            _ if FLAGS.contains(&word) => 1,
            // Combined short flags such as `-rc`.
            _ if word.len() > 2
                && word.starts_with('-')
                && !word.starts_with("--")
                && word[1..].chars().all(|flag| "rjcnseSCMaR".contains(flag)) =>
            {
                1
            }
            _ if word.starts_with('-') => return false,
            _ => 1,
        };
    }
    true
}

/// cas-b7b2: an awk run that only reads and prints, by a POSITIVE
/// allowlist. Options: only `-F SEP` and `-v NAME=VALUE` (or their attached
/// forms) before the program; any other option, long option or dialect flag
/// (program files, includes, `-W`, `-e`, in-place) fails closed. Exactly one
/// program, given inline, with no construct that can run a command, read or
/// write outside the input, or load code: `system`, `|`, `<`, `>`,
/// `getline`, `@` directives. Operands after it are input files; a dash
/// operand other than `-` (stdin) fails closed.
fn read_only_awk(args: &[String]) -> bool {
    let mut index = 0;
    let program = loop {
        let Some(word) = args.get(index) else {
            return false;
        };
        match word.as_str() {
            "-F" | "-v" => index += 2,
            "--" => {
                index += 1;
                break args.get(index);
            }
            option if (option.starts_with("-F") || option.starts_with("-v")) && option.len() > 2 => {
                index += 1
            }
            option if option.starts_with('-') => return false,
            _ => break Some(word),
        }
    };
    let Some(program) = program else {
        return false;
    };
    const IO_CONSTRUCTS: &[&str] = &["system", "|", "<", ">", "getline", "@"];
    !IO_CONSTRUCTS.iter().any(|construct| program.contains(construct))
        && args[index + 1..]
            .iter()
            .all(|operand| operand == "-" || !operand.starts_with('-'))
}

/// cas-b7b2: a find that only lists, by a POSITIVE allowlist of options,
/// tests and output actions. Start paths come first; every later word must
/// be a recognised expression token or the value of one. Anything else
/// (actions that run, delete or write files, debug or file-list options)
/// fails closed.
fn read_only_find(args: &[String]) -> bool {
    const LEADING_OPTIONS: &[&str] = &["-L", "-H", "-P"];
    const BARE: &[&str] = &[
        "!", "(", ")", ",", "-not", "-o", "-or", "-a", "-and", "-print", "-print0", "-prune",
        "-depth", "-xdev", "-mount", "-empty", "-readable", "-writable", "-executable",
        "-nouser", "-nogroup", "-true", "-false", "-quit", "-ls", "-follow", "-daystart",
        "-noleaf", "-ignore_readdir_race", "-noignore_readdir_race",
    ];
    const WITH_VALUE: &[&str] = &[
        "-name", "-iname", "-path", "-ipath", "-wholename", "-iwholename", "-lname",
        "-ilname", "-regex", "-iregex", "-regextype", "-type", "-xtype", "-maxdepth",
        "-mindepth", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin",
        "-newer", "-anewer", "-cnewer", "-user", "-group", "-uid", "-gid", "-perm",
        "-links", "-inum", "-samefile", "-used", "-printf", "-fstype",
    ];
    let mut words = args.iter().peekable();
    while let Some(word) = words.peek() {
        if LEADING_OPTIONS.contains(&word.as_str()) {
            words.next();
        } else {
            break;
        }
    }
    // Start paths: plain words before the first expression token.
    while words
        .peek()
        .is_some_and(|word| !word.starts_with('-') && !BARE.contains(&word.as_str()))
    {
        words.next();
    }
    while let Some(word) = words.next() {
        let word = word.as_str();
        if BARE.contains(&word) {
            continue;
        }
        if WITH_VALUE.contains(&word) {
            if words.next().is_none() {
                return false;
            }
            continue;
        }
        return false;
    }
    true
}

/// cas-1b94: refusal for an agent's Bash `cas mcp add|remove|import` that
/// would remove, replace or alias a `factory.supervisor_only_mcp` server, by
/// the rule `proxy_add`/`proxy_remove` apply (cas-0d4f0). Ordinary servers
/// stay manageable; `cas mcp import --force` is refused while any
/// supervisor-only server is configured, since it may overwrite one.
#[cfg(feature = "mcp-proxy")]
fn supervisor_only_mcp_command_denial(command: &str, cas_root: &Path) -> Option<String> {
    use crate::config::operator_policy::{supervisor_only_mcp_names, supervisor_only_mcp_refusal};
    let mut only: Option<Vec<String>> = None;
    for args in cas_invocations(command) {
        let Some(at) = args.iter().position(|arg| arg == "mcp") else {
            continue;
        };
        let rest = &args[at + 1..];
        let Some(sub) = rest.first() else { continue };
        let only = only.get_or_insert_with(|| supervisor_only_mcp_names(cas_root));
        if only.is_empty() {
            return None;
        }
        let load = |scope: &str| {
            let path = match scope {
                "user" | "global" => cmcp_core::config::Scope::User.config_path().ok(),
                _ => Some(cas_root.join("proxy.toml")),
            };
            path.and_then(|path| cmcp_core::config::Config::load_from(&path).ok())
                .unwrap_or_default()
        };
        let refusal = match sub.as_str() {
            "remove" | "rm" => {
                let mut scope = "local".to_string();
                let mut name = None;
                let mut words = rest[1..].iter();
                while let Some(word) = words.next() {
                    match word.as_str() {
                        "-s" | "--scope" => scope = words.next().cloned().unwrap_or(scope),
                        flag if flag.starts_with('-') => {}
                        value => name = name.or(Some(value.to_string())),
                    }
                }
                name.and_then(|name| supervisor_only_mcp_refusal(&name, None, &load(&scope), only))
            }
            "add" => crate::cli::mcp_cmd::parse_add_args(&rest[1..])
                .ok()
                .and_then(|parsed| {
                    supervisor_only_mcp_refusal(
                        &parsed.name,
                        Some(&parsed.server_config),
                        &load(&parsed.scope),
                        only,
                    )
                }),
            "import" if rest.iter().any(|arg| arg == "--force") => Some(format!(
                "🚫 OPERATOR-ONLY (cas-0d4f0): `cas mcp import --force` may overwrite a supervisor-only MCP server ({}). factory.supervisor_only_mcp keeps those from workers, so only the operator runs it, from their own terminal. Ask the supervisor to request the operator's approval.",
                only.join(", ")
            )),
            _ => None,
        };
        if refusal.is_some() {
            return refusal;
        }
    }
    None
}

/// cas-3c26: the variables Cassy's role gates read. An agent that sets,
/// unsets or clears them for a command it runs is impersonating another role.
const ROLE_ENV_NAMES: &[&str] = &["CAS_AGENT_ROLE", "CAS_FACTORY_MODE"];

/// cas-3c26: refuse an agent shell command that changes its own Cassy role
/// environment: an assignment prefix, `env NAME=`, `env -u NAME`, `env -i`
/// before `cas`, `export`/`declare -x NAME=` or `unset NAME`.
fn role_env_tampering_denial(command: &str) -> Option<String> {
    role_env_tampering_at_depth(command, 0)
}

fn role_env_denial(what: &str) -> String {
    format!(
        "🚫 ROLE SPOOF REFUSED (cas-3c26): {what}. {} tell Cassy which role this agent has; an agent may not set, unset or clear them for the commands it runs. Run the command as you are. Operator-only actions are the operator's to run from their own terminal.",
        ROLE_ENV_NAMES.join(" and ")
    )
}

fn role_env_tampering_at_depth(command: &str, depth: usize) -> Option<String> {
    if depth > 3 {
        return None;
    }
    let role_name = |name: &str| ROLE_ENV_NAMES.contains(&name);
    let assigned_role = |word: &str| {
        word.split_once('=')
            .filter(|(name, _)| is_shell_variable_name(name) && role_name(name))
            .map(|(name, _)| name.to_string())
    };
    for words in shell_statement_words(command) {
        // Assignment prefixes: `CAS_AGENT_ROLE=supervisor cas …`.
        let mut index = 0;
        while let Some(word) = words.get(index) {
            let Some((name, _)) = word.split_once('=') else {
                break;
            };
            if !is_shell_variable_name(name) {
                break;
            }
            if role_name(name) {
                return Some(role_env_denial(&format!("this command assigns {name}")));
            }
            index += 1;
        }
        let Some(program) = words.get(index) else {
            continue;
        };
        let args = &words[index + 1..];
        match shell_word_basename(program) {
            "env" => {
                let mut clears_all = false;
                let mut position = 0;
                while let Some(arg) = args.get(position) {
                    let unset = if arg == "-u" || arg == "--unset" {
                        position += 1;
                        args.get(position).map(String::as_str)
                    } else if let Some(name) = arg.strip_prefix("--unset=") {
                        Some(name)
                    } else if let Some(name) =
                        arg.strip_prefix("-u").filter(|name| !name.is_empty())
                    {
                        Some(name)
                    } else {
                        None
                    };
                    if let Some(name) = unset {
                        if role_name(name) {
                            return Some(role_env_denial(&format!("this command unsets {name}")));
                        }
                        position += 1;
                        continue;
                    }
                    if arg == "-i" || arg == "--ignore-environment" || arg == "-" {
                        clears_all = true;
                    } else if ENV_VALUE_OPTIONS.contains(&arg.as_str()) {
                        position += 1;
                    } else if let Some(name) = assigned_role(arg) {
                        return Some(role_env_denial(&format!("this command assigns {name}")));
                    } else if !arg.starts_with('-') && !arg.contains('=') {
                        if clears_all && shell_word_basename(arg) == "cas" {
                            return Some(role_env_denial(
                                "`env -i` runs cas without this agent's role environment",
                            ));
                        }
                        break;
                    }
                    position += 1;
                }
            }
            "export" | "declare" | "typeset" | "local" | "readonly" => {
                if let Some(name) = args.iter().find_map(|arg| assigned_role(arg)) {
                    return Some(role_env_denial(&format!("this command assigns {name}")));
                }
            }
            "unset" => {
                if let Some(name) = args.iter().find(|arg| role_name(arg)) {
                    return Some(role_env_denial(&format!("this command unsets {name}")));
                }
            }
            // Wrappers whose option value is itself a shell command.
            "sh" | "bash" | "zsh" | "dash" | "script" | "su" | "runuser" => {
                for (position, arg) in args.iter().enumerate() {
                    let flags_c =
                        arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c');
                    if (flags_c || arg == "--command")
                        && let Some(payload) = args.get(position + 1)
                        && let Some(denial) = role_env_tampering_at_depth(payload, depth + 1)
                    {
                        return Some(denial);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// cas-3147: files a Bash command edits in place without naming them as a
/// write destination: `sed -i` / `perl -i` operands and `mv` sources (a
/// rename changes the source's directory entry).
fn bash_in_place_edit_targets(command: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for words in shell_statement_words(command) {
        let Some(index) = executable_word_index(&words) else {
            continue;
        };
        let name = shell_word_basename(&words[index]);
        let args = &words[index + 1..];
        match name {
            "sed" | "perl" => {
                let in_place = args.iter().any(|arg| {
                    arg == "--in-place"
                        || arg.starts_with("--in-place=")
                        || (arg.starts_with('-')
                            && !arg.starts_with("--")
                            && arg[1..].contains('i'))
                });
                if !in_place {
                    continue;
                }
                // The program is `-e`/`-f` values or, without them, the
                // first operand; every other operand is a file.
                let mut operands = Vec::new();
                let mut explicit_program = false;
                let mut iter = args.iter();
                while let Some(arg) = iter.next() {
                    if matches!(arg.as_str(), "-e" | "-f" | "--expression" | "--file") {
                        explicit_program = true;
                        iter.next();
                    } else if arg.starts_with("--expression=") || arg.starts_with("--file=") {
                        explicit_program = true;
                    } else if name == "perl"
                        && arg.starts_with('-')
                        && !arg.starts_with("--")
                        && arg.ends_with('e')
                    {
                        // `perl -pie 'code' file`: the bundle ends in -e.
                        explicit_program = true;
                        iter.next();
                    } else if !arg.starts_with('-') {
                        operands.push(arg.clone());
                    }
                }
                if !explicit_program && !operands.is_empty() {
                    operands.remove(0);
                }
                targets.extend(operands);
            }
            "mv" => {
                let mut operands: Vec<String> = args
                    .iter()
                    .filter(|arg| !arg.starts_with('-'))
                    .cloned()
                    .collect();
                operands.pop();
                targets.extend(operands);
            }
            _ => {}
        }
    }
    targets
}

/// cas-3147: [`apply_patch_write_targets`] with the change each header makes.
fn apply_patch_write_targets_with_modes(
    patch: &str,
) -> Vec<(String, super::write_roots::WriteMode)> {
    use super::write_roots::WriteMode;
    const HEADERS: [(&str, WriteMode); 4] = [
        ("*** Add File: ", WriteMode::Create),
        ("*** Update File: ", WriteMode::Edit),
        ("*** Delete File: ", WriteMode::Delete),
        ("*** Move to: ", WriteMode::Create),
    ];
    patch
        .lines()
        .filter_map(|line| {
            HEADERS.iter().find_map(|(header, mode)| {
                line.strip_prefix(header)
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(|path| (path.to_string(), *mode))
            })
        })
        .collect()
}

/// cas-3147, GH #1182: the operator write policy in effect for this hook
/// call, and the task IDs whose grants apply. Project roots come from the
/// project's `.cas/config.toml` `[factory] write_roots` (this `cas_root` is
/// the owning project's, also for a worker in an isolated worktree), unioned
/// with roots still left in the cas-3147 operator policy file. A grant applies
/// while its task is open: to the worker holding the task's lease, and to the
/// supervisor. An unreadable policy file fails closed to "no legacy roots or
/// grants". The project's config files and the policy directory stay
/// protected whatever is configured.
fn operator_write_policy_for(
    cas_root: &Path,
    stores: &mut ToolHookStores<'_>,
    input: &HookInput,
    is_supervisor: bool,
) -> (super::write_roots::WritePolicy, std::collections::HashSet<String>) {
    use super::write_roots::{WriteMode, WritePolicy, WriteRoot};
    use crate::config::operator_policy::{OperatorWriteMode, load_operator_policy};
    let modes = |modes: &std::collections::BTreeSet<OperatorWriteMode>| {
        modes
            .iter()
            .map(|mode| match mode {
                OperatorWriteMode::Create => WriteMode::Create,
                OperatorWriteMode::Edit => WriteMode::Edit,
                OperatorWriteMode::Delete => WriteMode::Delete,
            })
            .collect()
    };
    let configured_roots = stores.config().factory().write_roots;
    let mut policy = WritePolicy {
        roots: crate::config::operator_policy::effective_project_write_roots(&configured_roots, cas_root)
            .into_iter()
            .map(|path| WriteRoot {
                path,
                modes: [WriteMode::Create, WriteMode::Edit, WriteMode::Delete].into_iter().collect(),
                task_id: None,
            })
            .collect(),
        protected: super::write_roots::protected_policy_paths(cas_root),
        configured: !configured_roots.is_empty()
            || crate::config::operator_policy::operator_policy_path(cas_root).exists(),
    };
    let mut task_ids = std::collections::HashSet::new();
    let operator = match load_operator_policy(cas_root) {
        Ok(operator) => operator,
        Err(error) => {
            tracing::warn!(%error, "operator write policy unreadable; no legacy roots or grants apply");
            return (policy, task_ids);
        }
    };
    for root in &operator.roots {
        if !policy.roots.iter().any(|existing| existing.path == root.path) {
            policy.roots.push(WriteRoot {
                path: root.path.clone(),
                modes: modes(&root.modes),
                task_id: None,
            });
        }
    }
    if operator.grants.is_empty() {
        return (policy, task_ids);
    }
    let leased: std::collections::HashSet<String> = stores
        .agents()
        .and_then(|store| store.list_agent_leases(&current_agent_id(input)).ok())
        .map(|leases| leases.into_iter().map(|lease| lease.task_id).collect())
        .unwrap_or_default();
    for grant in &operator.grants {
        let open = stores
            .tasks()
            .and_then(|store| store.get(&grant.task).ok())
            .is_some_and(|task| task.status != cas_types::TaskStatus::Closed);
        if !open {
            continue;
        }
        if is_supervisor || leased.contains(&grant.task) {
            task_ids.insert(grant.task.clone());
        }
        policy.roots.push(WriteRoot {
            path: grant.path.clone(),
            modes: modes(&grant.modes),
            task_id: Some(grant.task.clone()),
        });
    }
    (policy, task_ids)
}

/// cas-3147 (GH #1169 item 3): audit every out-of-tree change a write root or
/// grant admitted.
fn log_write_root_uses(cas_root: &Path, input: &HookInput, uses: &[WriteRootUse]) {
    let tool = input.tool_name.as_deref().unwrap_or("unknown");
    let agent = current_agent_id(input);
    for used in uses {
        let path = used.path.display().to_string();
        let root = used.root.display().to_string();
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            cas_root,
            "workspace_write_root_used",
            &[
                ("tool", tool),
                ("path", path.as_str()),
                ("mode", used.mode.as_str()),
                ("root", root.as_str()),
                ("task", used.task_id.as_deref().unwrap_or("")),
                ("agent", agent.as_str()),
            ],
        );
    }
}

/// Name the operator remedy even before any write roots have been configured.
fn factory_workspace_contract_denial_with_roots(
    input: &HookInput,
    violation: &FactoryWriteViolation,
    configured_artifacts_root: Option<&str>,
    configured_scratch_root: Option<&str>,
    worktree_root: Option<&std::path::Path>,
    policy: &super::write_roots::WritePolicy,
    task_ids: &std::collections::HashSet<String>,
) -> String {
    let denial = factory_workspace_contract_denial(
        input,
        violation,
        configured_artifacts_root,
        configured_scratch_root,
        worktree_root,
    );
    let roots = policy.describe(task_ids);
    let roots = if roots.is_empty() {
        String::new()
    } else {
        format!(" Operator write roots in effect: {}.", roots.join("; "))
    };
    format!(
        "{denial}{roots} Only the operator can add directories to `factory.write_roots` in the project's `.cas/config.toml` (`cas config set factory.write_roots`) or add task grants."
    )
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
    match factory_write_decision(
        input,
        configured_artifacts_root,
        configured_scratch_root,
        is_supervisor,
        registered_worktree_root,
        &super::write_roots::WritePolicy::default(),
        &std::collections::HashSet::new(),
    ) {
        FactoryWriteDecision::Denied(violation) => Some(violation),
        FactoryWriteDecision::Allowed | FactoryWriteDecision::AllowedByRoot(_) => None,
    }
}

/// cas-49c0: every file a Codex `apply_patch` call adds, updates, deletes or
/// moves to, in patch order.
fn apply_patch_write_targets(patch: &str) -> Vec<String> {
    // Only header lines name files; `+`, `-` and ` ` lines are content, so a
    // header-looking line inside an added file is never a target.
    const HEADERS: [&str; 4] = ["*** Add File: ", "*** Update File: ", "*** Delete File: ", "*** Move to: "];
    patch
        .lines()
        .filter_map(|line| {
            HEADERS
                .iter()
                .find_map(|header| line.strip_prefix(header))
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        })
        .collect()
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

fn factory_workspace_contract_denial(
    input: &HookInput,
    violation: &FactoryWriteViolation,
    configured_artifacts_root: Option<&str>,
    configured_scratch_root: Option<&str>,
    worktree_root: Option<&std::path::Path>,
) -> String {
    if violation.matched_rule == "Codex hook input omits workdir" {
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: cannot resolve relative redirection `{}` because the Codex hook input omits workdir. Use an absolute in-worktree path.",
            violation.evaluated_path,
        );
    }
    let artifacts = crate::config::resolved_factory_artifacts_root(configured_artifacts_root)
        .display()
        .to_string();
    let scratch = configured_scratch_root
        .filter(|root| !root.trim().is_empty())
        .map(|root| format!(" or `{root}/...` for ephemeral scratch output"))
        .unwrap_or_default();
    let worktree = worktree_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from(&input.cwd));
    if violation.matched_rule == "an unresolved shell payload" {
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: cannot resolve shell filesystem effects (an unresolved shell payload). For gh issue or PR text, save a literal body file under your configured scratch root or assigned worktree and pass `--body-file <path>`. Assigned worktree: `{}`.",
            worktree.display(),
        );
    }
    if matches!(violation.matched_rule,
        "a write with an unresolved effective working directory"
        | "a link with an unresolved effective working directory"
        | "an unresolved shell payload"
        | "an unresolved shell executable"
        | "an unresolved shell write target"
        | "a shell command without a resolvable payload"
        | "a shell payload beyond the resolution limit"
        | "an unmatched shell scope") {
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: cannot resolve shell filesystem effects ({}). Use literal directory operands and a literal, bounded shell payload so every write can be checked against its effective directory. Assigned worktree: `{}`.",
            violation.matched_rule, worktree.display(),
        );
    }
    if violation.matched_rule == "unresolved shell variable" {
        let suffix = violation.evaluated_path.split_once('$').map(|(_, rest)| rest).unwrap_or("");
        let variable = if suffix.starts_with('$') {
            "$$".to_string()
        } else {
            let name: String = suffix.trim_start_matches('{').chars()
                .take_while(|ch| *ch == '_' || ch.is_ascii_alphanumeric()).collect();
            format!("${name}")
        };
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: cannot establish the write root because of unexpanded shell variable `{variable}` in `{}`. Use a literal sanctioned root before the variable suffix, or bind the variable with a simple earlier assignment. Assigned worktree: `{}`.",
            violation.evaluated_path, worktree.display(),
        );
    }
    if violation.matched_rule.starts_with("link") || violation.matched_rule.starts_with("a path through") {
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: {} (`{}` resolves to `{}`). A write is judged where it lands: a link may not point at, into or above a Cassy policy path (`.cas/config.toml`, `.cas/config.yaml`, `.cas/operator/`), a hard link is a write to the file it shares, and a write through a link the same command creates must land in your worktree, under `{}/<task-id>/`{} or in an operator write root. Use literal link operands. Assigned worktree: `{}`.",
            violation.matched_rule,
            violation.evaluated_path,
            violation.resolved_path.display(),
            artifacts,
            scratch,
            worktree.display(),
        );
    }
    if violation.matched_rule.starts_with("deletion") {
        return format!(
            "🚫 FACTORY WORKSPACE CONTRACT: deletion refused ({}): {}. Assigned worktree: `{}`. Deleting is allowed inside your worktree, under `{}/<task-id>/`{}, and for stale Cassy runtime files (sockets, locks, pid and session files) under ~/.cas. $HOME, /, /tmp, the repository and your worktree root are never deleted.",
            violation.matched_rule,
            violation.resolved_path.display(),
            worktree.display(),
            artifacts,
            scratch,
        );
    }
    format!(
        "🚫 FACTORY WORKSPACE CONTRACT: file creation outside the assigned worktree, durable artifacts root, configured scratch root, or harness exceptions is blocked: {}. Assigned worktree: `{}`. Use your worktree, `{}/<task-id>/` for durable proof{}; only this session's harness scratchpad is sanctioned for ephemeral notes. Bare /tmp and stray $HOME files are not sanctioned.",
        violation.resolved_path.display(),
        worktree.display(),
        artifacts,
        scratch,
    )
}

/// Persist a refusal for a history-changing Git command that targeted a
/// path outside the worker's assigned worktree. Keep the command itself out
/// of the event: Bash payloads may contain credentials or other secrets, but
/// the operation and evaluated path are enough to audit the refusal.
fn log_factory_git_scope_rejection(
    cas_root: &Path,
    input: &HookInput,
    target: &GitCommandTarget,
    command: &str,
) {
    let tool = input.tool_name.as_deref().unwrap_or("unknown");
    let payload_bytes = input
        .tool_input
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|payload| payload.len().to_string())
        .unwrap_or_else(|| "0".to_string());
    let operation_tokens = command
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| "'\"`;&|".contains(ch)))
        .collect::<Vec<_>>();
    let operation_index = operation_tokens.iter().position(|word| {
        matches!(
            *word,
            "commit" | "merge" | "push" | "rebase" | "reset" | "checkout" | "add"
        )
    });
    let operation = operation_index
        .map(|index| {
            let operation = operation_tokens[index];
            if matches!(operation, "reset" | "add") {
                operation_tokens
                    .get(index + 1)
                    .filter(|flag| {
                        (operation == "reset" && **flag == "--hard")
                            || (operation == "add" && **flag == "-A")
                    })
                    .map_or(operation, |_| {
                        if operation == "reset" {
                            "reset --hard"
                        } else {
                            "add -A"
                        }
                    })
            } else {
                operation
            }
        })
        .unwrap_or("unknown");
    let evaluated_path = target.path_for_scope().display().to_string();
    let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
        cas_root,
        "workspace_contract_git_rejection",
        &[
            ("tool", tool),
            ("evaluated_path", evaluated_path.as_str()),
            ("matched_rule", "history-changing git outside worker worktree"),
            ("git_operation", operation),
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
    let Ok(home) = home.canonicalize() else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(&home) else {
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

    #[test]
    fn harness_file_memory_canonicalizes_home_without_allowing_symlink_escape() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let memory = home.path().join(".claude/projects/project/memory");
        std::fs::create_dir_all(&memory).expect("memory directory");
        let inside = canonicalize_for_containment(&memory.join("context.md")).unwrap();
        assert!(is_harness_file_memory_path(&inside, Some(home.path())));

        symlink(outside.path(), memory.join("escape")).expect("escape symlink");
        let escaped = canonicalize_for_containment(&memory.join("escape/secret.md")).unwrap();
        assert!(!is_harness_file_memory_path(&escaped, Some(home.path())));
    }

    fn bash_input(command: &str, cwd: &Path) -> HookInput {
        HookInput {
            cwd: cwd.to_string_lossy().to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({ "command": command })),
            ..Default::default()
        }
    }

    /// cas-d95c: plain Cargo from the task-verifier subagent goes to the
    /// counted verifier lane; other Bash from the verifier is untouched.
    #[test]
    fn verifier_rust_builds_are_sent_to_the_capped_lane_cas_d95c() {
        let root = tempfile::tempdir().unwrap();
        // A verifier subagent runs under its spawning supervisor's role.
        let verifier = |command: &str| HookInput {
            agent_type: Some("task-verifier".to_string()),
            agent_role: Some("supervisor".to_string()),
            ..bash_input(command, root.path())
        };
        let output = handle_pre_tool_use_inner(
            &verifier("cargo nextest run -p cas --lib -E 'test(x)'"),
            Some(root.path()),
            None,
        )
        .unwrap();
        let rendered = serde_json::to_string(&output).unwrap();
        assert!(rendered.contains("deny") && rendered.contains("cas factory verifier-test"), "{rendered}");
        let output =
            handle_pre_tool_use_inner(&verifier("git log --oneline -3"), Some(root.path()), None).unwrap();
        assert!(!serde_json::to_string(&output).unwrap().contains("VERIFIER RUST BUILD"));
    }

    /// cas-a30e9: Codex edits through apply_patch, so its leases cover every
    /// file the patch adds, updates, deletes or moves.
    #[test]
    fn codex_apply_patch_edits_claim_every_written_file_cas_a30e9() {
        let input = tool_input(
            "apply_patch",
            serde_json::json!({"command": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: src/b.rs\n+new\n*** End Patch\n"}),
            Path::new("/repo"),
        );
        assert_eq!(worker_edit_targets(&input), ["src/a.rs", "src/b.rs"]);
        let edit = tool_input("Edit", serde_json::json!({"file_path": "/repo/src/c.rs"}), Path::new("/repo"));
        assert_eq!(worker_edit_targets(&edit), ["/repo/src/c.rs"]);
    }

    fn tool_input(tool: &str, payload: serde_json::Value, cwd: &Path) -> HookInput {
        HookInput {
            cwd: cwd.to_string_lossy().to_string(),
            tool_name: Some(tool.to_string()),
            tool_input: Some(payload),
            ..Default::default()
        }
    }

    #[test]
    fn support_cycle_write_uses_project_root_and_names_operator_remedy_cas_4f8e() {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().join("home");
        let worktree = fixture.path().join("worker");
        let support = home.join(".gabber-support");
        let neighbour = home.join(".gabber-support-other");
        for directory in [&worktree, &support, &neighbour, &home.join(".cas")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let mut env = TestEnvGuard::new();
        env.set("HOME", &home);
        env.set("CAS_CLONE_PATH", &worktree);
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_AGENT_NAME", "support-cycle-test");
        env.set("CAS_FACTORY_SESSION", "cas-4f8e-test");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let cas_root = crate::store::init_cas_dir(&worktree).unwrap();
        let roots = "[factory]\nwrite_roots = [\"~/.gabber-support\"]\n";
        // Host settings do not grant the project's writes.
        std::fs::write(home.join(".cas/config.toml"), roots).unwrap();
        let completed_at = "2026-10-19T14:00:00Z";
        let state = serde_json::json!({"last_cycle_completed_at": completed_at}).to_string();
        let input = tool_input("Write", serde_json::json!({
            "file_path": support.join("state.json"), "content": state,
        }), &worktree);
        let run = |input: &HookInput| {
            serde_json::to_value(handle_pre_tool_use(input, Some(&cas_root)).unwrap()).unwrap()
        };
        let denied = run(&input);
        assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny", "{denied}");
        assert!(denied.to_string().contains("factory.write_roots"),
            "an unconfigured refusal must name the operator's remedy: {denied}");
        assert!(denied.to_string().contains("project's `.cas/config.toml`"), "{denied}");

        std::fs::write(cas_root.join("config.toml"), roots).unwrap();
        let admitted = run(&input);
        assert_ne!(admitted["hookSpecificOutput"]["permissionDecision"], "deny", "{admitted}");
        // Execute only the admitted write; the watchdog's completion field survives.
        std::fs::write(support.join("state.json"), &state).unwrap();
        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(support.join("state.json")).unwrap(),
        ).unwrap();
        assert_eq!(persisted["last_cycle_completed_at"], completed_at);

        let outside = tool_input("Write", serde_json::json!({
            "file_path": neighbour.join("state.json"), "content": state,
        }), &worktree);
        let denied = run(&outside);
        assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny", "{denied}");
        assert!(!neighbour.join("state.json").exists());
    }

    /// cas-0d4f0: an agent shell cannot set or reset a security-relevant key
    /// through `cas config`, however it wraps the call; ordinary keys and
    /// reads stay available.
    #[test]
    fn cas_0d4f0_agent_shell_cannot_set_operator_only_keys() {
        for command in [
            "cas config set slack.transport any",
            "cas config set verification.force_bypass_allowed true",
            "cas config set verification.enabled false",
            "/home/u/.local/bin/cas config set factory.supervisor_only_mcp ''",
            "env -u CLAUDECODE cas config set factory.worker_credential_env GH_TOKEN",
            "bash -c 'cas config set qa.evidence_gate false'",
            "setsid cas config set qa.independent_pass false",
            "cas config set release.claude_account_allowlist a@example.com",
            "cas config reset slack.transport",
        ] {
            let denial = operator_policy_command_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("operator"), "{denial}");
        }
        for allowed in [
            "cas config set issues.repo example/project",
            "cas config get slack.transport",
            "cas config reset sync.min_helpful",
            "rg 'cas config set slack.transport' docs",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
    }

    /// cas-1b94: an agent's Bash `cas mcp add|remove|import` cannot remove,
    /// replace or alias a supervisor-only MCP server, however wrapped; the
    /// rule is the one `proxy_add`/`proxy_remove` apply, and ordinary servers
    /// stay manageable.
    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn cas_1b94_agent_shell_cannot_remove_or_alias_supervisor_only_mcp_servers() {
        let root = tempfile::tempdir().expect("cas root");
        std::fs::write(
            root.path().join("config.toml"),
            "[factory]\nsupervisor_only_mcp = [\"vercel\"]\n",
        )
        .expect("config");
        std::fs::write(
            root.path().join("proxy.toml"),
            "[servers.vercel]\ntransport = \"http\"\nurl = \"https://mcp.vercel.com\"\n\n[servers.docs]\ntransport = \"http\"\nurl = \"https://docs.example/mcp\"\n",
        )
        .expect("proxy config");
        for command in [
            "cas mcp remove vercel",
            "cas mcp remove -s local vercel",
            "cas mcp add vercel https://other.example/mcp",
            "cas mcp add deploys https://mcp.vercel.com",
            "bash -c 'cas mcp add --transport http deploys https://mcp.vercel.com'",
            "nohup /usr/local/bin/cas mcp remove vercel",
            "cas mcp import --from claude --force",
        ] {
            let denial = supervisor_only_mcp_command_denial(command, root.path())
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(
                denial.contains("supervisor-only") && denial.contains("operator"),
                "{denial}"
            );
        }
        for allowed in [
            "cas mcp remove docs",
            "cas mcp add search https://search.example/mcp",
            "cas mcp list",
            "cas mcp import --from claude",
            "rg 'cas mcp remove vercel' docs",
        ] {
            assert_eq!(
                supervisor_only_mcp_command_denial(allowed, root.path()),
                None,
                "{allowed}"
            );
        }

        // The PreToolUse hook applies it to an agent's Bash call.
        let input = bash_input("cas mcp remove vercel", root.path());
        let output = handle_pre_tool_use_inner(&input, Some(root.path()), None).expect("hook");
        let rendered = serde_json::to_string(&output).expect("serialize");
        assert!(
            rendered.contains("deny") && rendered.contains("supervisor-only"),
            "{rendered}"
        );

        // Nothing is supervisor-only: nothing to refuse.
        let open = tempfile::tempdir().expect("cas root");
        assert_eq!(
            supervisor_only_mcp_command_denial("cas mcp remove vercel", open.path()),
            None
        );
    }

    /// cas-0d4f0: no agent tool call writes a Cassy config file directly,
    /// whatever its role; other files under `.cas/` and other config.toml
    /// files are untouched by this rule.
    #[test]
    fn cas_0d4f0_agents_cannot_write_cas_config_files() {
        let fixture = tempfile::tempdir().unwrap();
        let main = fixture.path().canonicalize().unwrap().join("main");
        std::fs::create_dir_all(main.join(".cas")).unwrap();
        let file = main.join(".cas/config.toml").display().to_string();
        for (case, input) in [
            ("Write", tool_input("Write", serde_json::json!({"file_path": file, "content": "[slack]\ntransport = \"any\""}), &main)),
            ("Edit", tool_input("Edit", serde_json::json!({"file_path": file, "old_string": "violet", "new_string": "any"}), &main)),
            ("MultiEdit", tool_input("MultiEdit", serde_json::json!({"file_path": file, "edits": []}), &main)),
            ("apply_patch", tool_input("apply_patch", serde_json::json!({"command": format!("*** Begin Patch\n*** Update File: {file}\n@@\n+x\n*** End Patch")}), &main)),
            ("relative Write", tool_input("Write", serde_json::json!({"file_path": ".cas/config.toml", "content": ""}), &main)),
            ("yaml", tool_input("Write", serde_json::json!({"file_path": ".cas/config.yaml", "content": ""}), &main)),
            ("append", bash_input("printf '[qa]\\nevidence_gate = false\\n' >> .cas/config.toml", &main)),
            ("tee", bash_input(&format!("echo x | tee {file}"), &main)),
            ("sed -i", bash_input("sed -i 's/violet/any/' .cas/config.toml", &main)),
            ("cp over", bash_input("cp /tmp/forged.toml .cas/config.toml", &main)),
            ("mv over", bash_input(&format!("mv /tmp/forged.toml {file}"), &main)),
            ("rm", bash_input("rm -f .cas/config.toml", &main)),
        ] {
            let denial = cas_config_file_write_denial(&input)
                .unwrap_or_else(|| panic!("{case} must be refused"));
            assert!(denial.contains("cas config set"), "{case}: {denial}");
        }
        // The proxy server list: the same file-edit bypass that proxy_add's
        // supervisor-only refusal closes.
        let proxy = main.join(".cas/proxy.toml").display().to_string();
        for (case, input) in [
            ("proxy Write", tool_input("Write", serde_json::json!({"file_path": proxy, "content": "[servers.vercel2]"}), &main)),
            ("proxy Edit", tool_input("Edit", serde_json::json!({"file_path": ".cas/proxy.toml", "old_string": "a", "new_string": "b"}), &main)),
            ("proxy append", bash_input("echo '[servers.x]' >> .cas/proxy.toml", &main)),
            ("proxy sed -i", bash_input("sed -i '/vercel/d' .cas/proxy.toml", &main)),
            ("proxy rm", bash_input("rm .cas/proxy.toml", &main)),
        ] {
            let denial = cas_config_file_write_denial(&input)
                .unwrap_or_else(|| panic!("{case} must be refused"));
            assert!(denial.contains("proxy_add"), "{case}: {denial}");
        }
        for (case, input) in [
            ("notes", tool_input("Write", serde_json::json!({"file_path": ".cas/notes.md", "content": ""}), &main)),
            ("other config.toml", tool_input("Write", serde_json::json!({"file_path": "crates/x/config.toml", "content": ""}), &main)),
            ("other proxy.toml", tool_input("Write", serde_json::json!({"file_path": "fixtures/proxy.toml", "content": ""}), &main)),
            ("read", bash_input("cat .cas/config.toml", &main)),
            ("cas config set", bash_input("cas config set issues.repo a/b", &main)),
        ] {
            assert_eq!(cas_config_file_write_denial(&input), None, "{case}");
        }
    }

    /// cas-a673: through the real PreToolUse, words after an argument
    /// substitution's `)` are still that command's arguments, so filename data
    /// is not a config write. A real write stays refused, whether it follows an
    /// operator, sits inside the substitution, or follows a substitution in
    /// command position (`$(true) touch x` runs `touch`).
    #[cfg(unix)]
    #[test]
    fn cas_a673_argument_substitution_data_is_not_a_config_write() {
        let mut env = TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        let project = home.join("projects/a673");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let worktree = cas_root.join("worktrees/lively-puma-3");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::write(worktree.join("src/new.rs"), "pub fn new() {}\n").unwrap();
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD", "CAS_ROOT"] {
            env.remove(key);
        }
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "cas-a673");
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_AGENT_NAME", "lively-puma-3");
        env.set("CAS_CLONE_PATH", &worktree);
        let decide = |command: &str| -> (String, String) {
            let input = HookInput {
                hook_event_name: "PreToolUse".to_string(),
                agent_role: Some("worker".to_string()),
                ..bash_input(command, &worktree)
            };
            let output = handle_pre_tool_use(&input, Some(&cas_root)).expect("handler ok");
            let value = serde_json::to_value(output).unwrap();
            (
                value["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string(),
                value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string(),
            )
        };
        for command in [
            "echo $(cat src/new.rs) touch ../../config.toml",
            "echo $(cat src/new.rs) rm ../../config.toml",
            "printf '%s\\n' $(cat src/new.rs) mv src/new.rs ../../config.toml",
            "echo $(cat $(ls src)) tee ../../config.toml",
        ] {
            let (decision, reason) = decide(command);
            assert!(
                decision != "deny" && !reason.contains("CASSY CONFIG"),
                "read-only `{command}` must be allowed: {decision} {reason}"
            );
        }
        for command in [
            "touch ../../config.toml",
            "echo $(touch ../../config.toml)",
            "echo $(cat src/new.rs) $(rm ../../config.toml)",
            "$(true) touch ../../config.toml",
            "echo $(cat src/new.rs); touch ../../config.toml",
            "echo $(cat src/new.rs) && mv src/new.rs ../../config.toml",
            "(cd src) ; rm ../../config.toml",
            "case x in x) touch ../../config.toml ;; esac",
        ] {
            let (decision, reason) = decide(command);
            assert_eq!(decision, "deny", "`{command}` writes the config and stays refused: {reason}");
            assert!(reason.contains("CASSY CONFIG"), "`{command}`: {reason}");
        }
    }

    /// cas-b7a1: the shared statement splitter keeps an argument
    /// substitution's trailing words in the command that owns them, so data
    /// after `echo $(…)` is never an in-place edit. A substitution in command
    /// position, or after a wrapper whose command it may become, still ends
    /// the statement as before.
    #[test]
    fn cas_b7a1_statement_words_continue_after_an_argument_substitution() {
        let words = |command: &str| shell_statement_words(command);
        assert_eq!(
            words("echo $(cat f) sed -i x config.toml"),
            vec![vec!["cat", "f"], vec!["echo", "$", "sed", "-i", "x", "config.toml"]]
        );
        assert_eq!(
            words("echo $(cat $(ls src)) sed -i x c"),
            vec![vec!["ls", "src"], vec!["cat", "$"], vec!["echo", "$", "sed", "-i", "x", "c"]]
        );
        for (command, edit) in [
            ("$(true) sed -i x c", vec!["sed", "-i", "x", "c"]),
            ("X=$(cat f) sed -i x c", vec!["sed", "-i", "x", "c"]),
            ("env $(cat f) sed -i x c", vec!["sed", "-i", "x", "c"]),
            ("timeout $(cat t) sed -i x c", vec!["sed", "-i", "x", "c"]),
            ("(cd src) ; sed -i x c", vec!["sed", "-i", "x", "c"]),
            ("case a in a) sed -i x c ;; esac", vec!["sed", "-i", "x", "c"]),
        ] {
            assert!(
                words(command).contains(&edit.iter().map(|word| word.to_string()).collect()),
                "`{command}` keeps `sed` as its own statement: {:?}",
                words(command)
            );
        }
    }

    /// cas-b7a1: through the real PreToolUse, data after an argument
    /// substitution is not an in-place config edit; real in-place edits stay
    /// refused, including inside `$()` and after a command-position
    /// substitution.
    #[cfg(unix)]
    #[test]
    fn cas_b7a1_argument_substitution_data_is_not_an_in_place_config_edit() {
        let mut env = TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        let project = home.join("projects/b7a1");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let worktree = cas_root.join("worktrees/lively-puma-3");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::write(worktree.join("src/new.rs"), "pub fn new() {}\n").unwrap();
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD", "CAS_ROOT"] {
            env.remove(key);
        }
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "cas-b7a1");
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_AGENT_NAME", "lively-puma-3");
        env.set("CAS_CLONE_PATH", &worktree);
        let decide = |command: &str| -> (String, String) {
            let input = HookInput {
                hook_event_name: "PreToolUse".to_string(),
                agent_role: Some("worker".to_string()),
                ..bash_input(command, &worktree)
            };
            let output = handle_pre_tool_use(&input, Some(&cas_root)).expect("handler ok");
            let value = serde_json::to_value(output).unwrap();
            (
                value["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string(),
                value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string(),
            )
        };
        for command in [
            "echo $(cat src/new.rs) sed -i x ../../config.toml",
            "printf '%s\\n' $(cat src/new.rs) perl -pi -e x ../../config.toml",
        ] {
            let (decision, reason) = decide(command);
            assert!(
                decision != "deny",
                "read-only `{command}` must be allowed: {decision} {reason}"
            );
        }
        for command in [
            "sed -i x ../../config.toml",
            "echo $(sed -i x ../../config.toml)",
            "$(true) sed -i x ../../config.toml",
            "X=$(cat src/new.rs) sed -i x ../../config.toml",
            "env $(cat src/new.rs) sed -i x ../../config.toml",
            "echo $(cat src/new.rs); sed -i x ../../config.toml",
        ] {
            let (decision, reason) = decide(command);
            assert_eq!(decision, "deny", "`{command}` edits the config and stays refused: {reason}");
        }
    }

    #[test]
    fn apply_patch_targets_every_added_updated_deleted_and_moved_file_cas_49c0() {
        let patch = "*** Begin Patch\n\
                     *** Add File: src/new.rs\n+pub fn new() {}\n\
                     *** Update File: /abs/src/lib.rs\n*** Move to: /abs/src/moved.rs\n@@\n-a\n+b\n\
                     *** Delete File: ../other/gone.rs\n\
                     *** End Patch\n";
        assert_eq!(
            apply_patch_write_targets(patch),
            vec!["src/new.rs", "/abs/src/lib.rs", "/abs/src/moved.rs", "../other/gone.rs"]
        );
        // Patch body lines are content, never targets.
        assert!(apply_patch_write_targets("*** Begin Patch\n+*** Add File: /etc/x\n*** End Patch\n").is_empty());
    }

    /// cas-49c0: the worker write guard's decision table. A worker may write
    /// inside its own worktree and under the durable artifacts root; a write
    /// that resolves into the supervisor's main checkout (or any other repo)
    /// is refused with the path named, whichever tool makes it — including a
    /// Codex `apply_patch` with an absolute path.
    #[test]
    fn worker_write_guard_decision_table_cas_49c0() {
        let main = tempfile::tempdir().expect("main checkout");
        let worktree = main.path().join(".cas/worktrees/strong-puma-16");
        std::fs::create_dir_all(worktree.join("src")).expect("worktree");
        let artifacts = tempfile::tempdir().expect("artifacts root");
        let other_repo = tempfile::tempdir().expect("another repository");
        let artifacts_root = Some(artifacts.path().display().to_string());
        let main_file = main.path().join("cas-cli/src/mcp/tools/service/core.rs");
        let patch = |header: &str, path: &str| {
            serde_json::json!({ "command": format!("*** Begin Patch\n*** {header}: {path}\n+x\n*** End Patch\n") })
        };
        let cases: Vec<(&str, HookInput, Option<std::path::PathBuf>)> = vec![
            ("apply_patch inside the worktree", tool_input("apply_patch", patch("Update File", "src/lib.rs"), &worktree), None),
            ("apply_patch absolute inside the worktree", tool_input("apply_patch", patch("Add File", &worktree.join("src/new.rs").display().to_string()), &worktree), None),
            ("apply_patch into the artifacts root", tool_input("apply_patch", patch("Add File", &artifacts.path().join("cas-49c0/LEDGER.md").display().to_string()), &worktree), None),
            ("apply_patch absolute into the main checkout", tool_input("apply_patch", patch("Update File", &main_file.display().to_string()), &worktree), Some(main_file.clone())),
            ("apply_patch relative escape into the main checkout", tool_input("apply_patch", patch("Delete File", "../../../cas-cli/src/mcp/tools/service/core.rs"), &worktree), Some(main_file.clone())),
            ("apply_patch move into another repository", tool_input("apply_patch", serde_json::json!({ "command": format!("*** Begin Patch\n*** Update File: src/lib.rs\n*** Move to: {}\n*** End Patch\n", other_repo.path().join("lib.rs").display()) }), &worktree), Some(other_repo.path().join("lib.rs"))),
            ("Edit absolute into the main checkout", tool_input("Edit", serde_json::json!({ "file_path": main_file.display().to_string() }), &worktree), Some(main_file.clone())),
            ("Write inside the worktree", tool_input("Write", serde_json::json!({ "file_path": worktree.join("src/lib.rs").display().to_string() }), &worktree), None),
        ];
        for (case, input, expected) in cases {
            let violation =
                factory_write_violation(&input, &artifacts_root, None, false, Some(worktree.as_path()));
            assert_eq!(
                violation.as_ref().map(|violation| violation.resolved_path.clone()),
                expected.as_deref().and_then(canonicalize_for_containment),
                "{case}"
            );
            if let Some(violation) = violation {
                let denial = factory_workspace_contract_denial(
                    &input,
                    &violation,
                    artifacts_root.as_deref(),
                    None,
                    Some(worktree.as_path()),
                );
                assert!(
                    denial.contains(&violation.resolved_path.display().to_string())
                        && denial.contains(&worktree.display().to_string()),
                    "{case}: the refusal names the path and the worker's own worktree: {denial}"
                );
            }
        }
    }

    /// cas-39f3: a worker may delete a zero-byte, unheld `index.lock` in its
    /// own worktree's gitdir (`<repo>/.git/worktrees/<self>/`), the leftover
    /// of a git process killed mid-write. A lock with content, a lock a
    /// process still holds open, another worktree's lock, the main
    /// repository's lock and any other file in the gitdir stay refused.
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_may_remove_only_its_own_stale_gitdir_index_lock_cas_39f3() {
        let root = tempfile::tempdir().expect("fixture root");
        let root_path = root.path().canonicalize().expect("canonical fixture root");
        let home = root_path.join("home");
        let main = root_path.join("main");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&main, &["init", "-q"]);
        std::fs::write(main.join("README"), "x").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "init"]);
        let worktree = main.join(".cas/worktrees/brisk-otter-7");
        let other = main.join(".cas/worktrees/other-worker-8");
        git(&main, &["worktree", "add", "-q", "-b", "factory/brisk-otter-7", worktree.to_str().unwrap()]);
        git(&main, &["worktree", "add", "-q", "-b", "factory/other-worker-8", other.to_str().unwrap()]);
        let own_gitdir = main.join(".git/worktrees/brisk-otter-7");
        let other_gitdir = main.join(".git/worktrees/other-worker-8");
        let own_lock = own_gitdir.join("index.lock");
        let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        let stale = |path: &Path, body: &[u8]| {
            std::fs::write(path, body).unwrap();
            std::fs::File::options().write(true).open(path).unwrap().set_modified(aged).unwrap();
        };

        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let decide = |command: &str| {
            factory_write_violation(&bash_input(command, &worktree), &None, None, false, Some(worktree.as_path()))
        };
        let rm = |path: &Path| format!("rm -f {}", path.display());

        stale(&own_lock, b"");
        assert_eq!(decide(&rm(&own_lock)), None, "own zero-byte unheld index.lock");
        assert_eq!(
            decide("rm -f ../../../.git/worktrees/brisk-otter-7/index.lock"),
            None,
            "the same lock by relative path"
        );

        let refused = |case: &str, command: String| {
            let violation = decide(&command).unwrap_or_else(|| panic!("{case} must be refused: {command}"));
            assert!(violation.matched_rule.starts_with("deletion"), "{case}: {violation:?}");
            violation
        };
        stale(&own_lock, b"DIRC partial");
        refused("own lock with content", rm(&own_lock));
        stale(&own_lock, b"");
        let holder = std::fs::File::open(&own_lock).unwrap();
        let held = refused("own lock held open by a process", rm(&own_lock));
        assert!(held.matched_rule.contains("held"), "{held:?}");
        drop(holder);
        assert_eq!(decide(&rm(&own_lock)), None, "released again");

        let other_lock = other_gitdir.join("index.lock");
        stale(&other_lock, b"");
        refused("another worker's gitdir lock", rm(&other_lock));
        let main_lock = main.join(".git/index.lock");
        stale(&main_lock, b"");
        refused("the main repository's index.lock", rm(&main_lock));
        refused("another file in the own gitdir", rm(&own_gitdir.join("HEAD")));
        refused("the whole own gitdir", format!("rm -rf {}", own_gitdir.display()));
    }

    /// cas-3147 (GH #1169): fixture with a managed directory outside the
    /// worktree. Returns (root, home, worktree, managed, outside).
    fn write_roots_fixture(
        root: &Path,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let root = root.canonicalize().expect("canonical fixture root");
        let home = root.join("home");
        let worktree = root.join("main/.cas/worktrees/brisk-otter-7");
        let managed = home.join("soundwave-config/docs/requests");
        let outside = home.join("elsewhere");
        for dir in [&worktree, &managed, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(managed.join("existing.md"), "x").unwrap();
        std::fs::write(outside.join("dotfile"), "x").unwrap();
        (root, home, worktree, managed, outside)
    }

    fn root_policy(path: &Path, modes: &[super::super::write_roots::WriteMode], task: Option<&str>)
        -> super::super::write_roots::WritePolicy {
        super::super::write_roots::WritePolicy {
            roots: vec![super::super::write_roots::WriteRoot {
                path: path.to_path_buf(),
                modes: modes.iter().copied().collect(),
                task_id: task.map(str::to_string),
            }],
            protected: Vec::new(),
            configured: true,
        }
    }

    /// cas-3147 (GH #1169): a write root admits its modes for create, edit,
    /// rename and in-place shell edits under it, audits each admitted path,
    /// refuses unpermitted modes and paths outside it (including `..` and
    /// symlink escapes), and names the roots in the refusal. An empty policy
    /// keeps the default contract.
    #[cfg(unix)]
    #[test]
    fn cas_3147_write_roots_admit_their_modes_and_deny_everything_else() {
        use super::super::write_roots::{WriteMode, WritePolicy};
        let fixture = tempfile::tempdir().unwrap();
        let (_root, home, worktree, managed, outside) = write_roots_fixture(fixture.path());
        std::os::unix::fs::symlink(&outside, managed.join("escape")).unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let no_tasks = std::collections::HashSet::new();
        let policy = root_policy(&managed, &[WriteMode::Create, WriteMode::Edit], None);
        let decide = |input: HookInput, policy: &WritePolicy| {
            factory_write_decision(&input, &None, None, false, Some(worktree.as_path()), policy, &no_tasks)
        };
        let m = |name: &str| managed.join(name).display().to_string();

        let admitted: Vec<(&str, HookInput, WriteMode)> = vec![
            ("Write a new file", tool_input("Write", serde_json::json!({"file_path": m("new.md"), "content": "x"}), &worktree), WriteMode::Create),
            ("Edit an existing file", tool_input("Edit", serde_json::json!({"file_path": m("existing.md"), "old_string": "x", "new_string": "y"}), &worktree), WriteMode::Edit),
            ("tee a new file", bash_input(&format!("printf x | tee {}", m("tee.md")), &worktree), WriteMode::Create),
            ("apply_patch Add File", tool_input("apply_patch", serde_json::json!({"command": format!("*** Begin Patch\n*** Add File: {}\n+x\n*** End Patch", m("patch.md"))}), &worktree), WriteMode::Create),
            ("sed -i in place", bash_input(&format!("sed -i s/x/y/ {}", m("existing.md")), &worktree), WriteMode::Edit),
            ("rename inside the root", bash_input(&format!("mv {} {}", m("existing.md"), m("renamed.md")), &worktree), WriteMode::Edit),
        ];
        for (case, input, mode) in admitted {
            match decide(input, &policy) {
                FactoryWriteDecision::AllowedByRoot(uses) => {
                    assert!(uses.iter().any(|used| used.mode == mode && used.root == managed), "{case}: {uses:?}");
                    assert!(uses.iter().all(|used| used.path.starts_with(&managed)), "{case}: {uses:?}");
                }
                other => panic!("{case} must be admitted by the write root: {other:?}"),
            }
            assert!(
                matches!(decide(tool_input("Write", serde_json::json!({"file_path": m("new.md"), "content": "x"}), &worktree), &WritePolicy::default()), FactoryWriteDecision::Denied(_)),
                "{case}: with no roots configured the default contract still refuses"
            );
        }

        let refused: Vec<(&str, HookInput)> = vec![
            ("outside the root", tool_input("Write", serde_json::json!({"file_path": outside.join("x.md").display().to_string(), "content": "x"}), &worktree)),
            ("delete is not granted", bash_input(&format!("rm {}", m("existing.md")), &worktree)),
            ("lexical .. escape", tool_input("Write", serde_json::json!({"file_path": format!("{}/../../../elsewhere/y.md", managed.display()), "content": "x"}), &worktree)),
            ("symlink escape", tool_input("Write", serde_json::json!({"file_path": m("escape/z.md"), "content": "x"}), &worktree)),
            ("in-place edit outside", bash_input(&format!("sed -i s/x/y/ {}", outside.join("dotfile").display()), &worktree)),
            ("rename away from outside", bash_input(&format!("mv {} {}", outside.join("dotfile").display(), m("stolen")), &worktree)),
        ];
        for (case, input) in refused {
            let FactoryWriteDecision::Denied(violation) = decide(input.clone(), &policy) else {
                panic!("{case} must be refused");
            };
            let denial = factory_workspace_contract_denial_with_roots(
                &input, &violation, None, None, Some(worktree.as_path()), &policy, &no_tasks,
            );
            assert!(denial.contains(&managed.display().to_string()), "{case}: the refusal names the roots: {denial}");
        }

        let with_delete = root_policy(&managed, &[WriteMode::Create, WriteMode::Edit, WriteMode::Delete], None);
        assert!(
            matches!(decide(bash_input(&format!("rm {}", m("existing.md")), &worktree), &with_delete), FactoryWriteDecision::AllowedByRoot(_)),
            "a root with delete admits rm under it"
        );
    }

    /// cas-3147 (GH #1169): a one-off operator grant applies only while the
    /// agent works on the granted task.
    #[test]
    fn cas_3147_task_grant_admits_only_its_own_task() {
        use super::super::write_roots::WriteMode;
        let fixture = tempfile::tempdir().unwrap();
        let (_root, home, worktree, managed, _outside) = write_roots_fixture(fixture.path());
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let grant = root_policy(&managed, &[WriteMode::Create, WriteMode::Edit], Some("cas-1169"));
        let write = tool_input(
            "Write",
            serde_json::json!({"file_path": managed.join("INGEST-1.md").display().to_string(), "content": "x"}),
            &worktree,
        );
        let tasks = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<std::collections::HashSet<_>>();
        match factory_write_decision(&write, &None, None, false, Some(worktree.as_path()), &grant, &tasks(&["cas-1169"])) {
            FactoryWriteDecision::AllowedByRoot(uses) => {
                assert_eq!(uses[0].task_id.as_deref(), Some("cas-1169"), "{uses:?}");
            }
            other => panic!("the granted task is admitted: {other:?}"),
        }
        assert!(matches!(
            factory_write_decision(&write, &None, None, false, Some(worktree.as_path()), &grant, &tasks(&["cas-other"])),
            FactoryWriteDecision::Denied(_)
        ));
        assert!(matches!(
            factory_write_decision(&write, &None, None, true, Some(worktree.as_path()), &grant, &tasks(&[])),
            FactoryWriteDecision::Denied(_)
        ), "a supervisor with no granted task is refused too");
    }

    /// cas-3147: no agent (worker or supervisor) may write the operator
    /// policy through any tool, even when a write root or its own sanctioned
    /// checkout contains it.
    #[cfg(unix)]
    #[test]
    fn cas_3147_agents_cannot_write_the_operator_policy() {
        use super::super::write_roots::{WriteMode, WritePolicy, WriteRoot};
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let home = root.join("home");
        let main = root.join("main");
        let operator_dir = main.join(".cas/operator");
        std::fs::create_dir_all(&operator_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let policy_file = operator_dir.join("write-policy.toml");
        std::fs::write(&policy_file, "").unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        // The widest case: a root over the whole project, and a supervisor
        // whose sanctioned checkout is the main repository itself.
        let policy = WritePolicy {
            roots: vec![WriteRoot {
                path: main.clone(),
                modes: [WriteMode::Create, WriteMode::Edit, WriteMode::Delete].into_iter().collect(),
                task_id: None,
            }],
            protected: vec![operator_dir.clone()],
            configured: true,
        };
        let no_tasks = std::collections::HashSet::new();
        let file = policy_file.display().to_string();
        for (case, input) in [
            ("Write", tool_input("Write", serde_json::json!({"file_path": file, "content": "[[roots]]"}), &main)),
            ("Edit", tool_input("Edit", serde_json::json!({"file_path": file, "old_string": "", "new_string": "x"}), &main)),
            ("apply_patch", tool_input("apply_patch", serde_json::json!({"command": format!("*** Begin Patch\n*** Update File: {file}\n@@\n+x\n*** End Patch")}), &main)),
            ("tee", bash_input(&format!("echo x | tee {file}"), &main)),
            ("sed -i", bash_input(&format!("sed -i s/a/b/ {file}"), &main)),
            ("rm", bash_input(&format!("rm -f {file}"), &main)),
            ("mv in", bash_input(&format!("mv /tmp/forged.toml {file}"), &main)),
            ("relative", bash_input("cp forged.toml .cas/operator/write-policy.toml", &main)),
        ] {
            for supervisor in [false, true] {
                let decision = factory_write_decision(&input, &None, None, supervisor, Some(main.as_path()), &policy, &no_tasks);
                let FactoryWriteDecision::Denied(violation) = decision else {
                    panic!("{case} (supervisor={supervisor}) must be refused: {decision:?}");
                };
                assert!(violation.matched_rule.contains("operator"), "{case}: {violation:?}");
            }
        }
    }

    /// cas-3147, updated for GH #1182: the hook unions project roots from
    /// config.toml `[factory] write_roots` (all modes) with roots left in the
    /// operator policy file, applies a grant to the supervisor and to the
    /// worker holding the task, drops it once the task closes, and always
    /// protects the policy directory and the project config files. (Before
    /// GH #1182 this asserted that config.toml roots were ignored.)
    #[test]
    fn cas_3147_hook_policy_comes_from_the_operator_file_and_ends_at_task_close() {
        use crate::config::operator_policy::{
            OperatorWriteGrant, OperatorWriteMode, OperatorWritePolicy, OperatorWriteRoot,
            save_operator_policy,
        };
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let managed = project.path().canonicalize().unwrap().join("managed");
        let granted = project.path().canonicalize().unwrap().join("granted");
        let configured = project.path().canonicalize().unwrap().join("configured");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::create_dir_all(&granted).unwrap();
        std::fs::create_dir_all(&configured).unwrap();
        std::fs::write(
            cas_root.join("config.toml"),
            format!("[factory]\nwrite_roots = [\"{}\"]\n", configured.display()),
        )
        .unwrap();
        let tasks = crate::store::open_task_store_local(&cas_root).unwrap();
        let mut task = cas_types::Task::new("cas-1169a".into(), "INGEST request files".into());
        task.status = cas_types::TaskStatus::InProgress;
        tasks.add(&task).unwrap();
        save_operator_policy(
            &cas_root,
            &OperatorWritePolicy {
                roots: vec![OperatorWriteRoot {
                    path: managed.clone(),
                    modes: [OperatorWriteMode::Create, OperatorWriteMode::Edit].into_iter().collect(),
                }],
                grants: vec![OperatorWriteGrant {
                    task: task.id.clone(),
                    path: granted.clone(),
                    modes: [OperatorWriteMode::Create].into_iter().collect(),
                    reason: "INGEST request files".into(),
                    granted_at: "2026-10-10T18:00:00Z".into(),
                    granted_by: None,
                }],
            },
        )
        .unwrap();
        let input = tool_input("Write", serde_json::json!({"file_path": "x"}), project.path());

        let mut stores = ToolHookStores::new(&cas_root);
        let (policy, supervisor_tasks) = operator_write_policy_for(&cas_root, &mut stores, &input, true);
        let protected = vec![cas_root.join("operator"), cas_root.join("config.toml"), cas_root.join("config.yaml")];
        assert_eq!(policy.protected, protected);
        assert!(policy.configured);
        assert_eq!(policy.roots.len(), 3, "config.toml's root, the file's root and grant: {policy:?}");
        assert!(policy.roots.iter().any(|root| root.path == configured
            && root.task_id.is_none()
            && root.modes.len() == 3), "a config.toml root permits create, edit and delete: {policy:?}");
        assert!(policy.roots.iter().any(|root| root.path == managed && root.task_id.is_none()));
        assert!(policy.roots.iter().any(|root| root.path == granted && root.task_id.as_deref() == Some("cas-1169a")));
        assert!(supervisor_tasks.contains("cas-1169a"), "the supervisor uses an open task's grant");
        let (_, unleased_worker_tasks) = operator_write_policy_for(&cas_root, &mut stores, &input, false);
        assert!(unleased_worker_tasks.is_empty(), "a worker without the task's lease gets no grant");

        task.status = cas_types::TaskStatus::Closed;
        tasks.update(&task).unwrap();
        let mut stores = ToolHookStores::new(&cas_root);
        let (closed, closed_tasks) = operator_write_policy_for(&cas_root, &mut stores, &input, true);
        assert!(closed_tasks.is_empty(), "a grant ends when its task closes");
        assert!(closed.roots.iter().all(|root| root.task_id.is_none()), "{closed:?}");
        assert_eq!(closed.protected, protected);
    }

    /// cas-3147 review: with no operator policy file, in-place edits and
    /// renames keep today's behaviour (not judged), so the default contract
    /// is unchanged; the consistent classification starts only once the
    /// operator configures a policy.
    #[cfg(unix)]
    #[test]
    fn cas_3147_unconfigured_contract_keeps_in_place_edits_and_renames_as_today() {
        use super::super::write_roots::WritePolicy;
        let fixture = tempfile::tempdir().unwrap();
        let (_root, home, worktree, _managed, outside) = write_roots_fixture(fixture.path());
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let dotfile = outside.join("dotfile").display().to_string();
        let no_tasks = std::collections::HashSet::new();
        for command in [
            format!("sed -i s/x/y/ {dotfile}"),
            format!("perl -pi -e 's/x/y/' {dotfile}"),
            format!("mv {dotfile} src/dotfile"),
        ] {
            let input = bash_input(&command, &worktree);
            assert_eq!(
                factory_write_violation(&input, &None, None, false, Some(worktree.as_path())),
                None,
                "default contract: {command}"
            );
            let unconfigured = WritePolicy {
                protected: vec![fixture.path().join("main/.cas/operator")],
                ..WritePolicy::default()
            };
            assert_eq!(
                factory_write_decision(&input, &None, None, false, Some(worktree.as_path()), &unconfigured, &no_tasks),
                FactoryWriteDecision::Allowed,
                "no policy file: {command}"
            );
            let configured = WritePolicy { configured: true, ..unconfigured };
            assert!(
                matches!(
                    factory_write_decision(&input, &None, None, false, Some(worktree.as_path()), &configured, &no_tasks),
                    FactoryWriteDecision::Denied(_)
                ),
                "with a policy file the edit is judged: {command}"
            );
        }
    }

    /// cas-3147: an agent shell cannot run the operator-only commands,
    /// however the command is wrapped.
    #[test]
    fn cas_3147_agent_shell_cannot_run_operator_policy_commands() {
        for command in [
            "cas config set factory.write_roots ~/soundwave-config",
            "/home/u/.local/bin/cas config set factory.write_roots ''",
            "env -u CLAUDECODE -u CAS_AGENT_ROLE cas config set factory.write_roots /x",
            "cas config grant-write --task cas-1 --path /x --reason r",
            "bash -c 'cas config grant-write --task cas-1 --path /x --reason r'",
            "script -qc 'cas config set factory.write_roots /x' /dev/null",
            "setsid cas config set factory.write_roots /x",
        ] {
            let denial = operator_policy_command_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("operator"), "{denial}");
        }
        for allowed in [
            "cas config get factory.write_roots",
            "cas config set factory.prompt_retention_days 7",
            "git commit -m 'document cas config set factory.write_roots'",
            "rg 'grant-write' cas-cli/src",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
    }

    /// cas-3c26: an agent shell cannot pair or authorize a Commander device
    /// or launch a factory, however the command is wrapped (a PTY wrapper
    /// included); ordinary hub and factory subcommands stay allowed.
    #[test]
    fn cas_3c26_agent_shell_cannot_pair_authorize_or_launch_a_factory() {
        for command in [
            "cas hub pair --origin https://commander.example",
            "cas hub pair --origin https://x --scopes machine:read,pane:input,message:send",
            "cas hub authorize K7MW-4H2Q --yes",
            "script -qc 'cas hub pair --origin https://x' /dev/null",
            "bash -c \"cas hub authorize ABCD-EFGH --yes\"",
            "nohup cas factory --workers 3",
            "script -q -c 'cas factory' /dev/null",
        ] {
            let denial = operator_policy_command_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("OPERATOR-ONLY (cas-3c26)"), "{denial}");
        }
        for allowed in [
            "cas hub status",
            "cas factory status",
            "cas factory worker-status",
            "git commit -m 'cas hub pair now asks for confirmation'",
            "rg 'hub pair' cas-cli/src",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
    }

    /// GH #1182 QA F02: a write root admits changes strictly beneath it;
    /// the root directory itself cannot be deleted, renamed away or
    /// overwritten through it, even with delete permitted.
    #[cfg(unix)]
    #[test]
    fn gh_1182_write_root_never_admits_the_root_itself() {
        use super::super::write_roots::WriteMode;
        let fixture = tempfile::tempdir().unwrap();
        let (_root, home, worktree, managed, _outside) = write_roots_fixture(fixture.path());
        std::fs::create_dir_all(managed.join("sub")).unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let policy = root_policy(&managed, &[WriteMode::Create, WriteMode::Edit, WriteMode::Delete], None);
        let no_tasks = std::collections::HashSet::new();
        let decide = |command: String| {
            factory_write_decision(&bash_input(&command, &worktree), &None, None, false, Some(worktree.as_path()), &policy, &no_tasks)
        };
        let root = managed.display().to_string();
        for command in [
            format!("rm -rf {root}"),
            format!("rm -rf {root}/"),
            format!("rm -rf {root}/sub/.."),
            format!("rmdir {root}"),
            format!("mv {root} {}/moved", managed.join("sub").display()),
        ] {
            assert!(
                matches!(decide(command.clone()), FactoryWriteDecision::Denied(_)),
                "{command} must not delete or rename the root itself"
            );
        }
        for command in [format!("rm -rf {root}/sub"), format!("rm {root}/existing.md")] {
            assert!(
                matches!(decide(command.clone()), FactoryWriteDecision::AllowedByRoot(_)),
                "{command} beneath the root is admitted"
            );
        }
    }

    /// GH #1182 QA round 1: the CLI no longer refuses agent environment, so
    /// the hook must refuse the operator-only commands however they are
    /// wrapped, substituted, evaluated or run from an interpreter, while
    /// read-only commands that merely mention them stay allowed.
    #[test]
    fn gh_1182_agent_shell_cannot_run_wrapped_operator_policy_commands() {
        for command in [
            "timeout 30 env -i PATH=/usr/bin HOME=/home/u /abs/path/cas config set factory.write_roots \"~/Downloads\"",
            "timeout 30 cas config set factory.write_roots /etc",
            "timeout -s KILL --foreground 5m cas config set factory.write_roots /x",
            "nice -n 10 cas config set factory.write_roots /x",
            "nice -5 cas config set factory.write_roots /x",
            "nohup cas config set factory.write_roots /x &",
            "env -u CLAUDECODE FOO=1 cas config set factory.write_roots /x",
            "command cas config set factory.write_roots /x",
            "exec -a x cas config set factory.write_roots /x",
            "stdbuf -o L cas config set factory.write_roots /x",
            "stdbuf -oL cas config set factory.write_roots /x",
            "setsid -w cas config set factory.write_roots /x",
            "echo /x | xargs -I{} cas config set factory.write_roots {}",
            "xargs -n 1 cas config grant-write --task cas-1 --path",
            "time -p cas config set factory.write_roots /x",
            "chrt -f 10 cas config set factory.write_roots /x",
            "ionice -c 3 cas config set factory.write_roots /x",
            "flock -w 5 /tmp/lock cas config revoke-write --task cas-1",
            "FOO=1 BAR=2 cas config set factory.write_roots /x",
            "$(which cas) config set factory.write_roots /x",
            "$(command -v cas) config set factory.write_roots /x",
            "`which cas` config set factory.write_roots /x",
            "eval 'cas config set factory.write_roots /x'",
            "eval \"timeout 3 cas config set factory.write_roots /x\"",
            "sh -c 'nice cas config set factory.write_roots /x'",
            "bash -lc \"eval 'cas config set factory.write_roots /x'\"",
            "python3 -c 'import subprocess; subprocess.run([\"cas\", \"config\", \"set\", \"factory.write_roots\", \"/x\"])'",
            "perl -e 'system(\"cas config set factory.write_roots /x\")'",
            "python3 - <<'EOF'\nimport os\nos.system('cas config set factory.write_roots /x')\nEOF",
            "echo 'cas config set factory.write_roots /x' | sh",
            "node -e 'require(\"child_process\").execSync(\"cas config grant-write --task t --path /x --reason r\")'",
        ] {
            let denial = operator_policy_command_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("OPERATOR-ONLY"), "{denial}");
        }
        for allowed in [
            "cas config get factory.write_roots",
            "timeout 10 cas config get factory.write_roots",
            "grep -rn \"config set factory.write_roots\" docs",
            "rg -n 'cas config set factory.write_roots' cas-cli/src | head -20",
            "cd docs && grep -rn 'grant-write' .",
            "git log --oneline -5 --grep 'factory.write_roots set'",
            "git commit -m 'document cas config set factory.write_roots'",
            "set -e; cas config get factory.write_roots",
            "cas config set factory.note_max_chars 1800",
            "cat CHANGELOG.md | grep -n 'set factory.write_roots'",
            "python3 -c 'print(1)'",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
    }

    #[test]
    fn cas_f99e_config_read_shape_is_exact_and_direct() {
        for command in [
            "cas config get factory.write_roots",
            "cas config get ordinary.key",
        ] {
            let mention = format!("{command}; echo grant-write");
            assert_eq!(operator_policy_command_denial(&mention), None, "{command}");
        }
        let commands = [
            "env cas config get ordinary.key",
            "timeout 1 cas config get ordinary.key",
            "MODE=value cas config get ordinary.key",
            "cas config list",
            "cas config describe ordinary.key",
            "cas config search ordinary.key",
            "cas config docs",
            "cas zz-not-a-subcommand ordinary.key",
            "cas --zz-not-a-flag config get ordinary.key",
            "cas config get ordinary.key --zz-not-a-flag",
            "cas config get --zz-not-a-flag",
            "cas config get",
            "cas config get ordinary.key extra",
        ];
        let admitted: Vec<_> = commands
            .into_iter()
            .filter(|command| {
                operator_policy_command_denial(&format!("{command}; echo grant-write")).is_none()
            })
            .collect();
        assert!(
            admitted.is_empty(),
            "only direct exact config reads qualify: {admitted:?}"
        );
    }

    #[test]
    fn cas_f99e_xargs_search_is_grep_only() {
        for command in [
            "find docs -name '*.md' | xargs grep -n grant-write",
            "xargs -0 -r -n 2 -P 1 grep -n grant-write docs",
            "rg -n grant-write docs/guide.md",
        ] {
            assert_eq!(operator_policy_command_denial(command), None, "{command}");
        }
        let commands = [
            "xargs rg -n grant-write docs",
            "xargs grep --zz-not-a-flag grant-write",
        ];
        let admitted: Vec<_> = commands
            .into_iter()
            .filter(|command| operator_policy_command_denial(command).is_none())
            .collect();
        assert!(
            admitted.is_empty(),
            "only allowlisted grep qualifies after xargs: {admitted:?}"
        );
    }

    #[test]
    fn cas_f99e_policy_readers_require_bare_command_words() {
        for command in [
            "grep -n grant-write docs/guide.md",
            "cat -n docs/grant-write.md",
            "git log --oneline --grep grant-write",
            "awk '/grant-write/' docs/guide.md",
        ] {
            assert_eq!(operator_policy_command_denial(command), None, "{command}");
        }
        let commands = [
            "/usr/bin/grep -n grant-write docs",
            "./grep -n grant-write docs",
            "tools/cat -n grant-write.md",
            "./git log --grep grant-write",
            "./awk '/grant-write/' docs",
            "./find docs -name '*grant-write*'",
            "./jq .grant_write_events grant-write.json",
            "./cas config get ordinary.key; echo grant-write",
            "./xargs grep -n grant-write docs",
            "xargs ./grep -n grant-write docs",
            "./grep --zz-not-a-flag grant-write docs",
        ];
        let admitted: Vec<_> = commands
            .into_iter()
            .filter(|command| operator_policy_command_denial(command).is_none())
            .collect();
        assert!(
            admitted.is_empty(),
            "path-qualified command words do not qualify: {admitted:?}"
        );
    }

    #[test]
    fn cas_3b2f_policy_mentions_require_positive_tool_flags() {
        let allowed = [
            "grep -rn grant-write docs",
            "egrep -n grant-write docs",
            "fgrep -n grant-write docs",
            "rg -n --hidden grant-write docs",
            "cat -n grant-write.md",
            "head -n 20 grant-write.md",
            "tail -n 20 grant-write.md",
            "wc -l grant-write.md",
            "sort -nr grant-write.md",
            "uniq -c grant-write.md",
            "cut -f 1 grant-write.md",
            "ls -la grant-write.md",
            "echo -n grant-write",
            "printf '%s' grant-write",
            "cd docs && grep grant-write .",
            "pwd -P; grep grant-write docs",
            "true; grep grant-write docs",
            "false; grep grant-write docs",
            "test -n grant-write",
            "[ -n grant-write ]",
            "set -euo pipefail; grep grant-write docs",
        ];
        for command in allowed {
            assert_eq!(operator_policy_command_denial(command), None, "{command}");
        }
        let tools = [
            "grep", "egrep", "fgrep", "rg", "cat", "head", "tail", "wc", "sort", "uniq", "cut",
            "ls", "echo", "printf", "cd", "pwd", "true", "false", "test", "[", "set", "less",
            "more", "ag", "ack",
        ];
        let admitted: Vec<_> = tools
            .into_iter()
            .map(|tool| format!("{tool} --zz-not-a-flag grant-write"))
            .filter(|command| operator_policy_command_denial(command).is_none())
            .collect();
        assert!(
            admitted.is_empty(),
            "unknown flags must fail closed: {admitted:?}"
        );
        assert!(
            operator_policy_command_denial("grep --color --zz-not-a-flag grant-write").is_some()
        );
        assert_eq!(
            operator_policy_command_denial("grep --color=never -n grant-write docs"),
            None
        );
    }

    #[test]
    fn cas_3b2f_git_policy_mentions_require_subcommand_flags() {
        for command in [
            "git log --oneline -5 --grep grant-write",
            "git show --stat HEAD -- grant-write.md",
            "git diff --stat --name-only -- grant-write.md",
            "git status --short -- grant-write.md",
            "git grep -n grant-write",
            "git commit -m 'document grant-write' -q -s --allow-empty",
            "git commit --message='document grant-write' --quiet --signoff",
        ] {
            assert_eq!(operator_policy_command_denial(command), None, "{command}");
        }
        let mut refused: Vec<String> = ["log", "show", "diff", "status", "grep", "commit"]
            .into_iter()
            .map(|sub| format!("git {sub} --zz-not-a-flag grant-write"))
            .collect();
        refused.extend(
            [
                "git -c color.ui=never log --grep grant-write",
                "git --zz-not-a-global log --grep grant-write",
                "git zz-not-a-subcommand grant-write",
                "env git log --grep grant-write",
                "git commit --quiet; grep grant-write docs",
            ]
            .map(str::to_string),
        );
        let admitted: Vec<_> = refused
            .into_iter()
            .filter(|command| operator_policy_command_denial(command).is_none())
            .collect();
        assert!(
            admitted.is_empty(),
            "unlisted git forms must fail closed: {admitted:?}"
        );
    }

    #[test]
    fn cas_3b2f_only_tiny_xargs_search_prefix_is_allowed() {
        for command in [
            "find docs -name '*.md' | xargs grep -n grant-write",
            "xargs -0 -r -n 2 -P 1 grep -n grant-write docs",
            "xargs --null --no-run-if-empty grep -l grant-write",
        ] {
            assert_eq!(operator_policy_command_denial(command), None, "{command}");
        }
        let mut refused = vec![
            "xargs --zz-not-a-flag grep grant-write".to_string(),
            "xargs -zz grep grant-write".to_string(),
            "xargs -n2 grep grant-write".to_string(),
            "xargs cat grant-write.md".to_string(),
            "xargs grep --zz-not-a-flag grant-write".to_string(),
        ];
        refused.extend(
            [
                "env",
                "nice",
                "timeout 1",
                "sudo",
                "command",
                "exec",
                "stdbuf -oL",
            ]
            .map(|prefix| format!("{prefix} grep -n grant-write docs")),
        );
        let admitted: Vec<_> = refused
            .into_iter()
            .filter(|command| operator_policy_command_denial(command).is_none())
            .collect();
        assert!(
            admitted.is_empty(),
            "unlisted wrappers must fail closed: {admitted:?}"
        );
    }

    #[test]
    fn cas_3b2f_nonempty_statement_needs_a_command_word() {
        assert!(read_only_statement(&[]));
        for words in [vec!["command"], vec!["env", "--"], vec!["MODE=value"]] {
            let words: Vec<String> = words.into_iter().map(str::to_string).collect();
            assert!(
                !read_only_statement(&words),
                "nonempty statement: {words:?}"
            );
        }
    }

    #[test]
    fn cas_3b2f_structural_reader_visits_benign_env_payload() {
        let found = shell_invocations("env --split-string 'printf sample'");
        assert!(
            found
                .iter()
                .any(|(name, args)| name == "printf" && args == &["sample"])
        );
        for command in ["xargs -rI item printf sample", "xargs -Iitem printf sample"] {
            assert!(
                shell_invocations(command)
                    .iter()
                    .any(|(name, args)| name == "printf" && args == &["sample"])
            );
        }
    }

    #[test]
    fn cas_3b2f_duration_words_belong_only_to_timeout() {
        for (words, expected) in [
            (vec!["timeout", "2m", "printf", "sample"], 2),
            (vec!["nohup", "2m", "sample"], 1),
            (vec!["nice", "2", "sample"], 1),
            (vec!["chrt", "-f", "10", "printf", "sample"], 3),
        ] {
            let words: Vec<String> = words.into_iter().map(str::to_string).collect();
            assert_eq!(unwrapped_command_index(&words), Some(expected), "{words:?}");
        }
    }

    /// cas-b7b2: read-only searches that merely mention grant-write are
    /// allowed (cas-1768 QA round 2, Normal F01), while every way of RUNNING a
    /// policy command through the same tools stays refused. Security review
    /// round 1: awk and find are judged by POSITIVE allowlists that fail
    /// closed, and gh is never admitted (a configured pager, browser or alias
    /// can run a command from what looks like a read).
    #[test]
    fn cas_b7b2_read_only_searches_pass_and_disguised_invocations_do_not() {
        for allowed in [
            // QA forms 1 and 2.
            "awk '/grant-write/' docs/operator-policy.md",
            "find docs cas-cli/src -name '*.md' | xargs grep -n grant-write",
            // QA form 3, narrowed: gh cannot be proved built-in, so the issue
            // text is searched from a saved file.
            "grep -n grant-write artifacts/issue-1182.txt",
            // Neighbours.
            "awk -F: -v want=revoke-write '$0 ~ want {print FILENAME, NR}' CHANGELOG.md",
            "awk -F'\t' '/grant-write/ {print $1}' -",
            "find . -name '*grant-write*' -print",
            "find -L docs -type f -iname '*.md' -newer CHANGELOG.md -print0 | xargs -0 grep -l revoke-write",
            "find . -maxdepth 2 \\( -name '*.md' -o -name '*.rs' \\) -printf '%p\\n' | xargs grep -l grant-write",
            "jq '.grant_write_events' artifacts/report.json | grep grant-write",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
        for command in [
            // gh: never admitted in this path.
            "gh issue view 1182 --repo Richards-LLC/cassy | grep grant-write",
            "gh pr list --search grant-write",
            "gh search issues grant-write",
            "GH_PAGER=sh gh issue view 1182 | grep grant-write",
            // awk: unknown or dialect options, program sources, environment.
            "awk --source 'BEGIN{}' '/grant-write/' f",
            "gawk --include lib '/grant-write/' f",
            "gawk -i inplace '/grant-write/' f",
            "awk -f grant-write.awk f",
            "awk -e '/grant-write/' f",
            "mawk -W exec x grant-write",
            "awk -E x.awk grant-write",
            "AWKPATH=/tmp awk '/grant-write/' f",
            "env AWKLIBPATH=/tmp gawk '/grant-write/' f",
            "awk '@load \"x\"; /grant-write/' f",
            "awk '@include \"x\"; /grant-write/' f",
            "awk '/grant-write/' -f x",
            // awk: program constructs that run, read or write outside input.
            "awk 'BEGIN{system(\"cas config grant-write --task t --path /x\")}'",
            "awk '{c = \"cas config \" \"grant-write\"; print | c}' f",
            "awk 'BEGIN{\"cas config grant-write\" | getline x}'",
            "awk '/grant-write/ {print > \"/tmp/run.sh\"}' f",
            "awk '/grant-write/ {getline line < \"/etc/x\"}' f",
            "awk '/grant-write/' f | sh",
            // find: anything outside the positive list.
            "find . -name x -exec cas config grant-write --task t --path /x \\;",
            "find . -name grant-write -execdir sh -c x \\;",
            "find . -name grant-write -delete",
            "find . -name grant-write -ok rm {} \\;",
            "find . -name grant-write -okdir rm {} \\;",
            "find . -name grant-write -fprint /tmp/out",
            "find . -name grant-write -fls /tmp/out",
            "find . -name grant-write -fprintf /tmp/out x",
            "find -D exec . -name grant-write",
            "find . -files0-from list -name grant-write",
            "find . -name grant-write -newer x -o -some-new-action",
            "POSIXLY_CORRECT=1 find . -name grant-write",
            // xargs children, shells, substitution.
            "find . -name grant-write | xargs cas config grant-write --task t --path",
            "find . -print0 | xargs -0 sh -c 'cas config grant-write --task t --path \"$1\"' _",
            "find . -name x | xargs env AWKPATH=/tmp awk '/grant-write/'",
            "jq -r '.[\"grant-write\"]' f.json | sh",
            "AWKPATH=/x jq . f | grep grant-write | bash",
            "grep \"$(cas config grant-write --task t --path /x)\" notes.md",
            "grep `cas config revoke-write --task t` notes.md",
            "cat <(cas config grant-write --task t --path /x)",
            "sed -i 's/x/y/' grant-write.md",
        ] {
            let denial = operator_policy_command_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("OPERATOR-ONLY"), "{denial}");
        }
    }

    /// cas-b7b2 security round 2: xargs supplies arguments the hook cannot
    /// see, so awk, find and jq are read-only only when invoked directly; the
    /// xargs unwrapper reads GNU optional-argument long options as flags; a
    /// bare assignment or an allexport `set` is not read-only; jq takes only
    /// known flags.
    #[test]
    fn cas_b7b2_xargs_cannot_supply_what_the_hook_cannot_see() {
        for allowed in [
            "find docs cas-cli/src -name '*.md' | xargs grep -n grant-write",
            "awk '/grant-write/' docs/operator-policy.md",
            "jq -r --arg want grant-write '.[] | select(.name == $want)' events.json",
            "set -euo pipefail; grep -rn grant-write docs",
            "set -e; grep -rn revoke-write docs",
        ] {
            assert_eq!(operator_policy_command_denial(allowed), None, "{allowed}");
        }
        let commands = [
            // H1: the find expression arrives on stdin.
            // (The catch-all only judges commands that name a policy word.)
            "printf 'grant-write' | xargs find docs",
            "grep -l grant-write docs/* | xargs find",
            // H2: replace mode makes the awk program a placeholder.
            "printf 'x' | xargs -I{} awk {} docs/grant-write.md",
            "printf 'x' | xargs -i awk {} grant-write.md",
            // L2: an arg file feeding awk, and awk or jq under any xargs.
            "xargs -a list.txt awk '/grant-write/'",
            "xargs -a list.txt awk 'BEGIN{system(\"cas config grant-write\")}'",
            "find . -name '*.json' | xargs jq . | grep grant-write",
            // L2: jq with an environment assignment.
            "AWKPATH=/x jq . f | grep grant-write",
            // M1: optional-argument long options take no separate value.
            "printf 'x' | xargs --replace sh grep grant-write",
            "printf 'x' | xargs --eof sh grep grant-write",
            "printf 'x' | xargs --max-lines sh grep grant-write",
            // M2: bare assignments and allexport.
            "PATH=/tmp/evil; grep grant-write docs",
            "set -a; grep grant-write docs",
            "set -o allexport; grep revoke-write docs",
            "set -ea; grep grant-write docs",
            // L1: jq takes only known flags.
            "jq -L /tmp/mods 'import \"x\" as x; .' f | grep grant-write",
            "jq --rawfile x /etc/shadow -n '$x' | grep grant-write",
            "jq -f prog.jq f | grep grant-write",
        ];
        // Collect every leak so one run shows the whole gap.
        let admitted: Vec<&str> = commands
            .into_iter()
            .filter(|command| {
                !operator_policy_command_denial(command)
                    .is_some_and(|denial| denial.contains("OPERATOR-ONLY"))
            })
            .collect();
        assert!(admitted.is_empty(), "must be refused: {admitted:#?}");
    }

    /// cas-3c26: an agent cannot change the role environment its own commands
    /// run with, so `CAS_AGENT_ROLE` / `CAS_FACTORY_MODE` gates see the truth.
    #[test]
    fn cas_3c26_agent_shell_cannot_spoof_or_drop_its_cassy_role() {
        for command in [
            "CAS_AGENT_ROLE=supervisor cas task close cas-1 --supervisor-override",
            "env CAS_AGENT_ROLE=supervisor cas serve",
            "env -u CAS_AGENT_ROLE cas update",
            "env --unset=CAS_AGENT_ROLE cas update",
            "env --unset CAS_FACTORY_MODE cas factory status",
            "env -i PATH=/usr/bin cas update",
            "export CAS_AGENT_ROLE=supervisor",
            "declare -x CAS_FACTORY_MODE=1",
            "unset CAS_AGENT_ROLE; cas update",
            "cd /x && CAS_FACTORY_MODE= cas update",
            "bash -c 'CAS_AGENT_ROLE=supervisor cas serve'",
        ] {
            let denial = role_env_tampering_denial(command)
                .unwrap_or_else(|| panic!("{command} must be refused"));
            assert!(denial.contains("cas-3c26"), "{denial}");
        }
        for allowed in [
            "echo $CAS_AGENT_ROLE",
            "printenv CAS_AGENT_ROLE",
            "git commit -m 'CAS_AGENT_ROLE=worker is read by the role gates'",
            "rg CAS_AGENT_ROLE= cas-cli/src",
            "env -i PATH=/usr/bin cargo --version",
            "CAS_PROJECT=x cas task list",
        ] {
            assert_eq!(role_env_tampering_denial(allowed), None, "{allowed}");
        }
    }

    /// GH #1182 acceptance 4: with no write roots configured, no agent
    /// (worker or supervisor) can write the project's `.cas/config.toml` or
    /// `.cas/config.yaml` through any file tool or shell write, edit, rename
    /// or delete, so it cannot widen its own roots. Reading the file and
    /// `cas config set` for other keys stay allowed.
    #[cfg(unix)]
    #[test]
    fn gh_1182_agents_cannot_write_the_project_config() {
        use super::super::write_roots::{WritePolicy, protected_policy_paths};
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let home = root.join("home");
        let main = root.join("main");
        let cas_root = main.join(".cas");
        let worktree = cas_root.join("worktrees/brisk-otter-7");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(cas_root.join("config.toml"), "[factory]\n").unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let unconfigured = WritePolicy {
            roots: Vec::new(),
            protected: protected_policy_paths(&cas_root),
            configured: false,
        };
        let no_tasks = std::collections::HashSet::new();
        for file in ["config.toml", "config.yaml"] {
            let absolute = cas_root.join(file).display().to_string();
            let relative = format!(".cas/{file}");
            for (case, absolute_input, relative_input) in [
                ("Write", serde_json::json!({"file_path": absolute, "content": "[factory]\nwrite_roots = [\"/\"]"}), serde_json::json!({"file_path": relative, "content": "x"})),
                ("Edit", serde_json::json!({"file_path": absolute, "old_string": "[factory]", "new_string": "[factory]\nwrite_roots = [\"/x\"]"}), serde_json::json!({"file_path": relative, "old_string": "a", "new_string": "b"})),
                ("MultiEdit", serde_json::json!({"file_path": absolute, "edits": []}), serde_json::json!({"file_path": relative, "edits": []})),
                ("NotebookEdit", serde_json::json!({"notebook_path": absolute, "new_source": "x"}), serde_json::json!({"notebook_path": relative, "new_source": "x"})),
                ("apply_patch", serde_json::json!({"command": format!("*** Begin Patch\n*** Update File: {absolute}\n@@\n+x\n*** End Patch")}), serde_json::json!({"command": format!("*** Begin Patch\n*** Update File: {relative}\n@@\n+x\n*** End Patch")})),
            ] {
                for supervisor in [false, true] {
                    for (input, cwd) in [(&absolute_input, &worktree), (&relative_input, &main)] {
                        if cwd == &main && !supervisor {
                            continue;
                        }
                        let input = tool_input(case, input.clone(), cwd);
                        let decision = factory_write_decision(&input, &None, None, supervisor, Some(cwd.as_path()), &unconfigured, &no_tasks);
                        let FactoryWriteDecision::Denied(violation) = decision else {
                            panic!("{case} {file} (supervisor={supervisor}, cwd={}) must be refused: {decision:?}", cwd.display());
                        };
                        assert!(violation.matched_rule.contains("operator-only"), "{case}: {violation:?}");
                    }
                }
            }
            let commands = |target: &str| {
                vec![
                    format!("echo 'write_roots = [\"/\"]' > {target}"),
                    format!("printf x >> {target}"),
                    format!("echo x | tee -a {target}"),
                    format!("cp /tmp/forged.toml {target}"),
                    format!("mv /tmp/forged.toml {target}"),
                    format!("sed -i 's/a/b/' {target}"),
                    format!("perl -pi -e 's/a/b/' {target}"),
                    format!("mv {target} /tmp/old-config"),
                    format!("rm -f {target}"),
                ]
            };
            for supervisor in [false, true] {
                // The supervisor works in the main checkout with relative
                // paths; a worker reaches the file by its absolute path.
                let (cwd, target) = if supervisor { (&main, &relative) } else { (&worktree, &absolute) };
                for command in commands(target) {
                    let decision = factory_write_decision(&bash_input(&command, cwd), &None, None, supervisor, Some(cwd.as_path()), &unconfigured, &no_tasks);
                    let FactoryWriteDecision::Denied(violation) = decision else {
                        panic!("{command} (supervisor={supervisor}) must be refused: {decision:?}");
                    };
                    assert!(violation.matched_rule.contains("operator-only"), "{command}: {violation:?}");
                }
            }
        }
        for allowed in [
            "cat .cas/config.toml",
            "grep write_roots .cas/config.toml",
            "cas config set factory.note_max_chars 1800",
            "cp .cas/config.toml src/config-copy.toml",
        ] {
            assert_eq!(
                factory_write_decision(&bash_input(allowed, &main), &None, None, true, Some(main.as_path()), &unconfigured, &no_tasks),
                FactoryWriteDecision::Allowed,
                "{allowed}"
            );
        }
    }

    /// GH #1182: no write root ever admits a Cassy config or operator policy
    /// file, even another project's that lies under the root.
    #[cfg(unix)]
    #[test]
    fn gh_1182_write_roots_never_admit_any_projects_cas_policy_files() {
        use super::super::write_roots::WriteMode;
        let fixture = tempfile::tempdir().unwrap();
        let (_root, home, worktree, managed, _outside) = write_roots_fixture(fixture.path());
        let other_cas = managed.join("other-project/.cas");
        std::fs::create_dir_all(other_cas.join("operator")).unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let policy = root_policy(&managed, &[WriteMode::Create, WriteMode::Edit, WriteMode::Delete], None);
        let no_tasks = std::collections::HashSet::new();
        for path in [
            other_cas.join("config.toml"),
            other_cas.join("config.yaml"),
            other_cas.join("operator/write-policy.toml"),
        ] {
            let input = tool_input("Write", serde_json::json!({"file_path": path.display().to_string(), "content": "x"}), &worktree);
            assert!(
                matches!(
                    factory_write_decision(&input, &None, None, false, Some(worktree.as_path()), &policy, &no_tasks),
                    FactoryWriteDecision::Denied(_)
                ),
                "{} must not be admitted by a root",
                path.display()
            );
        }
        let ordinary = tool_input("Write", serde_json::json!({"file_path": managed.join("other-project/README.md").display().to_string(), "content": "x"}), &worktree);
        assert!(matches!(
            factory_write_decision(&ordinary, &None, None, false, Some(worktree.as_path()), &policy, &no_tasks),
            FactoryWriteDecision::AllowedByRoot(_)
        ));
    }

    /// GH #1182 acceptance 1 and 2, through the real PreToolUse handler with
    /// two Cassy roots: `[factory] write_roots` in project A's config.toml
    /// lets A's worker (in its isolated worktree) and A's supervisor create,
    /// edit, rename and delete under the root; a write outside it is refused
    /// and the refusal lists A's roots. Project B, with no key, is still
    /// refused the same writes.
    #[cfg(unix)]
    #[test]
    fn gh_1182_config_write_roots_admit_only_their_own_projects_agents() {
        let mut env = TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        let downloads = home.join("Downloads");
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let project_a = home.join("projects/a");
        let project_b = home.join("projects/b");
        let cas_a = crate::store::init_cas_dir(&project_a).unwrap();
        let cas_b = crate::store::init_cas_dir(&project_b).unwrap();
        let mut config = crate::config::Config::load(&cas_a).unwrap();
        config.set_factory_write_roots(vec![downloads.display().to_string()]);
        config.save(&cas_a).unwrap();
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD", "CAS_ROOT"] {
            env.remove(key);
        }
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "gh-1182");

        let mut decide = |cas_root: &Path, project: &Path, role: &str, input: HookInput| -> (String, String) {
            let worktree = cas_root.join("worktrees/brisk-otter-7");
            std::fs::create_dir_all(&worktree).unwrap();
            env.set("CAS_AGENT_ROLE", role);
            env.set("CAS_AGENT_NAME", format!("{role}-gh-1182"));
            let cwd = if role == "worker" {
                env.set("CAS_CLONE_PATH", &worktree);
                worktree
            } else {
                env.remove("CAS_CLONE_PATH");
                project.to_path_buf()
            };
            let input = HookInput {
                cwd: cwd.display().to_string(),
                hook_event_name: "PreToolUse".to_string(),
                agent_role: Some(role.to_string()),
                ..input
            };
            let output = handle_pre_tool_use(&input, Some(cas_root)).expect("handler ok");
            let value = serde_json::to_value(output).unwrap();
            (
                value["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string(),
                value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string(),
            )
        };
        let d = |name: &str| downloads.join(name).display().to_string();
        for role in ["worker", "supervisor"] {
            std::fs::write(downloads.join("existing.md"), "x").unwrap();
            std::fs::write(downloads.join("old.md"), "x").unwrap();
            let admitted = [
                ("create", tool_input("Write", serde_json::json!({"file_path": d("x"), "content": "x"}), &home)),
                ("edit", tool_input("Edit", serde_json::json!({"file_path": d("existing.md"), "old_string": "x", "new_string": "y"}), &home)),
                ("rename", bash_input(&format!("mv {} {}", d("existing.md"), d("renamed.md")), &home)),
                ("delete", bash_input(&format!("rm {}", d("old.md")), &home)),
            ];
            for (case, input) in &admitted {
                let (decision, reason) = decide(&cas_a, &project_a, role, input.clone());
                assert_ne!(decision, "deny", "project A {role} {case} must be admitted: {reason}");
                if role == "supervisor" && *case == "delete" {
                    // A supervisor may already delete under /tmp (cas-cf4f),
                    // where this fixture's HOME lives.
                    continue;
                }
                let (decision, reason) = decide(&cas_b, &project_b, role, input.clone());
                assert_eq!(decision, "deny", "project B {role} {case} has no roots and stays refused: {reason}");
            }
            let outside = tool_input("Write", serde_json::json!({"file_path": elsewhere.join("x").display().to_string(), "content": "x"}), &home);
            let (decision, reason) = decide(&cas_a, &project_a, role, outside);
            assert_eq!(decision, "deny", "{role}: outside the root is refused");
            assert!(reason.contains(&downloads.display().to_string()), "{role}: the refusal lists the roots: {reason}");
            let config_write = tool_input("Edit", serde_json::json!({"file_path": cas_a.join("config.toml").display().to_string(), "old_string": "a", "new_string": "b"}), &home);
            let (decision, reason) = decide(&cas_a, &project_a, role, config_write);
            assert_eq!(decision, "deny", "{role}: config.toml is never writable: {reason}");
        }
    }

    /// cas-7d68 (GH #1182 QA): a link the command itself creates does not
    /// exist when the hook runs, so a single-command link-then-write
    /// (`ln -s ../../.. up && echo x >> up/.cas/operator/write-policy.toml`)
    /// used to resolve inside the worktree and pass. Through the real
    /// PreToolUse handler, for worker and supervisor: every write is judged at
    /// the destination the command's own links give it, a link may never
    /// point at, into or above a Cassy policy path, a hard link is a write to
    /// its target, and the link path itself is a write. Links and writes that
    /// stay inside the permitted roots are unaffected.
    #[cfg(unix)]
    #[test]
    fn cas_7d68_single_command_link_then_write_cannot_reach_policy_or_escape_roots() {
        let mut env = TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        let outside = home.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let project = home.join("projects/a");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        std::fs::create_dir_all(cas_root.join("operator")).unwrap();
        let worktree = cas_root.join("worktrees/brisk-otter-7");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD", "CAS_ROOT"] {
            env.remove(key);
        }
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "cas-7d68");

        let mut decide = |role: &str, command: &str| -> (String, String) {
            env.set("CAS_AGENT_ROLE", role);
            env.set("CAS_AGENT_NAME", format!("{role}-cas-7d68"));
            let cwd = if role == "worker" {
                env.set("CAS_CLONE_PATH", &worktree);
                worktree.clone()
            } else {
                env.remove("CAS_CLONE_PATH");
                project.clone()
            };
            let input = HookInput {
                hook_event_name: "PreToolUse".to_string(),
                agent_role: Some(role.to_string()),
                ..bash_input(command, &cwd)
            };
            let output = handle_pre_tool_use(&input, Some(&cas_root)).expect("handler ok");
            let value = serde_json::to_value(output).unwrap();
            (
                value["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string(),
                value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string(),
            )
        };
        let outside_dir = outside.display().to_string();
        // Worker cwd: <project>/.cas/worktrees/brisk-otter-7.
        let worker_denied = [
            "ln -s ../../.. up && echo x >> up/.cas/operator/write-policy.toml".to_string(),
            "ln -s ../../.. up && echo x >> up/.cas/config.toml".to_string(),
            "ln -s ../../config.toml cfg && echo x >> cfg".to_string(),
            "ln -s ../../operator op; printf x > op/write-policy.toml".to_string(),
            "ln -sfn ../.. c && cp /tmp/forged.toml c/config.toml".to_string(),
            "ln -s ../../.. up && rm -rf up/.cas/operator".to_string(),
            "ln ../../config.toml cfg".to_string(),
            "mkdir -p d && ln -s ../../../.. d/up && echo x | tee -a d/up/.cas/operator/write-policy.toml".to_string(),
            "sh -c 'ln -s ../../config.toml cfg && echo x >> cfg'".to_string(),
            format!("ln -s {outside_dir} o && echo x > o/escaped.txt"),
            format!("ln -s {outside_dir}/target.txt t && echo x > t"),
            "ln -s \"$TARGET\" t && echo x > t".to_string(),
        ];
        for command in &worker_denied {
            let (decision, reason) = decide("worker", command);
            assert_eq!(decision, "deny", "worker must be refused `{command}`: {reason}");
        }
        // Supervisor cwd: the project's main checkout, which it may write.
        let supervisor_denied = [
            "ln -s .cas/operator op && echo x > op/write-policy.toml",
            "ln -s .cas c && echo x >> c/config.toml",
            "ln .cas/config.toml cfg && echo x >> cfg",
            "ln -sf /tmp/forged.toml .cas/config.toml",
            "ln -s ../.. up && echo x >> up/projects/a/.cas/operator/write-policy.toml",
            "cd src && ln -s ../.cas/operator op",
        ];
        for command in supervisor_denied {
            let (decision, reason) = decide("supervisor", command);
            assert_eq!(decision, "deny", "supervisor must be refused `{command}`: {reason}");
        }
        // A symlink left by an earlier command: `..` after it leaves its real
        // destination, not its lexical parent. `a/b/l -> <worktree>` makes
        // `a/b/l/../../config.toml` the project's `.cas/config.toml`.
        std::fs::create_dir_all(worktree.join("a/b")).unwrap();
        std::os::unix::fs::symlink(&worktree, worktree.join("a/b/l")).unwrap();
        for command in [
            "echo x >> a/b/l/../../config.toml",
            "echo x > a/b/l/../../operator/write-policy.toml",
            "link ../../config.toml cfg",
        ] {
            let (decision, reason) = decide("worker", command);
            assert_eq!(decision, "deny", "worker must be refused `{command}`: {reason}");
        }
        for (role, command) in [
            ("worker", "ln -s src s && echo x > s/new.rs"),
            ("worker", "ln -sf ../src/lib.rs src/alias.rs"),
            ("worker", "ln src/existing.rs src/hard.rs"),
            ("supervisor", "ln -s src s && echo x > s/new.rs"),
            ("supervisor", "cat .cas/config.toml"),
            ("worker", "ln -s src/existing.rs s.rs && cat s.rs"),
        ] {
            std::fs::write(worktree.join("src/existing.rs"), "x").unwrap();
            let (decision, reason) = decide(role, command);
            assert_ne!(decision, "deny", "{role} `{command}` stays allowed: {reason}");
        }
    }

    /// The three ordinary-worker allow-to-deny flips in the 200-call replay,
    /// with only the artifact namespace/cwd mapped into a hermetic project.
    #[test]
    fn cas_e460_literal_shell_bodies_keep_status_pid_and_read_only_monitor_commands() {
        let mut env = TestEnvGuard::temp_home();
        let project = env.home().join("project");
        let root = crate::store::init_cas_dir(&project).unwrap();
        let worktree = root.join("worktrees/worker-e460");
        let artifacts = crate::config::resolved_factory_artifact_paths(&root, None).project_root;
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(artifacts.join("cas-426f")).unwrap();
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_AGENT_NAME", "worker-e460");
        env.set("CAS_CLONE_PATH", &worktree);
        env.remove("CDPATH");
        let artifacts = artifacts.display();
        let commands = [
            format!(r#"mkdir -p {artifacts}/cas-426f; nohup bash -c 'scripts/release.sh; rc=$?; printf "\nRELEASE_EXIT=%s\n" "$rc"; exit "$rc"' > {artifacts}/cas-426f/release.log 2>&1 < /dev/null & echo $! > {artifacts}/cas-426f/release.pid"#),
            format!(r#"bash -c 'printf "%s\n" "$$" > {artifacts}/cas-426f/release.pid; scripts/release.sh > {artifacts}/cas-426f/release.log 2>&1; rc=$?; printf "\nRELEASE_EXIT=%s\n" "$rc" >> {artifacts}/cas-426f/release.log; exit "$rc"'"#),
            r#"timeout 590 bash -c 'while [ $(grep -c "Summary \[" target/w58r2.log) -lt 2 ]; do sleep 10; done'; grep -E "FAIL|Summary|SIGABRT|TIMEOUT" target/w58r2.log; grep -c PASS target/w58r2.log"#.to_string(),
            r#"sh -c 'printf "%s\n" "$?"'"#.to_string(),
        ];
        let mut mismatches = Vec::new();
        for command in commands {
            let input = HookInput { hook_event_name: "PreToolUse".into(),
                agent_role: Some("worker".into()), ..bash_input(&command, &worktree) };
            let output = handle_pre_tool_use(&input, Some(&root)).unwrap();
            let value = serde_json::to_value(output).unwrap();
            if value["hookSpecificOutput"]["permissionDecision"] != "allow" {
                mismatches.push(format!("{command}: {value}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn cas_e460_real_pre_tool_resolves_effective_cwd_and_nested_shell_writes() {
        let mut env = TestEnvGuard::temp_home();
        let project = env.home().join("project");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let worktree = cas_root.join("worktrees/worker-e460");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::create_dir_all(cas_root.join("operator")).unwrap();
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD", "CAS_ROOT", "CDPATH"] {
            env.remove(key);
        }
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "cas-e460");
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_AGENT_NAME", "worker-e460");
        env.set("CAS_CLONE_PATH", &worktree);
        let mut mismatches = Vec::new();
        for (command, denied) in [
            ("cd ../../operator && printf x > write-policy.toml", true),
            ("cd ../.. && printf x > config.toml", true),
            ("sh -c 'printf x > ../../operator/write-policy.toml'", true),
            ("bash -c 'cd ../.. && printf x > config.toml'", true),
            ("(cd ../.. && printf x > config.toml)", true),
            ("cd \"$UNKNOWN_CWD\" && printf x > file.txt", true),
            ("cd \"$LATER_CWD\"; LATER_CWD=src; printf x > file.txt", true),
            ("D=../..; cd \"$D\"; D=src; printf x > config.toml", true),
            ("{ cd ../..; }; printf x > config.toml", true),
            ("D=../..; false && D=src; cd \"$D\" && printf x > config.toml", true),
            ("false && D=src; cd \"$D\" && printf x > file.txt", true),
            ("CDPATH=../..; cd operator && printf x > write-policy.toml", true),
            ("CDPATH=../.. cd operator && printf x > write-policy.toml", true),
            ("sh -c 'cd && printf x > new.rs'; HOME=src", true),
            ("CDPATH=../..; cd ./src && printf x > new.rs", false),
            ("sh -c \"$UNKNOWN_SCRIPT\"", true),
            ("sh -c '$UNKNOWN_SCRIPT'", true),
            ("sh -c '$(printf touch) src/new.rs'", true),
            ("sh -c 'cd $(printf src) && printf x > new.rs'", true),
            ("sh -c 'printf x > \"$(printf src/new.rs)\"'", true),
            ("sh -c \"printf x $UNKNOWN_SCRIPT\"", true),
            ("sh -c 'printf \"%s\" \"$(cat src/new.rs)\"'", false),
            ("sh -c 'printf \"%s\" \"$(touch ../../config.toml)\"'", true),
            // cas-74b2: nested double quotes belong to the substitution's
            // shell, including parentheses that are merely filename data.
            (r#"printf '%s\n' "$(cat "status).txt")""#, false),
            (r#"printf '%s\n' "$(printf x > "../../outside).txt")""#, true),
            (r#"for file in "$(cat "status).txt")"; do cat "$file"; done"#, false),
            (r#"for file in "$(cat "status).txt")"; do rm ../../config.toml; done"#, true),
            (r#"while [ "$(cat "status).txt")" = waiting ]; do cat status.txt; done"#, false),
            (r#"while [ "$(cat "status).txt")" = waiting ]; do git -C ../.. push origin main; done"#, true),
            (r#"printf '%s\n' "$(cat"#, true),
            // cas-9623: a quoted cat heredoc is literal argument data. Its
            // quotes, substitutions and parentheses must not become syntax.
            ("gh issue comment 1290 --body \"$(cat <<'EOF'\nIt's done: $(touch /etc/cas-literal) (\nEOF\n)\"", false),
            ("gh issue comment 1290 --body \"$(cat <<\"EOF\"\nIt's done\nEOF\n)\"", false),
            ("gh issue comment 1290 --body \"$(cat <<'EOF'\ntext\nEOF\n)\" > ../../ordinary-data.txt", true),
            ("gh issue comment 1290 --body \"$(cat > ../../ordinary-data.txt <<'EOF'\nIt's done\nEOF\n)\"", true),
            ("gh issue comment 1290 --body \"$(cat <<EOF\n$(touch ../../ordinary-data.txt)\nEOF\n)\"", true),
            ("bash -c \"$(cat <<'EOF'\ntouch ../../ordinary-data.txt\nEOF\n)\"", true),
            ("sh -c \"printf '%s' $?\"", false),
            ("B=$PWD/target/debug/cas; $B --version", false),
            ("R='env -i PATH=/usr/bin /bin/true'; $R", false),
            ("kill $(cat target/a.pid) $(cat target/b.pid)", false),
            ("echo $(cat src/new.rs) touch ../../ordinary-data.txt", false),
            ("echo $(touch ../../config.toml) data", true),
            ("C=touch; $C ../../config.toml", true),
            ("C=touch; sh -c '$C ../../config.toml'", true),
            ("CMD=$UNKNOWN; $CMD", true),
            ("sh -c '$UNKNOWN/tool src/new.rs'", true),
            ("cd src && printf x > new.rs", false),
            ("D=src; cd \"$D\" && printf x > new.rs", false),
            ("cd ../.. && cd worktrees/worker-e460 && printf x > local.rs", false),
            ("cd ../.. && cd worktrees/worker-e460; printf x > config.toml", true),
            ("cd src && printf x > ../../../operator/write-policy.toml", true),
            ("cd src; cd ../../..; printf x > config.toml", true),
            ("cd ../.. || printf x > config.toml", true),
            ("if true; then cd ../..; fi; printf x > config.toml", true),
            ("(cd ../..); printf x > config.toml", false),
            ("sh -c 'cd ../..'; printf x > config.toml", false),
            ("cd src && sh -c 'printf x > ../../../config.toml'", true),
            ("sh -c 'bash -c \"printf x > ../../config.toml\"'", true),
            ("sh -c 'rm ../../config.toml'", true),
            ("cd ../.. && sed -i 's/a/b/' config.toml", true),
            ("cd src && ln -s ../../../operator op", true),
            ("cd src && ln -s . local && printf x > local/new.rs", false),
            ("cd src && python3 - <<'PY'\nopen('../../../config.toml','w').write('x')\nPY", true),
            ("cd src && python3 - <<'PY'\nopen('new.rs','w').write('x')\nPY", false),
            ("cd \"$UNKNOWN_CWD\" && cat file.txt", false),
            ("sh -c 'printf x > src/new.rs'", false),
            ("(cd src && printf x > new.rs); printf x > local.rs", false),
            ("cd ../.. && cat config.toml", false),
        ] {
            let input = HookInput { hook_event_name: "PreToolUse".to_string(),
                agent_role: Some("worker".to_string()), ..bash_input(command, &worktree) };
            let output = handle_pre_tool_use(&input, Some(&cas_root)).unwrap();
            let value = serde_json::to_value(output).unwrap();
            let decision = value["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("");
            if (decision == "deny") != denied {
                mismatches.push(format!("{command}: expected denied={denied}, got {value}"));
            }
            if command == r#"printf '%s\n' "$(cat"#
                && !value["hookSpecificOutput"]["permissionDecisionReason"].as_str()
                    .is_some_and(|reason| reason.contains("cannot resolve shell filesystem effects"))
            {
                mismatches.push(format!("malformed substitution needs a resolution reason: {value}"));
            }
            if command.starts_with("gh issue comment") && denied
                && value["hookSpecificOutput"]["permissionDecisionReason"].as_str()
                    .is_some_and(|reason| reason.contains("an unresolved shell payload")
                        && !reason.contains("--body-file"))
            {
                mismatches.push(format!("unresolved gh body needs --body-file guidance: {value}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    /// cas-cf4f: `rm` is deletion, not creation. A worker may delete inside
    /// its sanctioned roots and stale Cassy runtime files (sockets, locks,
    /// pid and session files) under ~/.cas; the supervisor may also clear
    /// caches under /tmp. `$HOME`, `/`, `/tmp`, the repository and the
    /// worktree root are never deletable, and every other delete is refused
    /// as a deletion with its reason, never as "file creation".
    #[test]
    fn rm_is_classified_as_deletion_decision_table_cas_cf4f() {
        let root = tempfile::tempdir().expect("fixture root");
        let root_path = root.path().canonicalize().expect("canonical fixture root");
        let home = root_path.join("home");
        let main = root_path.join("main");
        let worktree = main.join(".cas/worktrees/solid-condor-23");
        let artifacts = root_path.join("artifacts");
        for dir in [home.join(".cas/sessions"), worktree.join("src"), artifacts.join("cas-cf4f/old")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for file in [
            home.join(".cas/factory-qa4cd6.gui.sock"),
            home.join(".cas/sessions/qa4cd6.json.lock"),
            home.join(".cas/sessions/qa4cd6.json"),
            home.join(".cas/daemon.pid"),
            home.join(".zshrc"),
            worktree.join("src/stale.rs"),
        ] {
            std::fs::write(file, b"").unwrap();
        }
        let tmp_cache = tempfile::Builder::new()
            .prefix("cas-cf4f-cache")
            .tempdir_in("/tmp")
            .expect("/tmp cache dir");
        let tmp_cache_path = tmp_cache.path().display().to_string();

        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let artifacts_root = Some(artifacts.display().to_string());
        let decide = |command: &str, supervisor: bool| {
            factory_write_violation(
                &bash_input(command, &worktree),
                &artifacts_root,
                None,
                supervisor,
                Some(worktree.as_path()),
            )
        };

        let allowed: Vec<(&str, String, bool)> = vec![
            ("stale socket and lock under ~/.cas", "rm -f ~/.cas/factory-qa4cd6.gui.sock ~/.cas/sessions/qa4cd6.json.lock".into(), false),
            ("stale session file", "rm ~/.cas/sessions/qa4cd6.json".into(), false),
            ("stale pid file via $HOME", "rm -f $HOME/.cas/daemon.pid".into(), false),
            ("a file in the worktree", "rm -f src/stale.rs".into(), false),
            ("task-scoped artifacts", format!("rm -rf {}", artifacts.join("cas-cf4f/old").display()), false),
            ("supervisor clears a /tmp cache", format!("rm -rf {tmp_cache_path}"), true),
        ];
        for (case, command, supervisor) in &allowed {
            assert_eq!(decide(command, *supervisor), None, "{case}: {command}");
        }

        let refused: Vec<(&str, String, bool, &str)> = vec![
            ("all of $HOME", "rm -rf ~".into(), false, "protected"),
            ("all of $HOME via the variable", "rm -rf $HOME".into(), true, "protected"),
            ("the main checkout", format!("rm -rf {}", main.display()), true, "protected"),
            ("the worktree root", "rm -rf .".into(), false, "protected"),
            ("/tmp itself", "rm -rf /tmp".into(), true, "protected"),
            ("the whole ~/.cas directory", "rm -rf ~/.cas".into(), false, "outside"),
            ("an unrelated home file", "rm ~/.zshrc".into(), false, "outside"),
            ("a worker clearing /tmp", format!("rm -rf {tmp_cache_path}"), false, "outside"),
        ];
        for (case, command, supervisor, rule) in &refused {
            let violation = decide(command, *supervisor).unwrap_or_else(|| panic!("{case} must be refused: {command}"));
            assert!(violation.matched_rule.starts_with("deletion"), "{case}: {violation:?}");
            assert!(violation.matched_rule.contains(rule), "{case}: {violation:?}");
            let denial = factory_workspace_contract_denial(
                &bash_input(command, &worktree),
                &violation,
                artifacts_root.as_deref(),
                None,
                Some(worktree.as_path()),
            );
            assert!(
                denial.contains("deletion") && !denial.contains("file creation"),
                "{case}: a delete is refused as a deletion, with its reason: {denial}"
            );
        }

        // Creation keeps its own rule: `touch` of the same socket path is
        // still refused as file creation outside the sanctioned roots.
        let created = decide("touch ~/.cas/factory-qa4cd6.gui.sock", false).expect("creation is still guarded");
        assert_eq!(created.matched_rule, "none");
    }

    /// cas-aa4e: a ~/.cas runtime leftover may be deleted only when it is
    /// stale. A socket someone still listens on, a lock someone holds or
    /// whose recorded pid is alive, a pid file or session record whose pid is
    /// alive: all refused with the reason. Stale ones are allowed.
    #[cfg(unix)]
    #[test]
    fn runtime_leftover_delete_requires_staleness_decision_table_cas_aa4e() {
        use fs2::FileExt;
        // Unix socket paths are short-limited (about 104 bytes on macOS).
        let root = tempfile::Builder::new().prefix("aa4e").tempdir_in("/tmp").expect("short root");
        let home = root.path().canonicalize().unwrap().join("h");
        let cas = home.join(".cas");
        let worktree = root.path().canonicalize().unwrap().join("wt");
        std::fs::create_dir_all(cas.join("sessions")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        // A pid that has exited.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        let live_pid = std::process::id();

        let stale_sock = cas.join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale_sock).unwrap());
        let live_sock = cas.join("live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live_sock).unwrap();
        let free_lock = cas.join("free.lock");
        std::fs::write(&free_lock, b"").unwrap();
        let held_lock = cas.join("held.lock");
        std::fs::write(&held_lock, b"").unwrap();
        let holder = std::fs::File::open(&held_lock).unwrap();
        holder.lock_exclusive().unwrap();
        let pid_lock_live = cas.join("runner.lock");
        std::fs::write(&pid_lock_live, format!("{live_pid}\n")).unwrap();
        let pid_lock_dead = cas.join("old-runner.lock");
        std::fs::write(&pid_lock_dead, format!("{dead_pid}\n")).unwrap();
        let live_pidfile = cas.join("daemon.pid");
        std::fs::write(&live_pidfile, format!("{live_pid}\n")).unwrap();
        let dead_pidfile = cas.join("old-daemon.pid");
        std::fs::write(&dead_pidfile, format!("{dead_pid}\n")).unwrap();
        let live_session = cas.join("sessions/live.json");
        std::fs::write(&live_session, format!("{{\"daemon_pid\": {live_pid}}}")).unwrap();
        let dead_session = cas.join("sessions/dead.json");
        std::fs::write(&dead_session, format!("{{\"daemon_pid\": {dead_pid}}}")).unwrap();

        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("HOME", &home);
        env.remove("CAS_CLONE_PATH");
        for key in ["CAS_SCRATCHPAD", "CAS_SCRATCHPAD_PATH", "CLAUDE_SCRATCHPAD"] {
            env.remove(key);
        }
        let artifacts_root = Some(root.path().join("artifacts").display().to_string());
        let decide = |path: &std::path::Path| {
            factory_write_violation(
                &bash_input(&format!("rm -f {}", path.display()), &worktree),
                &artifacts_root,
                None,
                false,
                Some(worktree.as_path()),
            )
        };

        for (case, path) in [
            ("socket with connect() refused", &stale_sock),
            ("lock nobody holds", &free_lock),
            ("lock recording a dead pid", &pid_lock_dead),
            ("pid file of a dead pid", &dead_pidfile),
            ("session record of a dead daemon", &dead_session),
        ] {
            assert_eq!(decide(path), None, "{case}: a stale leftover is deletable");
        }
        for (case, path, reason) in [
            ("socket with a live listener", &live_sock, "listener"),
            ("lock held by another handle", &held_lock, "held"),
            ("lock recording a live pid", &pid_lock_live, "alive"),
            ("pid file of a live pid", &live_pidfile, "alive"),
            ("session record of a live daemon", &live_session, "alive"),
        ] {
            let violation = decide(path).unwrap_or_else(|| panic!("{case} must be refused"));
            assert!(
                violation.matched_rule.contains("live Cassy runtime file")
                    && violation.matched_rule.contains(reason),
                "{case}: {violation:?}"
            );
        }
        drop(holder);
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
    fn cas_2366_chained_assignments_resolve_before_workspace_checks() {
        let root = tempfile::tempdir().expect("fixture root");
        let root_path = root.path().canonicalize().unwrap();
        let worktree = root_path.join("worktree");
        let artifacts = root_path.join("artifacts");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&artifacts).unwrap();
        let artifacts_root = Some(artifacts.display().to_string());

        for (suffix, command) in [
            ("cas-2366/post", format!("R={}; P=$R/cas-2366/post; mkdir -p $P", artifacts.display())),
            ("cas-2366/post/more", format!("R={}; P=$R/cas-2366/post; Q=${{P}}/more; mkdir -p \"$Q\"", artifacts.display())),
        ] {
            assert_eq!(bash_write_targets(&command), vec![artifacts.join(suffix).display().to_string()]);
            assert_eq!(factory_write_violation(
                &bash_input(&command, &worktree), &artifacts_root, None, true, Some(&worktree)
            ), None, "known assignment chain must resolve inside artifacts: {command}");
        }

        let command = "R=..; mkdir -p $R/x";
        assert_eq!(bash_write_targets(command), vec!["../x"]);
        let violation = factory_write_violation(
            &bash_input(command, &worktree), &artifacts_root, None, true, Some(&worktree)
        ).expect("resolved path outside the worktree remains guarded");
        assert_eq!(violation.resolved_path, root_path.join("x"));
        assert_eq!(violation.matched_rule, "none");

        let command = "P=$UNKNOWN/x; mkdir -p $P";
        let violation = factory_write_violation(
            &bash_input(command, &worktree), &artifacts_root, None, true, Some(&worktree)
        ).expect("unknown values remain guarded");
        assert_eq!(violation.matched_rule, "unresolved shell variable");
    }

    #[test]
    fn bash_rm_targets_expand_loop_variables() {
        // cas-cf4f: rm operands are deletion targets, still expanded and guarded.
        let targets: Vec<String> = bash_delete_targets(
            "B=cas-cli/src/builtins; for h in codex grok; do for sk in cas-html-reports cas-dataviz; do rm -rf $B/$h/skills/$sk; done; done",
        )
        .into_iter()
        .map(|(target, recursive)| {
            assert!(recursive, "-rf is recursive");
            target
        })
        .collect();
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
    fn bash_heredoc_js_regex_literal_is_not_a_write_target() {
        let targets =
            bash_write_targets("cat > scripts/x.mjs <<'EOF'\nconst ok = /s/.test(\"s\");\nEOF");

        assert_eq!(targets, vec!["scripts/x.mjs"]);
    }

    #[test]
    fn bash_heredoc_body_paths_are_opaque_but_redirect_target_is_guarded() {
        let cwd = tempfile::tempdir().expect("worktree");
        std::fs::create_dir(cwd.path().join("scripts")).expect("scripts directory");
        let body_only = bash_input(
            "cat > scripts/x.mjs <<'EOF'\nconst output = \"/etc/x\";\ntouch /etc/cas-heredoc-body\nEOF",
            cwd.path(),
        );
        assert_eq!(
            factory_write_violation(&body_only, &None, None, false, Some(cwd.path())),
            None,
            "an absolute path in heredoc content is not a write target"
        );

        let outside_redirect =
            bash_input("cat > /etc/x <<'EOF'\nconst ok = true;\nEOF", cwd.path());
        assert_eq!(
            factory_write_violation(&outside_redirect, &None, None, false, Some(cwd.path()))
                .map(|violation| violation.resolved_path),
            canonicalize_for_containment(std::path::Path::new("/etc/x")),
            "an absolute redirect target must remain guarded"
        );
    }

    #[test]
    fn issue_payload_script_text_is_not_a_workspace_target() {
        let cwd = tempfile::tempdir().expect("worktree");
        let target = cwd.path().join("tests/generated.ts");
        std::fs::create_dir_all(target.parent().expect("target parent")).expect("target dir");
        let target = target.to_string_lossy();
        let commands = [
            format!("cat > '{target}' <<'EOF'\nconst closing = '</script>';\nEOF"),
            format!(
                "python3 - <<'PY'\nfrom pathlib import Path\npath = '{target}'\nsource = source.replace(\"${{canonicalEmail}}\", canonical_email)\nPath(path).write_text(source)\nPY"
            ),
            format!(
                "python3 - <<'PY'\nfrom pathlib import Path\npath = '{target}'\nsource = \"target.$transaction(async () => true)\"\nPath(path).write_text(source)\nPY"
            ),
            format!(
                "python3 - <<'PY'\nfrom pathlib import Path\npath = '{target}'\nreplacement = \"{{{{ a }}}} / {{{{ b }}}}\"\nPath(path).write_text(replacement)\nPY"
            ),
            format!(
                "python3 - <<'PY'\nfrom pathlib import Path\npath = '{target}'\nreplacement = \"/{{{{ learnerCards.length }}}}\"\nPath(path).write_text(replacement)\nPY"
            ),
            format!(
                "python3 - <<'PY'\nfrom pathlib import Path\npath = '{target}'\nsource = \"learner:${{s.workspaceId}}:${{s.projectSlug}}:${{s.itemId}}\"\nPath(path).write_text(source)\nPY"
            ),
        ];

        for command in commands {
            let input = bash_input(&command, cwd.path());
            assert_eq!(
                bash_write_targets(&command),
                vec![target.to_string()],
                "only the actual output file is a target: {command}"
            );
            assert_eq!(
                factory_write_violation(&input, &None, None, false, Some(cwd.path())),
                None,
                "in-worktree issue payload must be allowed: {command}"
            );
        }
    }

    #[test]
    fn script_write_targets_are_structural_and_outside_paths_remain_denied() {
        let cwd = tempfile::tempdir().expect("worktree");
        let inside = cwd.path().join("generated.ts");
        for command in [
            format!(
                "python3 -c 'from pathlib import Path; Path(\"{}\").write_text(\"/{{{{ not a path }}}}\")'",
                inside.display()
            ),
            format!(
                "node -e 'require(\"fs\").writeFileSync(\"{}\", \"target.$transaction\")'",
                inside.display()
            ),
        ] {
            let input = bash_input(&command, cwd.path());
            assert_eq!(
                factory_write_violation(&input, &None, None, false, Some(cwd.path())),
                None,
                "embedded script strings must not become targets: {command}"
            );
        }

        for (command, expected) in [
            (
                "python3 -c 'from pathlib import Path; Path(\"/tmp/cas-script-escape\").write_text(\"ok\")'",
                "/tmp/cas-script-escape",
            ),
            (
                "node -e 'require(\"fs\").writeFileSync(\"/tmp/cas-script-escape\", \"ok\")'",
                "/tmp/cas-script-escape",
            ),
            (
                "install -D source /tmp/cas-install-escape",
                "/tmp/cas-install-escape",
            ),
        ] {
            let input = bash_input(command, cwd.path());
            assert_eq!(
                factory_write_violation(&input, &None, None, false, Some(cwd.path()))
                    .map(|violation| violation.resolved_path),
                canonicalize_for_containment(std::path::Path::new(expected)),
                "outside script/install targets must remain denied: {command}"
            );
        }
    }

    /// cas-becd (GH #922): the factory hook refused a heredoc'd Node script
    /// that wrote a temp file named `${file}.${process.pid}.tmp`: the JS
    /// template literal reached the shell expander and read as an unresolved
    /// shell variable outside the worktree. Replays that shape (heredoc and
    /// `-e`) and keeps literal-prefix escapes denied.
    #[test]
    fn template_literal_script_targets_resolve_as_javascript_cas_becd() {
        let cwd = tempfile::tempdir().expect("worktree");
        for command in [
            "node - <<'EOF'\nconst fs = require('fs');\nfor (const file of process.argv.slice(2)) {\n  const tmp = `${file}.${process.pid}.tmp`;\n  fs.writeFileSync(`${file}.${process.pid}.tmp`, fs.readFileSync(file));\n  fs.renameSync(tmp, file);\n}\nEOF",
            "node -e 'require(\"fs\").writeFileSync(`${file}.tmp`, \"x\")'",
            "node - <<'EOF'\nconst fs = require('fs');\nfs.writeFileSync(`out/${name}.json`, '{}');\nEOF",
            "node - <<'EOF'\nconst dir = \"generated\";\nrequire('fs').writeFileSync(`${dir}/${name}.ts`, '');\nEOF",
        ] {
            let input = bash_input(command, cwd.path());
            assert_eq!(
                factory_write_violation(&input, &None, None, false, Some(cwd.path())),
                None,
                "an in-worktree template-literal write must be allowed: {command}"
            );
        }

        for (command, expected) in [
            (
                "node - <<'EOF'\nrequire('fs').writeFileSync(`/tmp/cas-template-escape-${name}`, 'x');\nEOF",
                "/tmp/cas-template-escape-_",
            ),
            (
                "node - <<'EOF'\nconst dir = \"/tmp\";\nrequire('fs').writeFileSync(`${dir}/cas-template-escape`, 'x');\nEOF",
                "/tmp/cas-template-escape",
            ),
        ] {
            let input = bash_input(command, cwd.path());
            let violation = factory_write_violation(&input, &None, None, false, Some(cwd.path()))
                .unwrap_or_else(|| panic!("an outside template-literal write must stay denied: {command}"));
            assert_eq!(violation.evaluated_path, expected, "{command}");
            assert_eq!(violation.matched_rule, "none", "resolved as a path, not a shell variable");
        }

        let no_assignments = std::collections::HashMap::new();
        assert_eq!(script_template_target("${file}.${process.pid}.tmp", &no_assignments), None);
        assert_eq!(
            script_template_target("out/${a}-${b}.json", &no_assignments).as_deref(),
            Some("out/_-_.json")
        );
        assert_eq!(script_template_target("out/${unterminated", &no_assignments), None);
    }

    #[test]
    fn sed_substitution_expression_is_not_a_write_target() {
        assert!(bash_write_targets("sed 's/a/b/' input.txt").is_empty());
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
                canonicalize_for_containment(&home.path().join(expected)),
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
            canonicalize_for_containment(&scratch.parent().unwrap().join("escape.log")),
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
            canonicalize_for_containment(&outside),
            "configured scratch enforcement must deny unrelated host paths"
        );
    }

    #[test]
    fn sanctioned_static_prefix_allows_dynamic_creation_suffixes_cas_6ada() {
        let _env = TestEnvGuard::temp_home();
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("worktree");
        let artifacts = root.path().join("artifacts");
        let scratch = root.path().join("scratch");
        for path in [&worktree, &artifacts, &scratch] { std::fs::create_dir_all(path).unwrap(); }
        let artifacts_root = Some(artifacts.display().to_string());
        let scratch_root = scratch.display().to_string();
        for base in [&worktree, &artifacts, &scratch] {
            for command in [
                format!("mkdir -p {}/run-$$; printf proof > {}/run-$$/all.jsonl", base.display(), base.display()),
                format!("printf proof > {}/run-${{RUN_ID}}/all.jsonl", base.display()),
                format!("S={}/violet-logs-$$; mkdir -p \"$S\"; printf proof > \"$S/all.jsonl\"", base.display()),
                format!("R={}; S=$R/violet-logs-$RUN_ID; mkdir -p $S; printf proof > $S/all.jsonl", base.display()),
            ] {
                assert_eq!(factory_write_violation(&bash_input(&command, &worktree), &artifacts_root,
                    Some(&scratch_root), true, Some(&worktree)), None, "sanctioned static prefix: {command}");
            }
        }
    }

    #[test]
    fn variable_root_denial_names_the_unexpanded_variable_cas_6ada() {
        let _env = TestEnvGuard::temp_home();
        let worktree = tempfile::tempdir().unwrap();
        let input = bash_input("printf proof > $UNKNOWN_ROOT/all.jsonl", worktree.path());
        let violation = factory_write_violation(&input, &None, None, false, Some(worktree.path())).unwrap();
        let reason = factory_workspace_contract_denial(&input, &violation, None, None, Some(worktree.path()));
        assert!(reason.contains("unexpanded shell variable `$UNKNOWN_ROOT`"), "{reason}");
    }

    #[test]
    fn dynamic_creation_prefix_keeps_boundary_traversal_and_delete_guards_cas_6ada() {
        use std::os::unix::fs::symlink;
        let _env = TestEnvGuard::temp_home();
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("worktree");
        let scratch = root.path().join("scratch");
        let outside = root.path().join("outside");
        for path in [&worktree, &scratch, &outside] { std::fs::create_dir_all(path).unwrap(); }
        symlink(&outside, scratch.join("escape")).unwrap();
        let scratch_root = scratch.display().to_string();
        for command in [
            format!("printf proof > {}$ROOT_SUFFIX/file", scratch.display()),
            format!("printf proof > {}-lookalike/$RUN/file", scratch.display()),
            format!("printf proof > {}/run-$RUN/../../outside/file", scratch.display()),
            format!("printf proof > {}/escape/run-$RUN/file", scratch.display()),
            format!("rm -rf {}/run-$$", scratch.display()),
            "printf proof > ${UNKNOWN_ROOT}/file".to_string(),
        ] {
            assert!(factory_write_violation(&bash_input(&command, &worktree), &None,
                Some(&scratch_root), false, Some(&worktree)).is_some(), "must retain guard: {command}");
        }
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
            canonicalize_for_containment(&std::env::temp_dir().join("cas-3bd6-tool-temp.log")),
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
            canonicalize_for_containment(&outside.join("escape.txt")),
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
    fn codex_exec_redirection_admission_uses_workdir_or_names_its_omission_cas_0224() {
        let mut env = TestEnvGuard::temp_home();
        let project = env.home().join("project");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let worktree = cas_root.join("worktrees/worker-0224");
        let child = worktree.join("hub-web");
        let outside = cas_root.join("worktrees/outside.log");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(worktree.join("target")).unwrap();
        env.set("CAS_HOOK_HARNESS", "codex");
        env.set("CAS_AGENT_ROLE", "worker");
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_CLONE_PATH", &worktree);

        let mut mismatches = Vec::new();
        for (command, workdir, expected, missing_workdir) in [
            ("printf x > ../target/check.log 2>&1".to_string(), Some(child.clone()), "allow", false),
            ("printf x > ../target/check.log 2>&1".to_string(), Some("hub-web".into()), "allow", false),
            ("printf x > target/check.log 2>&1".to_string(), None, "allow", false),
            ("printf x > ../target/check.log 2>&1".to_string(), None, "deny", true),
            ("printf x > ../../outside.log".to_string(), Some(child), "deny", false),
            (format!("printf x > '{}'", worktree.join("target/check.log").display()), None, "allow", false),
            (format!("printf x > '{}'", outside.display()), None, "deny", false),
            (format!(r#"printf x > '{}'; sh -c "cd '{}' && printf y > outside.log""#,
                outside.display(), outside.parent().unwrap().display()), None, "deny", false),
        ] {
            // Codex 0.163 projects exec_command as Bash/command with turn cwd.
            // The optional workdir models the upstream projection once fixed.
            let mut input = bash_input(&command, &worktree);
            if let Some(workdir) = workdir {
                input.tool_input.as_mut().unwrap()["workdir"] = serde_json::json!(workdir);
            }
            let value = serde_json::to_value(handle_pre_tool_use(&input, Some(&cas_root)).unwrap()).unwrap();
            let specific = &value["hookSpecificOutput"];
            let reason = specific["permissionDecisionReason"].as_str().unwrap_or("");
            let decision = if value == serde_json::json!({}) {
                "allow" // Codex accepts an empty response for no-op admission.
            } else {
                specific["permissionDecision"].as_str().unwrap_or("")
            };
            if decision != expected {
                mismatches.push(format!("{command}, {:?}: expected {expected}, got {value}", input.tool_input));
            }
            if missing_workdir {
                if !["../target/check.log", "hook input omits workdir", "absolute in-worktree path"]
                    .iter().all(|part| reason.contains(part))
                    || reason.contains(&cas_root.join("worktrees/target/check.log").display().to_string())
                    || reason.contains("outside")
                {
                    mismatches.push(format!("missing-workdir refusal must explain uncertainty without a guessed escape: {value}"));
                }
            } else if expected == "deny" && !reason.contains(&outside.display().to_string()) {
                mismatches.push(format!("real escape must name its destination: {value}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
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
            canonicalize_for_containment(&target),
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
            canonicalize_for_containment(&target),
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
    // cas-8563b: spawn_workers lives on `factory`; the coordination alias
    // still runs it for one release, so both names are gated.
    let factory_tool = format!("{tool_prefix}factory");
    (tool_name == task_tool && action == Some("create"))
        || ((tool_name == coordination_tool || tool_name == factory_tool)
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

/// Deny the host harness transport for factory supervisors (GH #793).
///
/// A `SendMessage` hook result of `allow` does not replace Claude Code's
/// native delivery: the host runs that transport after the hook returns. A
/// supervisor assignment therefore has to fail closed here and point at the
/// durable CAS commands instead.
fn deny_factory_supervisor_send_message() -> HookOutput {
    let prefix = crate::harness_policy::own_tool_prefix();
    HookOutput::with_pre_tool_permission(
        "deny",
        &format!(
            "🚫 SendMessage is disabled for factory supervisors.\n\n\
             Use {prefix}coordination action=message target=<worker> summary=\"<brief summary>\" message=\"<message>\"\n\
             For task assignments, use {prefix}task action=update id=<task-id> assignee=<worker>.\n\n\
             The CAS coordination path is the only durable supervisor-to-worker transport."
        ),
    )
}

/// Deny the host harness transport for factory workers.
fn deny_factory_worker_send_message() -> HookOutput {
    let prefix = crate::harness_policy::own_tool_prefix();
    HookOutput::with_pre_tool_permission(
        "deny",
        &format!(
            "🚫 SendMessage is disabled for factory workers.\n\n\
             Use {prefix}coordination action=message target=<peer> summary=\"<brief summary>\" message=\"<message>\".\n\n\
             This sends the message through the durable factory coordination path."
        ),
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

    /// cas-6db4: a push_branch task in a local_merge session (for example
    /// one targeting protected main) may push its factory branch, because its
    /// close gate requires the branch on origin. A local_merge task still may
    /// not, and a worker with no task keeps the session's mode.
    #[test]
    fn push_guard_follows_the_active_tasks_delivery_mode_cas_6db4() {
        use cas_types::DeliveryMode::{LocalMerge, PushBranch};
        assert_eq!(
            effective_worker_delivery_mode(LocalMerge, &[PushBranch]),
            PushBranch,
            "the task's push_branch wins over a local_merge session"
        );
        assert_eq!(
            effective_worker_delivery_mode(PushBranch, &[LocalMerge]),
            LocalMerge,
            "a local_merge task is not pushed even in a push_branch session"
        );
        assert_eq!(effective_worker_delivery_mode(LocalMerge, &[]), LocalMerge);
        assert_eq!(effective_worker_delivery_mode(PushBranch, &[]), PushBranch);
        assert_eq!(
            effective_worker_delivery_mode(LocalMerge, &[LocalMerge, PushBranch]),
            PushBranch
        );
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

    /// cas-93e8: history-changing Git commands must all enter the worker
    /// worktree guard, including commands that were previously invisible to
    /// the commit/merge/push matcher.
    #[test]
    fn history_changing_git_commands_are_guarded_cas_93e8() {
        for command in [
            "git commit -m work",
            "git merge main",
            "git rebase main",
            "git cherry-pick --continue",
            "git reset --hard HEAD~1",
            "git checkout main",
            "git add -A",
        ] {
            assert!(
                looks_like_git_write_op(command),
                "history-changing command must be recognized: {command}"
            );
        }
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
            ("CAS_AGENT_NAME", Some("iso{a,b}!e")),
        ]);

        let msg = check_worker_git_commit_scope(&p).expect("isolated worker on main is denied");
        assert!(
            msg.contains("git switch 'factory/iso{a,b}!e'"),
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
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_CLONE_PATH", Some(&ps)), ("CAS_AGENT_NAME", Some("worker{a,b}!e"))]);

        let result = check_worker_git_commit_scope(&ps);
        assert!(
            result.is_some(),
            "detached HEAD must be denied, got: {result:?}"
        );
        let msg = result.unwrap();
        // pin: detached-head switch advice quotes the worker ref.
        assert!(msg.contains("git switch 'factory/worker{a,b}!e'"), "{msg}");
        assert!(msg.contains("WORKER COMMIT GUARD"));
        assert!(msg.contains("detached"));
    }

    #[test]
    fn paused_rebase_resolves_original_worker_branch_for_continuations() {
        let tmp = make_git_repo();
        let p = tmp.path();
        let branch = "factory/rebase-worker";

        let switch = std::process::Command::new("git")
            .args(["switch", "-c", branch])
            .current_dir(p)
            .output()
            .unwrap();
        assert!(switch.status.success());
        std::fs::write(p.join("f.txt"), "worker change").unwrap();
        for args in [vec!["add", "f.txt"], vec!["commit", "-m", "worker"]] {
            let output = std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
        }

        let switch = std::process::Command::new("git")
            .args(["switch", "main"])
            .current_dir(p)
            .output()
            .unwrap();
        assert!(switch.status.success());
        std::fs::write(p.join("f.txt"), "main change").unwrap();
        for args in [vec!["add", "f.txt"], vec!["commit", "-m", "main"]] {
            let output = std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
        }

        let switch = std::process::Command::new("git")
            .args(["switch", branch])
            .current_dir(p)
            .output()
            .unwrap();
        assert!(switch.status.success());
        let rebase = std::process::Command::new("git")
            .args(["rebase", "main"])
            .current_dir(p)
            .output()
            .unwrap();
        assert!(
            !rebase.status.success(),
            "fixture rebase must pause on a conflict: {rebase:?}"
        );

        let detached = get_branch_at_git_target(&GitCommandTarget::from_cwd(&p.to_string_lossy()));
        assert!(
            detached.is_none(),
            "rebase should detach HEAD: {detached:?}"
        );
        let rebase_branch =
            get_branch_at_rebase_git_target(&GitCommandTarget::from_cwd(&p.to_string_lossy()));
        assert_eq!(rebase_branch.as_deref(), Some(branch));

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(&ps)),
            ("CAS_AGENT_NAME", Some("rebase-worker")),
        ]);
        for command in ["git rebase --continue", "git cherry-pick --continue"] {
            assert!(
                check_worker_git_commit_scope_for_command(&ps, command).is_none(),
                "continuation should use the branch recorded by rebase metadata: {command}"
            );
        }
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

    /// cas-93e8: a `cd` in the same shell command changes the repository
    /// targeted by every following Git history mutation. The hook cwd alone
    /// is not sufficient because a worker can leave its worktree with
    /// `cd /primary/checkout && git commit`.
    #[test]
    fn explicit_cd_outside_worker_worktree_is_denied_for_history_writes_cas_93e8() {
        let parent = tempfile::tempdir().unwrap();
        let worktree = parent.path().join("worker");
        let outside = parent.path().join("primary");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let worktree_path = worktree.to_string_lossy().to_string();
        let outside_path = outside.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_CLONE_PATH", Some(worktree_path.as_str())),
            ("CAS_AGENT_NAME", Some("worker")),
        ]);

        for operation in [
            "git commit -m work",
            "git merge main",
            "git rebase main",
            "git reset --hard HEAD~1",
            "git checkout main",
            "git add -A",
        ] {
            let command = format!("cd '{outside_path}' && {operation}");
            let message = check_worker_git_commit_scope_for_command(&worktree_path, &command)
                .unwrap_or_else(|| panic!("outside-worktree operation must be denied: {command}"));
            assert!(
                message.contains("outside your assigned worktree"),
                "refusal must identify the workspace boundary: {message}"
            );
        }
    }

    /// cas-93e8: an outside-worktree history mutation is denied through the
    /// real PreToolUse handler and leaves an auditable workspace-contract
    /// rejection event.
    #[test]
    fn outside_worktree_history_write_is_logged_by_pre_tool_cas_93e8() {
        let cas_root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let worktree = parent.path().join("worker");
        let outside = parent.path().join("primary");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let worktree_path = worktree.to_string_lossy().to_string();
        let outside_path = outside.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(worktree_path.as_str())),
            ("CAS_AGENT_NAME", Some("worker{a,b}!e")),
            ("CAS_FACTORY_SESSION", Some("cas-93e8-pre-tool")),
        ]);

        let input = hook_git_write_input(
            &worktree_path,
            &format!("cd '{outside_path}' && git reset --hard HEAD~1"),
        );
        let output = handle_pre_tool_use(&input, Some(cas_root.path())).expect("handler ok");
        let value = serde_json::to_value(output).unwrap();
        // pin: outside-worktree remedy quotes the intended worker ref.
        assert!(value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("git switch 'factory/worker{a,b}!e'"), "{value}");
        assert_eq!(
            value["hookSpecificOutput"]["permissionDecision"], "deny",
            "outside-worktree history mutation must be denied: {value}"
        );

        let log_path = cas_root.path().join(format!(
            "logs/factory-session-{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        ));
        let log = std::fs::read_to_string(log_path).expect("git workspace refusal log");
        assert!(
            log.contains("workspace_contract_git_rejection")
                && log.contains(&outside_path)
                && log.contains("reset --hard"),
            "git workspace refusal must be durable and identify the operation/path: {log}"
        );
    }

    // ── Integration: handle_pre_tool_use for Bash git commit ─────────────

    fn add_linked_worktree(repo: &std::path::Path, worktree: &std::path::Path, branch: &str) {
        let output = std::process::Command::new("git")
            .args(["worktree", "add", "-b", branch])
            .arg(worktree)
            .current_dir(repo)
            .output()
            .expect("create linked worktree");
        assert!(
            output.status.success(),
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn hook_git_write_input(cwd: &str, command: &str) -> crate::hooks::handlers::HookInput {
        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = cwd.to_string();
        input.tool_input = Some(serde_json::json!({"command": command}));
        input
    }

    #[test]
    fn pre_tool_allows_git_c_commit_in_linked_factory_worktree() {
        let repo = make_git_repo();
        let linked = tempfile::tempdir().expect("linked worktree path");
        add_linked_worktree(repo.path(), linked.path(), "factory/linked-worker");
        let repo_path = repo.path().to_string_lossy().to_string();
        let linked_path = linked.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("linked-worker")),
        ]);

        let artifacts = tempfile::tempdir().unwrap();
        let cwd = artifacts.path().display().to_string();
        let ledger = format!("cat > LEDGER.md <<'EOF'\ngit config\ncommit failed; commits normally\n$(git -C '{repo_path}' commit -m inert)\nEOF");
        let mut mismatches = Vec::new();
        for (command, denied) in [
            (ledger.to_string(), false),
            ("printf '%s' git commit".to_string(), false),
            (format!("git -C '{repo_path}' log --grep commit"), false),
            (format!("git -C '{linked_path}' commit -m work"), false),
            (format!("{ledger}\ngit -C '{repo_path}' commit -m work"), true),
            (format!("cat > LEDGER.md <<EOF\n$(git -C '{repo_path}' commit -m work)\nEOF"), true),
        ] {
            let input = hook_git_write_input(&cwd, &command);
            let out = handle_pre_tool_use(&input, None).expect("handler ok");
            let val = serde_json::to_value(&out).unwrap();
            if (val["hookSpecificOutput"]["permissionDecision"] == "deny") != denied {
                mismatches.push(format!("{command}: expected denied={denied}: {val}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn pre_tool_allows_git_dir_and_work_tree_commit_in_linked_factory_worktree() {
        let repo = make_git_repo();
        let linked = tempfile::tempdir().expect("linked worktree path");
        add_linked_worktree(repo.path(), linked.path(), "factory/explicit-target");
        let repo_path = repo.path().to_string_lossy().to_string();
        let linked_path = linked.path().to_string_lossy().to_string();
        let git_dir = std::process::Command::new("git")
            .args(["-C", &linked_path, "rev-parse", "--git-dir"])
            .output()
            .expect("resolve linked worktree git dir");
        assert!(git_dir.status.success());
        let git_dir = String::from_utf8_lossy(&git_dir.stdout).trim().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("explicit-target")),
        ]);

        let input = hook_git_write_input(
            &repo_path,
            &format!("git --git-dir '{git_dir}' --work-tree '{linked_path}' commit -m work"),
        );
        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(
            decision,
            Some("deny"),
            "explicit linked worktree target must be honored: {val}"
        );
    }

    #[test]
    fn pre_tool_allows_commit_when_hook_cwd_is_linked_factory_worktree() {
        let repo = make_git_repo();
        let linked = tempfile::tempdir().expect("linked worktree path");
        add_linked_worktree(repo.path(), linked.path(), "factory/cwd-worker");
        let linked_path = linked.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("cwd-worker")),
        ]);

        let input = hook_git_write_input(&linked_path, "git commit -m work");
        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_ne!(
            decision,
            Some("deny"),
            "linked cwd branch must be honored: {val}"
        );
    }

    #[test]
    fn pre_tool_still_denies_shared_checkout_commit_on_staging() {
        let repo = make_git_repo();
        let output = std::process::Command::new("git")
            .args(["switch", "-c", "staging"])
            .current_dir(repo.path())
            .output()
            .expect("switch to staging");
        assert!(output.status.success());
        let repo_path = repo.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", None),
            ("CAS_AGENT_NAME", Some("shared-worker")),
        ]);

        let input = hook_git_write_input(&repo_path, "git commit -m work");
        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let decision = val
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|v| v.as_str());
        assert_eq!(
            decision,
            Some("deny"),
            "shared protected checkout must remain denied: {val}"
        );
    }

    #[test]
    fn pre_tool_denies_git_commit_on_protected_branch() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_MODE", Some("1")),
            ("CAS_CLONE_PATH", Some(&p)),
        ]);

        let mut mismatches = Vec::new();
        for command in [
            "git commit -m 'oops'",
            r"find . -exec git commit -m x \;",
            r"find . -execdir git commit -m x \;",
            "command git commit -m x",
            "sudo git commit -m x",
            "exec git commit",
            "builtin git -C . commit -m x",
            "xargs -I{} git commit",
        ] {
            let input = hook_git_write_input(&p, command);
            let out = handle_pre_tool_use(&input, None).expect("handler ok");
            let val = serde_json::to_value(&out).unwrap();
            if val["hookSpecificOutput"]["permissionDecision"] != "deny" {
                mismatches.push(format!("{command}: expected deny: {val}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
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
            ("CAS_AGENT_NAME", Some("worker{a,b}!e")),
            ("CAS_CLONE_PATH", None), // no isolated worktree
        ]);

        let mut input = crate::hooks::handlers::HookInput::default();
        input.hook_event_name = "PreToolUse".to_string();
        input.tool_name = Some("Bash".to_string());
        input.cwd = p.clone();
        input.tool_input = Some(serde_json::json!({"command": "git commit -m 'oops on main'"}));

        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        let val = serde_json::to_value(&out).unwrap();
        let reason = val["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        // pin: actual handler worktree/create/switch/push advice preserves shell argv.
        for command in ["git worktree add '../factory-worker{a,b}!e' -b 'factory/worker{a,b}!e'", "git switch -c 'factory/worker{a,b}!e'", "git push -u origin 'factory/worker{a,b}!e'"] {
            assert!(reason.contains(command), "{reason}");
        }
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
