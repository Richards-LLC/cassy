use crate::hooks::handlers::session_budget::SessionContextAssembler;
use crate::hooks::handlers::*;

fn registered_role_mismatch_banner(
    configured_role: Option<AgentRole>,
    registered_role: Option<AgentRole>,
) -> Option<String> {
    let (configured, registered) = configured_role.zip(registered_role)?;
    (configured != registered).then(|| {
        format!(
            "\u{26a0}\u{fe0f} Cassy AGENT ROLE MISMATCH: `CAS_AGENT_ROLE={configured}` but the durable agent row was registered as `{registered}` at session start. Cassy attempted to repair the row; run `mcp__cs__coordination action=whoami` and `cas doctor` before assigning or closing factory work."
        )
    })
}

pub fn handle_session_start(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let timer = TraceTimer::new();

    // Computed inside the inner `cas_root` block and applied to the output
    // after context building (cas-ae09). None for non-factory sessions.
    let mut factory_session_title: Option<String> = None;
    let mut registration_role_warning: Option<String> = None;

    // Record session start for analytics and register agent
    if let Some(cas_root) = cas_root {
        let mut stores = HookStores::new(cas_root);

        if let Some(sqlite_store) = stores.sqlite() {
            let session = Session::new(
                input.session_id.clone(),
                input.cwd.clone(),
                input.permission_mode.clone(),
            );
            if sqlite_store.start_session(&session).is_ok() {
                eprintln!(
                    "cas: Session {} started",
                    &input.session_id[..8.min(input.session_id.len())]
                );
            }
        }

        // Notify daemon via socket for instant agent registration
        // Daemon tracks PID → session mapping in memory (no files needed)
        // Pass agent_name and agent_role from this process's env (set by factory mode)
        use crate::agent_id::get_cc_pid_for_hook;
        let cc_pid = get_cc_pid_for_hook();
        let agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
        let clone_path = std::env::var("CAS_CLONE_PATH").ok();

        // Capture disagreement before re-registration repairs it so a
        // supervisor does not begin factory work with a silently broken row.
        let configured_role = crate::mcp::daemon::parse_agent_role_hint(agent_role.as_deref());
        let registered_role = stores
            .agents()
            .and_then(|store| store.get(&input.session_id).ok())
            .map(|agent| agent.role);
        registration_role_warning =
            registered_role_mismatch_banner(configured_role, registered_role);

        // Helper to register agent directly in database
        let register_directly = |stores: &mut HookStores| {
            if let Some(agent_store) = stores.agents() {
                match crate::mcp::daemon::register_session_start_agent(
                    agent_store.as_ref(),
                    &input.session_id,
                    agent_name.as_deref(),
                    agent_role.as_deref(),
                    cc_pid,
                    clone_path.as_deref(),
                ) {
                    Ok((_agent, reused)) => {
                        let verb = if reused { "Refreshed" } else { "Registered" };
                        eprintln!(
                            "cas: {verb} agent directly (pid: {cc_pid}, role: {agent_role:?})"
                        );
                    }
                    Err(reg_err) => eprintln!("cas: Failed to register agent: {reg_err}"),
                }
            }
        };

        {
            use crate::mcp::socket::{DaemonEvent, send_event};
            let event = DaemonEvent::SessionStart {
                session_id: input.session_id.clone(),
                agent_name: agent_name.clone(),
                agent_role: agent_role.clone(),
                cc_pid,
                clone_path: clone_path.clone(),
            };
            match send_event(cas_root, &event) {
                Ok(_) => eprintln!(
                    "cas: Notified daemon of session start (pid: {}, role: {:?})",
                    cc_pid,
                    std::env::var("CAS_AGENT_ROLE").ok()
                ),
                Err(e) => {
                    // Daemon socket not available - register directly in database as fallback
                    eprintln!("cas: Daemon not available ({e}), registering directly");
                    register_directly(&mut stores);
                }
            }
        }

        // Write OTEL context for telemetry correlation
        let project_id = crate::cloud::resolve_canonical_id(cas_root);
        let project_path = cas_root.parent().map(|p| p.to_string_lossy().to_string());

        // Check for active task (reuses cached task store)
        // Fetch the full list so downstream consumers (OTEL, sessionTitle) share
        // the same query without redundant store access.
        let active_tasks: Vec<Task> = stores
            .tasks()
            .and_then(|ts| ts.list(Some(TaskStatus::InProgress)).ok())
            .unwrap_or_default();
        let active_task_id = active_tasks.first().map(|t| t.id.clone());

        // Compute factory session title now while active_tasks is in scope (cas-ae09).
        let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
        factory_session_title = compute_session_title(&role, &active_tasks);

        let otel_ctx = OtelContext::new(input.session_id.clone())
            .with_project_id(project_id)
            .with_project_path(project_path)
            .with_permission_mode(input.permission_mode.clone())
            .with_task_id(active_task_id);

        if let Err(e) = otel_ctx.write(cas_root) {
            eprintln!("cas: Warning: Failed to write OTEL context: {e}");
        }

        // Cleanup orphaned tasks from crashed/interrupted previous sessions
        let reopened = cleanup_orphaned_tasks(cas_root);
        if reopened > 0 {
            eprintln!("cas: Reopened {reopened} orphaned task(s) from previous session");
        }
    }

    // Check if we're in plan mode
    let is_plan_mode = input.permission_mode.as_deref() == Some("plan");

    // Load config to check AI context setting
    let config = cas_root
        .map(|r| Config::load_with_host_staging_defaults(r).unwrap_or_default())
        .unwrap_or_default();

    // Need cas_root for context building
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };
    let context_limit = config.context_limit();

    // Build appropriate context based on mode
    let context = if is_plan_mode {
        eprintln!("cas: Plan mode detected, building planning context");
        build_plan_context(input, 10, cas_root)?
    } else if config.hooks.as_ref().map(|h| h.ai_context).unwrap_or(false) {
        // Try AI-powered context selection
        eprintln!("cas: Using AI-assisted context prioritization");
        match build_context_ai(input, context_limit, cas_root) {
            Ok(ctx) => ctx,
            Err(e) => {
                // Check if fallback is enabled
                let ai_fallback = config.hooks.as_ref().map(|h| h.ai_fallback).unwrap_or(true);
                if ai_fallback {
                    eprintln!("cas: AI context failed ({e}), falling back to standard");
                    build_context(input, context_limit, cas_root)?
                } else {
                    eprintln!("cas: AI context failed: {e}");
                    return Err(e);
                }
            }
        }
    } else {
        build_context(input, context_limit, cas_root)?
    };

    // Inject codemap + project-overview freshness warnings.
    //
    // High-severity warnings (missing / significantly stale / any staleness for
    // supervisors) are **prepended** so they land inside the truncated
    // SessionStart preview window the agent skims first. Info-level warnings
    // are appended.
    //
    // Codemap runs first and wins the top slot when both would prepend;
    // project-overview always appends to preserve codemap's ordering dominance
    // when both are high-severity.
    let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
    let is_supervisor = agent_role.as_deref() == Some("supervisor");

    // cas-b114: everything from here on is assembled through the aggregate
    // size budget (see `session_budget`). Variable-length warning sections
    // register a compact summary alongside their full rendering so an
    // over-budget payload degrades to counts + remediation command instead of
    // being silently truncated by the harness at ~10KB. The base context
    // (role guidance + Cassy header + memories/tasks) is protected.
    let mut assembler = SessionContextAssembler::new(context);

    if let Some(warning) = registration_role_warning {
        assembler.append_protected(warning);
    }

    #[cfg(feature = "mcp-proxy")]
    if is_supervisor
        && let Some(inbound) = crate::mcp::viktor_watch::surface_inbound_at_session_start(
            cas_root,
            std::env::var("CAS_FACTORY_SESSION").ok().as_deref(),
        )
    {
        assembler.prepend_protected(inbound);
    }

    #[cfg(feature = "mcp-proxy")]
    if let Some(warning) = crate::mcp::viktor_watch::session_start_warning(cas_root) {
        assembler.prepend_degradable(warning.clone(), warning);
    }

    // Planning is a shared write surface: two live supervisors can otherwise
    // decompose the same epic before either sees the other's task set. Put
    // this at the protected top of supervisor context so it survives the
    // SessionStart preview/budget compaction path.
    if is_supervisor {
        if let Some(banner) =
            active_peer_supervisor_banner(cas_root, std::env::var("CAS_AGENT_NAME").ok().as_deref())
        {
            assembler.prepend_degradable(banner.clone(), banner);
        }
    }

    // cas-cd54: ambient recall is a degradable, independently hard-bounded
    // evidence segment. It stays below immutable role guidance and never
    // competes with safety banners for protected SessionStart bytes.
    if let Some(packet) =
        crate::ambient_recall::build_ambient_recall_context(input, cas_root, None, true)
    {
        assembler.append_degradable(packet.full, packet.compact);
    }

    if let Some(staleness) =
        crate::hooks::handlers::handlers_events::check_codemap_freshness(cas_root)
    {
        let full = staleness.format_injection(is_supervisor);
        let compact = staleness.format_injection_compact(is_supervisor);
        if staleness.is_high_severity(is_supervisor) {
            assembler.prepend_degradable(full, compact);
        } else {
            assembler.append_degradable(full, compact);
        }
    }

    if let Some(repo_root) = cas_root.parent() {
        match crate::hooks::handlers::handlers_events::project_overview::check_freshness(
            repo_root,
            agent_role.as_deref(),
        ) {
            // Always append so codemap retains the preview top slot when both
            // modules report high severity.
            Ok(Some(staleness)) => {
                assembler.append_protected(staleness.format_injection(is_supervisor))
            }
            Ok(None) => {}
            Err(e) => eprintln!("cas: project-overview freshness check failed: {e}"),
        }
    }

    // Factory session-start hygiene triage (task cas-aeec): for supervisor
    // sessions, append a banner listing uncommitted files in the main
    // worktree with per-file last-touching-task-id attribution. Visibility
    // only — the supervisor decides salvage / commit / discard before
    // spawning workers. Best-effort: git failures, non-supervisor roles,
    // and clean trees all fall through silently.
    //
    // Appended (not prepended) so codemap and project-overview retain the
    // preview top slot they are explicitly engineered to land in (see
    // comments above). The banner is not severity-ranked against those
    // modules, so it sits below them in the supervisor's initial view.
    if is_supervisor {
        if let Some(banner) =
            crate::hooks::handlers::session_hygiene::build_session_start_wip_banner_sized(cas_root)
        {
            assembler.append_degradable(banner.full, banner.compact);
        }
    }

    // cas-0c0a: builtin skill references the last sync refused to update.
    // Surfaced to every role, not just supervisors — the stale files are the
    // worker's own operating guidance, and the `cas update --sync` warning that
    // reports the skip is invisible to unattended/scripted syncs.
    if let Some(banner) =
        crate::hooks::handlers::session_hygiene::build_session_start_stale_reference_banner_sized(
            cas_root,
        )
    {
        assembler.append_degradable(banner.full, banner.compact);
    }

    // cas-20f27: issue-filing detectors. Both are surfaced to every role — a
    // worker is as likely as a supervisor to be the one who staged the report,
    // and the filing directive is a standing one for both. Appended below
    // codemap/project-overview so those keep the preview top slot.
    //
    // (1) Staged BUG-*/FEATURE-* reports that were never pushed to GitHub.
    if let Some(banner) =
        crate::hooks::handlers::session_hygiene::build_session_start_unfiled_reports_banner_sized(
            cas_root,
        )
    {
        assembler.append_degradable(banner.full, banner.compact);
    }

    // (2) No issue target configured in a project that stages requests. Never
    // suggests a value — guessing from `origin` routes Cassy bugs into a
    // consumer's own tracker (filing-cas-bugs.md).
    if let Some(banner) =
        crate::hooks::handlers::session_hygiene::build_session_start_issues_target_banner_sized(
            cas_root, &config,
        )
    {
        assembler.append_degradable(banner.full, banner.compact);
    }

    // cas-b7dd (GH #88): leftovers from dead sessions — orphan processes still
    // running in worktrees and stale server registrations. Surfaced here
    // because a new session otherwise inherits them invisibly and meets them
    // as an EADDRINUSE failure with no hint that the squatter is Cassy's own.
    // Visibility only: this banner never signals anything.
    if is_supervisor {
        if let Some(banner) =
            crate::hooks::handlers::session_hygiene::build_session_start_orphan_banner_sized(
                cas_root,
            )
        {
            assembler.append_degradable(banner.full, banner.compact);
        }
    }

    // Read-only GitHub issue triage (cas-ce3d). Supervisors need the intake
    // signal before assigning work; workers do not, and should not pay its
    // latency or context cost. The helper is fully best-effort: unset config,
    // cache/query/parse failures, and a hard subprocess timeout all emit
    // nothing and cannot fail SessionStart.
    if is_supervisor {
        if let Some(banner) = crate::hooks::handlers::issue_triage::build_session_start_banner_sized(
            cas_root, &config,
        ) {
            assembler.append_degradable(banner.full, banner.compact);
        }
    }

    // Phase 3 / cas-3efe: opt-in integrations staleness banner. Default
    // off — only fires when `[integrations] session_start_warn = true` in
    // .cas/config.toml *and* at least one platform reports a `Stale` ID.
    // Appended last so it sits below codemap / project-overview / WIP.
    // Reuses the already-loaded `config` from earlier in this handler.
    if let Some(banner) = build_integrations_session_start_banner(cas_root, &config) {
        assembler.append_protected(banner);
    }

    // Host-scoped staging convention for large generated artifacts. Appended
    // near the end with other runtime banners so immutable role guidance stays
    // budget-stable, while worker worktree assertions can still prepend above
    // it when they detect a more urgent safety issue.
    if let Some(banner) = build_large_artifact_staging_banner(&config) {
        assembler.append_protected(banner);
    }

    // Render under the aggregate budget before the worktree assertion, which
    // must stay verbatim at the very top of whatever survives.
    let context = assembler.render();

    // ========================================================================
    // WORKER WORKTREE ASSERTION (cas-bea2 LAYER 3)
    //
    // For isolated factory workers: verify the session cwd matches the
    // assigned worktree (CAS_CLONE_PATH) and HEAD is on a factory/<name>
    // branch (allowlist — detached HEAD and non-factory branches all warn).
    // Mismatches are prepended as a loud warning so the worker sees them
    // before any other context. Non-isolated workers and non-factory sessions
    // fall through silently. Best-effort — git failures or absent env vars
    // are treated as "no mismatch".
    // ========================================================================
    let context = build_worker_worktree_assertion(&input.cwd, context);

    let output = if context.is_empty() {
        HookOutput::empty()
    } else {
        HookOutput::with_session_start_context(context.clone())
    };

    // Emit reloadSkills when skill files have changed since this session last
    // loaded them (cas-f9ad). Best-effort: failure to read/write sentinel/marker
    // files is silently ignored so SessionStart never blocks on I/O errors.
    let output = if detect_and_mark_skill_drift(cas_root, &input.session_id) {
        output.with_reload_skills(true)
    } else {
        output
    };

    // Emit sessionTitle for factory sessions so agent dashboard / tmux panes
    // show which worker owns which task at a glance (cas-ae09).
    // Non-factory sessions produce None → field absent → unchanged wire shape.
    let output = match factory_session_title {
        Some(title) => output.with_session_title(title),
        None => output,
    };

    // Record trace if dev mode is enabled
    if let Some(tracer) = DevTracer::get() {
        if tracer.should_trace_hooks() {
            let input_json = serde_json::json!({
                "session_id": input.session_id,
                "cwd": input.cwd,
                "permission_mode": input.permission_mode,
            });
            let output_json = serde_json::json!({
                "has_context": !context.is_empty(),
                "context_length": context.len(),
            });

            let _ = tracer.record_hook(
                "SessionStart",
                &input_json,
                &output_json,
                if context.is_empty() {
                    None
                } else {
                    Some(&context)
                },
                Some(estimate_tokens(&context)),
                timer.elapsed_ms(),
                true,
                None,
            );
        }
    }

    Ok(output)
}

fn active_peer_supervisor_banner(cas_root: &Path, current_name: Option<&str>) -> Option<String> {
    let agents = crate::store::open_agent_store(cas_root)
        .ok()?
        .list(Some(cas_types::AgentStatus::Active))
        .ok()?;
    let peers: Vec<String> = agents
        .into_iter()
        .filter(|agent| agent.role == cas_types::AgentRole::Supervisor)
        .filter(|agent| current_name.is_none_or(|name| agent.name != name))
        .map(|agent| agent.name)
        .collect();
    (!peers.is_empty()).then(|| {
        format!(
            "⚠️ CONCURRENT SUPERVISORS ACTIVE: {}\nPlanning under a shared epic can race. Review existing children before creating tasks; Cassy will require confirmation for recent competing plans and duplicate titles.",
            peers.join(", ")
        )
    })
}

/// Estimate token count (rough approximation: ~4 chars per token)
pub(crate) fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

pub(crate) fn build_large_artifact_staging_banner(config: &Config) -> Option<String> {
    let dir = config.staging.as_ref()?.staging_dir.as_deref()?.trim();

    if dir.is_empty() {
        return None;
    }

    Some(format!(
        "Stage large artifacts (>1GB) in {dir} — /tmp is tmpfs on this host."
    ))
}

#[cfg(test)]
mod large_artifact_staging_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use crate::types::{Agent, AgentRole};

    /// Role environment for a SessionStart test, rooted in a temporary HOME.
    ///
    /// `handle_session_start` builds the full session context, and the Host
    /// Constraints section resolves `~/.cas` from `HOME` rather than from the
    /// project root (`hooks::context::build_host_constraints_section`). Without
    /// a temp HOME these tests opened the developer's real global store — one
    /// of the paths behind the 994 leaked fixture rows in cas-78c8 / GH #156.
    fn staging_env(role: &str) -> TestEnvGuard {
        let mut guard = TestEnvGuard::temp_home();
        guard.set("CAS_AGENT_ROLE", role);
        guard.remove("CAS_CLONE_PATH");
        guard.remove("CAS_AGENT_NAME");
        guard
    }

    fn session_input(cwd: &str) -> HookInput {
        HookInput {
            session_id: "staging-session".to_string(),
            cwd: cwd.to_string(),
            hook_event_name: "SessionStart".to_string(),
            ..HookInput::default()
        }
    }

    fn additional_context(output: HookOutput) -> String {
        serde_json::to_value(output)
            .unwrap()
            .pointer("/hookSpecificOutput/additionalContext")
            .and_then(|value| value.as_str())
            .expect("SessionStart additionalContext")
            .to_string()
    }

    #[test]
    fn staging_banner_is_absent_when_unset() {
        let config = Config::default();
        assert!(build_large_artifact_staging_banner(&config).is_none());
    }

    #[test]
    fn staging_banner_trims_and_mentions_configured_dir() {
        let config = Config {
            staging: Some(crate::config::StagingConfig {
                staging_dir: Some(" /mnt/datacube/staging ".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let banner = build_large_artifact_staging_banner(&config).expect("banner");
        assert_eq!(
            banner,
            "Stage large artifacts (>1GB) in /mnt/datacube/staging — /tmp is tmpfs on this host."
        );
    }

    #[test]
    fn session_start_includes_staging_banner_for_supervisor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[staging]\nlarge_artifact_dir = \"/mnt/datacube/staging\"\n",
        )
        .unwrap();
        let _env = staging_env("supervisor");

        let input = session_input(tmp.path().to_str().unwrap());
        let context = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());

        assert!(context.contains(
            "Stage large artifacts (>1GB) in /mnt/datacube/staging — /tmp is tmpfs on this host."
        ));
    }

    #[test]
    fn session_start_includes_staging_banner_for_worker() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[staging]\nlarge_artifact_dir = \"/mnt/datacube/staging\"\n",
        )
        .unwrap();
        let _env = staging_env("worker");

        let input = session_input(tmp.path().to_str().unwrap());
        let context = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());

        assert!(context.contains(
            "Stage large artifacts (>1GB) in /mnt/datacube/staging — /tmp is tmpfs on this host."
        ));
    }

    #[test]
    fn session_start_honors_configured_context_limit() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[hooks]\ncontext_limit = 3\n",
        )
        .unwrap();

        let store = crate::store::SqliteStore::open(tmp.path()).unwrap();
        store.init().unwrap();
        for index in 0..5 {
            store
                .add(&crate::types::Entry::new(
                    format!("context-limit-{index}"),
                    format!("context limit candidate {index}"),
                ))
                .unwrap();
        }

        let _env = staging_env("worker");
        let input = session_input(tmp.path().to_str().unwrap());
        let context = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());

        assert!(
            context.contains("## Helpful Memories (3 memories"),
            "configured context_limit must cap Helpful Memories at three: {context}"
        );
        assert_eq!(
            context
                .lines()
                .filter(|line| line.contains("context limit candidate"))
                .count(),
            3,
            "configured context_limit must inject exactly three memories: {context}"
        );
    }

    #[test]
    fn supervisor_session_start_warns_and_repairs_registered_role_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_store = crate::store::open_agent_store(tmp.path()).unwrap();
        agent_store.init().unwrap();
        let standard = Agent::new(
            "staging-session".to_string(),
            "supervisor-before-repair".to_string(),
        );
        agent_store.register(&standard).unwrap();

        let mut env = staging_env("supervisor");
        env.set("CAS_AGENT_NAME", "supervisor-after-repair");
        env.set("CAS_FACTORY_SESSION", "factory-role-warning");
        let input = session_input(tmp.path().to_str().unwrap());
        let context = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());

        assert!(
            context.contains("Cassy AGENT ROLE MISMATCH"),
            "session start must expose the env/row disagreement: {context}"
        );
        assert!(context.contains("CAS_AGENT_ROLE=supervisor"));
        assert!(context.contains("registered as `standard`"));
        let repaired = agent_store.get("staging-session").unwrap();
        assert_eq!(repaired.role, AgentRole::Supervisor);
        assert_eq!(repaired.agent_type, crate::types::AgentType::Primary);
    }

    #[test]
    fn supervisor_session_start_warns_about_active_peer_but_not_itself() {
        let with_peer = tempfile::tempdir().unwrap();
        let peer_store = crate::store::open_agent_store(with_peer.path()).unwrap();
        peer_store.init().unwrap();
        let peer = Agent::new_with_role(
            "peer-supervisor-session".to_string(),
            "peer-supervisor".to_string(),
            AgentRole::Supervisor,
        );
        peer_store.register(&peer).unwrap();

        let mut env = staging_env("supervisor");
        env.set("CAS_AGENT_NAME", "current-supervisor");
        let input = session_input(with_peer.path().to_str().unwrap());
        let context =
            additional_context(handle_session_start(&input, Some(with_peer.path())).unwrap());
        assert!(context.contains("CONCURRENT SUPERVISORS ACTIVE: peer-supervisor"));
        assert!(context.contains("Planning under a shared epic can race."));

        let no_peer = tempfile::tempdir().unwrap();
        let input = session_input(no_peer.path().to_str().unwrap());
        let context =
            additional_context(handle_session_start(&input, Some(no_peer.path())).unwrap());
        assert!(
            !context.contains("CONCURRENT SUPERVISORS ACTIVE"),
            "a lone supervisor must not receive a false concurrent-supervisor warning: {context}"
        );
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn supervisor_session_start_surfaces_viktor_inbound_once() {
        let tmp = tempfile::tempdir().unwrap();
        let inbound = cas_store::SqliteViktorInboundStore::open(tmp.path()).unwrap();
        assert!(
            inbound
                .record(
                    "thread-provider-question",
                    "message-provider-question",
                    "Can Cassy answer from SessionStart?",
                )
                .unwrap()
        );
        inbound
            .mark_delivery_error(
                "message-provider-question",
                "no live factory supervisor was registered at discovery time",
            )
            .unwrap();

        let mut env = staging_env("supervisor");
        env.set("CAS_AGENT_NAME", "next-supervisor");
        env.set("CAS_FACTORY_SESSION", "factory-viktor-inbound");
        let input = session_input(tmp.path().to_str().unwrap());
        let first = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());
        assert!(first.contains("Viktor-originated message arrived"));
        assert!(first.contains("thread-provider-question"));
        assert!(first.contains("Can Cassy answer from SessionStart?"));

        let second = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());
        assert!(
            !second.contains("thread-provider-question"),
            "the durable inbound question must be receipted after one SessionStart"
        );
    }

    /// Production-shape regression for cas-066a: the real worker guidance,
    /// Cassy task/memory/search reminder, factory identity, and host/runtime
    /// banners must fit below the 12 KiB harness boundary with enough margin
    /// for representative identifier growth. Optional listings may degrade;
    /// these protected semantics may not.
    #[test]
    fn worker_session_start_protected_context_has_hard_limit_margin() {
        const HARD_LIMIT: usize = 12_288;
        const REQUIRED_MARGIN: usize = 800;

        let tmp = tempfile::tempdir().unwrap();
        let mut env = staging_env("worker");
        env.set(
            "CAS_AGENT_NAME",
            "worker-with-a-representative-long-factory-name",
        );
        env.set(
            "CAS_AGENT_ID",
            "codex-worker-with-a-representative-long-session-identity",
        );
        env.set("CAS_FACTORY_SESSION", "representative-factory-session");

        let input = HookInput {
            session_id: "7d3511aa-9cf5-44d8-921d-0289bd66fe0a".to_string(),
            cwd: tmp.path().to_string_lossy().into_owned(),
            hook_event_name: "SessionStart".to_string(),
            permission_mode: Some("default".to_string()),
            ..HookInput::default()
        };
        let context = additional_context(handle_session_start(&input, Some(tmp.path())).unwrap());

        assert!(
            context.len() <= HARD_LIMIT - REQUIRED_MARGIN,
            "worker SessionStart protected context is {} bytes; require at least \
             {REQUIRED_MARGIN}B margin below the {HARD_LIMIT}B harness limit",
            context.len()
        );

        let prefix = crate::harness_policy::own_tool_prefix();
        for required in [
            "## 📋 CAS Context",
            &format!("`{prefix}task`"),
            &format!("`{prefix}memory`"),
            &format!("`{prefix}search`"),
            "**You**: worker-with-a-representative-long-factory-name (worker)",
            "# Factory Worker",
            "Never self-dispatch",
            "One task at a time",
            "Scope is frozen",
            "Honor non-goals and layer boundaries",
            "Never block the pane",
            "Checkpoint, never compact",
            "verification required",
            "MERGE REQUIRED",
        ] {
            assert!(
                context.contains(required),
                "worker SessionStart lost mandatory protected guidance: {required:?}"
            );
        }
    }
}

// ─── Session title computation (cas-ae09) ─────────────────────────────────

/// Compute the `sessionTitle` string for a factory session.
///
/// Returns `None` for non-factory sessions (empty or unknown role) so the
/// `sessionTitle` field is absent from the SessionStart JSON output, preserving
/// the unchanged wire shape for regular interactive sessions.
///
/// ## Title formats
///
/// | Role       | Condition              | Title                                  |
/// |------------|------------------------|----------------------------------------|
/// | worker     | active in-progress task | `[worker] <task-id> · <title ≤40ch>` |
/// | worker     | no active task          | `[worker] idle`                        |
/// | supervisor | in-progress epic exists | `[supervisor] <epic-id>`              |
/// | supervisor | no in-progress epic     | `[supervisor] factory`                 |
/// | other / "" | —                       | `None`                                 |
pub(crate) fn compute_session_title(agent_role: &str, active_tasks: &[Task]) -> Option<String> {
    match agent_role {
        "worker" => {
            let title = active_tasks.first().map(|t| {
                let preview = truncate_display(&t.title, 40);
                format!("[worker] {} · {}", t.id, preview)
            });
            Some(title.unwrap_or_else(|| "[worker] idle".to_string()))
        }
        "supervisor" => {
            let epic_context = active_tasks
                .iter()
                .find(|t| t.task_type == TaskType::Epic)
                .map(|t| t.id.clone())
                .unwrap_or_else(|| "factory".to_string());
            Some(format!("[supervisor] {epic_context}"))
        }
        _ => None,
    }
}

// ─── Skill drift detection (cas-f9ad) ─────────────────────────────────────

/// Check whether working-tree skill files have changed since *this* session
/// last loaded them. Each session marker stores a content fingerprint of the
/// actual `SKILL.md` paths and bytes under the project harness directories.
///
/// A missing marker is the initial load: record the fingerprint without
/// requesting another reload. A later mismatch means the files served from
/// disk moved mid-session, so update the marker and emit `reloadSkills`.
///
/// This deliberately does not compare HEAD with origin/staging. In a shared
/// checkout another worker can correct a safety-critical skill on disk while
/// the remote relationship stays unchanged; the disk file is authoritative.
/// Read/write failures remain best-effort and never block `SessionStart`.
pub(crate) fn detect_and_mark_skill_drift(cas_root: &Path, session_id: &str) -> bool {
    if session_id.trim().is_empty() {
        return false;
    }

    let marker_path = cas_root.join(format!("session_skills_seen_{session_id}"));
    let Some(fingerprint) = working_tree_skill_fingerprint(cas_root) else {
        return false;
    };
    let marker = std::fs::read_to_string(&marker_path).ok();

    if marker.as_deref() == Some(fingerprint.as_str()) {
        return false;
    }
    let first_load = marker.is_none();
    let _ = std::fs::write(&marker_path, fingerprint.as_bytes());
    !first_load
}

fn working_tree_skill_fingerprint(cas_root: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};

    fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(&path, out)?;
            } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                out.push(path);
            }
        }
        Ok(())
    }

    let project_root = cas_root.parent()?;
    let mut files = Vec::new();
    for relative in [".claude/skills", ".codex/skills", ".grok/skills"] {
        collect(&project_root.join(relative), &mut files).ok()?;
    }
    files.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"cas-working-tree-skills-v1\0");
    for path in files {
        let relative = path.strip_prefix(project_root).ok()?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(std::fs::read(&path).ok()?);
        hasher.update(b"\0");
    }
    Some(format!("sha256:{:x}", hasher.finalize()))
}

/// Build the opt-in Phase 3 (cas-3efe) integrations banner.
///
/// Returns `None` unless **all three** conditions hold:
/// 1. `config.integrations.session_start_warn == true` (project-level
///    `.cas/config.toml`; the spec scopes the flag to project config).
/// 2. The repo root resolves (cas_root parent).
/// 3. At least one platform's [`crate::cli::integrate::types::VerifyReport`]
///    returns `has_stale() == true`.
///
/// `McpUnreachable` and `not_configured` are deliberately silent here: they
/// aren't actionable enough to displace the codemap freshness banner that
/// shares this slot. Failures during reading/verifying are swallowed —
/// SessionStart should never block on a misconfigured integration.
///
/// Takes the already-loaded [`Config`](crate::config::Config) by reference
/// rather than reloading from disk, so the SessionStart hook only parses
/// `config.toml` once per fire.
pub(crate) fn build_integrations_session_start_banner(
    cas_root: &Path,
    config: &crate::config::Config,
) -> Option<String> {
    let opt_in = config
        .integrations
        .as_ref()
        .map(|i| i.session_start_warn)
        .unwrap_or(false);
    if !opt_in {
        return None;
    }
    let repo_root = cas_root.parent()?;
    let reports = crate::cli::integrate::doctor::collect_reports(repo_root);
    let body = crate::cli::integrate::doctor::session_start_banner_text(&reports, true)?;
    let safe_body = escape_xml_text(&body);
    Some(format!(
        "<integrations-freshness severity=\"info\">\n{safe_body}\n</integrations-freshness>"
    ))
}

/// Minimal XML-text escape so a recorded platform ID containing `<`, `>`,
/// `&`, `"`, or `'` cannot mis-close the wrapper tag (or inject an
/// attribute into the opening tag). Used only for SessionStart banner
/// bodies whose content is platform-supplied via SKILL.md keep blocks.
fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

// ── Worker worktree assertion (cas-bea2, LAYER 3) ─────────────────────────

/// Prepend a critical warning to `context` if the session cwd of an isolated
/// factory worker is:
/// - outside the assigned worktree (`CAS_CLONE_PATH`), OR
/// - NOT on a `factory/<name>` branch (allowlist semantics: main, master,
///   staging, epic/*, arbitrary branches, and detached HEAD are all denied —
///   fail-closed).
///
/// Only fires when `CAS_AGENT_ROLE=worker` AND `CAS_CLONE_PATH` is set.
/// Non-factory sessions and non-isolated workers are silent pass-through.
/// Best-effort: git failures and absent env vars are treated as "no mismatch".
pub(crate) fn build_worker_worktree_assertion(cwd: &str, context: String) -> String {
    let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
    if !role.eq_ignore_ascii_case("worker") {
        return context;
    }
    let clone_path = match std::env::var("CAS_CLONE_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => return context,
    };

    let mut warnings: Vec<String> = Vec::new();

    // Check 1: session cwd is inside the assigned worktree
    let cwd_path = std::path::Path::new(cwd);
    let worktree_path = std::path::Path::new(&clone_path);
    if !cwd_path.starts_with(worktree_path) {
        warnings.push(format!(
            "⚠️  CWD MISMATCH: Session cwd ({cwd}) is outside your assigned worktree \
            ({clone_path}).\n   Run: cd {clone_path}"
        ));
    }

    // Check 2: HEAD must be on a factory/<name> branch (allowlist).
    // Detached HEAD is also warned — fail-closed.
    let worker_name =
        std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "<worker-name>".to_string());
    let branch_result = std::process::Command::new("git")
        .args(["-C", cwd, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    match branch_result {
        None => {
            warnings.push(format!(
                "⚠️  DETACHED HEAD: Cannot determine current branch — DO NOT commit here!\n   \
                Switch first: git switch factory/{worker_name}"
            ));
        }
        Some(ref branch) if !branch.starts_with("factory/") => {
            warnings.push(format!(
                "⚠️  WRONG BRANCH: HEAD is on '{branch}' — DO NOT commit here!\n   \
                Workers may only commit on factory/<name>. Switch first:\n   \
                git switch factory/{worker_name}"
            ));
        }
        Some(_) => {} // factory/* — allowed
    }

    if warnings.is_empty() {
        return context;
    }

    let banner = format!(
        "<worker-worktree-alert severity=\"critical\">\n{}\n</worker-worktree-alert>",
        warnings.join("\n\n")
    );

    // Prepend: critical alerts must appear before other context
    if context.is_empty() {
        banner
    } else {
        format!("{banner}\n{context}")
    }
}

/// Compute session outcome based on metrics and friction events
///
/// Outcome determination priority:
/// Handle SessionEnd hook - generate session summary and mark for extraction
pub fn handle_session_end(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    let mut stores = HookStores::new(cas_root);

    // Get observations from this session
    let entry_store = stores.entries()?;
    let entries = entry_store.list()?;
    let session_observations: Vec<_> = entries
        .iter()
        .filter(|e| e.session_id.as_deref() == Some(&input.session_id))
        .collect();

    let session_count = session_observations.len();
    let supervisor_actionable_idle_minutes = if std::env::var("CAS_AGENT_ROLE")
        .ok()
        .as_deref()
        == Some("supervisor")
    {
        std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .and_then(|session| {
                crate::ui::factory::supervisor_progress_from_session_metadata_named(&session)
            })
            .map(|(_, tracker)| tracker.actionable_idle_minutes_at(chrono::Utc::now()))
    } else {
        None
    };
    if let Some(minutes) = supervisor_actionable_idle_minutes {
        eprintln!("cas: Supervisor actionable-idle minutes: {minutes}");
    }

    // Clean up agent leases and reset task status - ALWAYS do this regardless of observation count
    cleanup_agent_leases(cas_root, &input.session_id);

    // A default reminder is a one-shot thought attached to this exact session.
    // Cancel it synchronously at SessionEnd so a later session never receives
    // stale monitoring instructions. Explicit cross-session reminders remain
    // pending and disclose their origin and creation time on delivery.
    if let Ok(reminders) = crate::store::open_reminder_store(cas_root) {
        match reminders.cancel_pending_for_origin_session(&input.session_id) {
            Ok(cancelled) if cancelled > 0 => {
                eprintln!("cas: Cancelled {cancelled} session-scoped reminder(s)");
            }
            Ok(_) => {}
            Err(error) => eprintln!("cas: Failed to cancel session reminders: {error}"),
        }
    }

    // Factory session hygiene (task cas-a9ab): append a durable manifest of
    // the main worktree's uncommitted state so the next supervisor can see
    // what was left behind if this session died mid-task. Best-effort —
    // never let hygiene logging break session-end.
    {
        let agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
        if let Some(path) = crate::hooks::handlers::session_hygiene::write_session_end_manifest(
            cas_root,
            &input.session_id,
            agent_name.as_deref(),
            agent_role.as_deref(),
        ) {
            eprintln!("cas: Wrote session-end manifest to {}", path.display());
        }
        if let Err(error) =
            crate::hooks::handlers::session_hygiene::write_current_state_snapshot(cas_root)
        {
            eprintln!("cas: Failed to write current-state snapshot: {error}");
        }
    }

    // Notify daemon via socket that session ended
    {
        use crate::agent_id::get_cc_pid_for_hook;
        use crate::mcp::socket::{DaemonEvent, send_event};
        let cc_pid = get_cc_pid_for_hook();
        let event = DaemonEvent::SessionEnd {
            session_id: input.session_id.clone(),
            cc_pid: Some(cc_pid),
        };
        if send_event(cas_root, &event).is_ok() {
            eprintln!("cas: Notified daemon of session end");
        }
    }

    // Clean up current_session file
    let _ = std::fs::remove_file(cas_root.join("current_session"));

    // Clean up session files used for context boosting
    clear_session_files(cas_root);

    // Clean up OTEL context file
    let _ = OtelContext::remove(cas_root);

    // Clean up verifier marker file (safety cleanup in case subagent didn't clean up)
    let _ = std::fs::remove_file(cas_root.join(".verifier_unjail_marker"));

    if session_count == 0 {
        eprintln!(
            "cas: Session {} ended (no observations)",
            &input.session_id[..8.min(input.session_id.len())]
        );
        return Ok(HookOutput::empty());
    }

    // Log session end
    eprintln!(
        "cas: Session {} ended with {} observations",
        &input.session_id[..8.min(input.session_id.len())],
        session_count
    );

    // Check if AI features are enabled
    let config = Config::load(cas_root).unwrap_or_default();
    let should_summarize = config
        .hooks
        .as_ref()
        .map(|h| h.generate_summaries)
        .unwrap_or(false);

    // Generate session title and compute outcome (reuses single SqliteStore)
    if let Some(sqlite_store) = stores.sqlite() {
        match generate_session_title_sync(&session_observations) {
            Ok(title) => {
                if sqlite_store
                    .update_session_title(&input.session_id, &title)
                    .is_ok()
                {
                    eprintln!("cas: Session title: {title}");
                }
            }
            Err(e) => {
                eprintln!("cas: Title generation failed: {e}");
            }
        }

        // Compute session outcome
        let session_opt = sqlite_store.get_session(&input.session_id).ok().flatten();

        let outcome = if let Some(session) = session_opt {
            if session.tasks_closed > 0 {
                cas_types::SessionOutcome::TasksCompleted
            } else if session.entries_created > 0 {
                cas_types::SessionOutcome::LearningsCreated
            } else if session.tool_uses > 0 {
                cas_types::SessionOutcome::Exploration
            } else {
                cas_types::SessionOutcome::Abandoned
            }
        } else if session_count > 0 {
            cas_types::SessionOutcome::Exploration
        } else {
            cas_types::SessionOutcome::Abandoned
        };

        if sqlite_store
            .update_session_signals(&input.session_id, Some(outcome), None, None)
            .is_ok()
        {
            eprintln!("cas: Session outcome: {outcome}");
        }
    }

    if should_summarize {
        // Generate summary
        let entry_store = stores.entries()?;
        {
            if let Ok(summary) = generate_session_summary_sync(&session_observations) {
                // Store the summary as a context entry
                if !summary.summary.is_empty() {
                    let id = entry_store.generate_id()?;
                    let mut content = format!("## Session Summary\n\n{}\n", summary.summary);

                    if let Some(minutes) = supervisor_actionable_idle_minutes {
                        content.push_str(&format!(
                            "\n### Factory Forward Motion\n- Actionable-idle minutes: {minutes}\n"
                        ));
                    }

                    if !summary.decisions.is_empty() {
                        content.push_str("\n### Decisions\n");
                        for decision in &summary.decisions {
                            content.push_str(&format!("- {decision}\n"));
                        }
                    }

                    if !summary.key_learnings.is_empty() {
                        content.push_str("\n### Learnings\n");
                        for learning in &summary.key_learnings {
                            content.push_str(&format!("- {learning}\n"));
                        }
                    }

                    if !summary.follow_up_tasks.is_empty() {
                        content.push_str("\n### Follow-up Tasks\n");
                        for task in &summary.follow_up_tasks {
                            content.push_str(&format!("- {task}\n"));
                        }
                    }

                    let entry = Entry {
                        id: id.clone(),
                        entry_type: EntryType::Context,
                        content,
                        tags: vec!["session-summary".to_string()],
                        session_id: Some(input.session_id.clone()),
                        ..Default::default()
                    };

                    if entry_store.add(&entry).is_ok() {
                        eprintln!("cas: Generated session summary: {id}");
                    }
                }
            }
        }
    }

    Ok(HookOutput::empty())
}

#[cfg(test)]
mod session_end_reminder_tests {
    use super::*;
    use crate::store::{init_cas_dir, open_reminder_store};
    use cas_store::{KnowledgeStore, SqliteKnowledgeStore};

    #[test]
    fn session_end_cancels_default_reminders_but_not_cross_session_opt_in() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(project.path()).unwrap();
        let reminders = open_reminder_store(&cas_root).unwrap();
        for cross_session in [false, true] {
            reminders
                .create_with_scope(
                    "session-end-owner",
                    None,
                    "must not become stale",
                    cas_store::ReminderTriggerType::Time,
                    Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
                    None,
                    None,
                    3600,
                    Some("factory-a"),
                    Some("session-ending"),
                    cross_session,
                    None,
                )
                .unwrap();
        }

        handle_session_end(
            &HookInput {
                session_id: "session-ending".to_string(),
                cwd: project.path().to_string_lossy().into_owned(),
                hook_event_name: "SessionEnd".to_string(),
                ..HookInput::default()
            },
            Some(&cas_root),
        )
        .unwrap();

        let pending = reminders.list_pending("session-end-owner").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].cross_session);
        assert!(
            SqliteKnowledgeStore::open(&cas_root)
                .unwrap()
                .get_page_by_rel_path("current-state.md")
                .unwrap()
                .is_some(),
            "SessionEnd must write the deterministic current-state snapshot"
        );
    }
}

/// Generate session summary using AI (synchronous wrapper with timeout)
pub(crate) fn generate_session_summary_sync(
    observations: &[&Entry],
) -> Result<SessionSummary, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 5 second timeout to prevent blocking the hook for too long
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            generate_session_summary_async(observations),
        )
        .await
        .map_err(|_| MemError::Other("AI summary generation timed out after 5s".to_string()))?
    })
}

/// Generate session summary using AI
async fn generate_session_summary_async(
    observations: &[&Entry],
) -> Result<SessionSummary, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    // Build prompt from observations
    let obs_text: String = observations
        .iter()
        .take(50) // Limit to prevent token overflow
        .map(|e| {
            format!(
                "- [{}] {}",
                e.source_tool.as_deref().unwrap_or("?"),
                e.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_text = format!(
        r#"Analyze these observations from a coding session and generate a structured summary.

## Observations
{obs_text}

## Task
Generate a JSON summary with:
- summary: 1-2 sentence overview of what was accomplished
- decisions: Array of key decisions made (architectural, design, etc.)
- tasks_completed: Array of tasks that were finished
- key_learnings: Array of important discoveries or patterns learned
- follow_up_tasks: Array of suggested next tasks

Respond with JSON only, no markdown:
{{"summary": "...", "decisions": [...], "tasks_completed": [...], "key_learnings": [...], "follow_up_tasks": [...]}}"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_summary",
    )
    .await
    .map_err(|e| MemError::Other(format!("AI summary failed: {e}")))?;

    let response_text = result.text();

    // Parse JSON response
    let json_str = response_text
        .find('{')
        .and_then(|start| {
            response_text
                .rfind('}')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or(response_text);

    serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("Failed to parse summary: {e}")))
}

/// Generate session title (synchronous wrapper with timeout)
pub fn generate_session_title_sync(observations: &[&Entry]) -> Result<String, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 15 second timeout - claude CLI spawn can take a few seconds
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(15),
            generate_session_title_async(observations),
        )
        .await
        .map_err(|_| MemError::Other("Title generation timed out after 15s".to_string()))?
    })
}

/// Generate a concise session title using AI
async fn generate_session_title_async(observations: &[&Entry]) -> Result<String, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    if observations.is_empty() {
        return Ok("Empty session".to_string());
    }

    // Build a brief summary of what happened
    let obs_text: String = observations
        .iter()
        .take(20) // Limit to key observations
        .map(|e| {
            let tool = e.source_tool.as_deref().unwrap_or("?");
            let content = truncate_display(&e.content, 100);
            format!("- [{tool}] {content}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_text = format!(
        r#"Generate a 5-8 word title summarizing this coding session.

## Session Activity
{obs_text}

## Examples of good titles:
- "Implemented user authentication flow"
- "Fixed payment processing bug"
- "Refactored database queries for performance"
- "Added dark mode support"
- "Set up CI/CD pipeline"

Respond with ONLY the title, no quotes or punctuation at the end."#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_title",
    )
    .await
    .map_err(|e| MemError::Other(format!("Title generation failed: {e}")))?;

    let title = result.text().trim().to_string();

    // Clean up the title - remove quotes if present
    let title = title.trim_matches('"').trim_matches('\'').to_string();

    // Ensure reasonable length
    if title.chars().count() > 100 {
        Ok(title.chars().take(100).collect())
    } else if title.is_empty() {
        Ok("Coding session".to_string())
    } else {
        Ok(title)
    }
}

/// Extract learnings from transcript (synchronous wrapper with timeout)
pub(crate) fn extract_learnings_sync(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<ExtractedLearning>, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 5 second timeout to prevent blocking the hook for too long
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            extract_learnings_async(transcript_path, file_paths),
        )
        .await
        .map_err(|_| MemError::Other("Learning extraction timed out after 5s".to_string()))?
    })
}

/// Extract learnings from transcript using AI
///
/// Reads the transcript, sends to Haiku to identify project conventions
/// that the user taught Claude during the session.
async fn extract_learnings_async(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<ExtractedLearning>, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    // Read the transcript file
    let transcript = std::fs::read_to_string(transcript_path)
        .map_err(|e| MemError::Other(format!("Failed to read transcript: {e}")))?;

    // Skip if transcript is too short (likely no meaningful interaction)
    if transcript.len() < 500 {
        return Ok(vec![]);
    }

    // Truncate transcript if too long (keep last 50k chars - most recent context)
    // Find a valid char boundary to avoid slicing in the middle of multi-byte UTF-8 chars
    let transcript_excerpt = if transcript.len() > 50000 {
        let mut start = transcript.len() - 50000;
        // Walk forward to find a valid UTF-8 char boundary
        while start < transcript.len() && !transcript.is_char_boundary(start) {
            start += 1;
        }
        &transcript[start..]
    } else {
        &transcript
    };

    // Build file context from observed paths
    let file_context = if file_paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Files Modified This Session\n{}",
            file_paths
                .iter()
                .take(20)
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt_text = format!(
        r#"Analyze this Claude Code session transcript and extract project-specific rules or conventions that the USER TAUGHT Claude.

## What to Look For
- User corrections: "No, don't do X, instead do Y"
- User preferences: "Always use X pattern", "Never import from Y"
- API corrections: "That function doesn't exist, use Z instead"
- Framework conventions: "In this project we use X for Y"
- Style rules: "We don't use useEffect here", "Always use generated types"

## What to IGNORE
- General programming knowledge (not project-specific)
- Claude's own discoveries without user confirmation
- One-off fixes that aren't conventions
- Debugging steps

## Transcript
{transcript_excerpt}
{file_context}

## Task
Extract 0-5 project-specific rules the user taught. For each, include:
- content: The rule in imperative form ("Use X", "Never Y", "Always Z")
- path_pattern: Glob pattern for files this applies to (e.g., "**/*.tsx", "lib/**/*.ex") or null if global
- confidence: 0.7-1.0 based on how explicit the user was
- tags: Relevant tags like ["react", "elixir", "testing"]

Respond with JSON array only, no markdown:
[{{"content": "...", "path_pattern": "...", "confidence": 0.9, "tags": ["..."]}}]

If no clear learnings found, respond with: []"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "learning_extraction",
    )
    .await
    .map_err(|e| MemError::Other(format!("Learning extraction failed: {e}")))?;

    let response_text = result.text();

    // Parse JSON response
    let json_str = response_text
        .find('[')
        .and_then(|start| {
            response_text
                .rfind(']')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or("[]");

    let learnings: Vec<ExtractedLearning> = serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("Failed to parse learnings: {e}")))?;

    // Filter out low-confidence learnings
    Ok(learnings
        .into_iter()
        .filter(|l| l.confidence >= 0.7)
        .collect())
}

// ─── session-learn: 7-signal memory classifier (cas-6156 / EPIC cas-ebea) ─────

const SESSION_LEARN_SKILL_BODY: &str = include_str!("../../builtins/skills/session-learn/SKILL.md");

fn build_session_learn_prompt(transcript_excerpt: &str, file_context: &str) -> String {
    format!(
        "{SESSION_LEARN_SKILL_BODY}\n\n## Transcript\n{transcript_excerpt}{file_context}\n\nReturn only the JSON array, no prose, no markdown wrapper."
    )
}

/// Run the session-learn 7-signal classifier against the transcript.
///
/// Synchronous wrapper — creates a `tokio::Runtime`, calls `session_learn_async`
/// with a 30-second timeout (longer than `extract_learnings_sync` because the
/// 7-signal prompt is richer), and returns the draft list.
///
/// Callers in `stop_flow.rs` apply the confidence gate and overlap-detection
/// (`find_similar_entry`) before writing survivors to the store.
pub(crate) fn session_learn_sync(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<SessionLearnDraft>, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            session_learn_async(transcript_path, file_paths),
        )
        .await
        .map_err(|_| MemError::Other("session-learn timed out after 30s".to_string()))?
    })
}

/// Async implementation — reads transcript, builds the 7-signal prompt, calls
/// Haiku, and parses the returned JSON array into `Vec<SessionLearnDraft>`.
async fn session_learn_async(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<SessionLearnDraft>, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    let transcript = std::fs::read_to_string(transcript_path)
        .map_err(|e| MemError::Other(format!("session-learn: cannot read transcript: {e}")))?;

    // Skip trivial transcripts — same guard the SKILL.md documents
    if transcript.len() < 500 {
        return Ok(vec![]);
    }

    // Keep the most-recent 50 k chars (valid UTF-8 boundary)
    let transcript_excerpt = if transcript.len() > 50_000 {
        let mut start = transcript.len() - 50_000;
        while start < transcript.len() && !transcript.is_char_boundary(start) {
            start += 1;
        }
        &transcript[start..]
    } else {
        &transcript
    };

    let file_context = if file_paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Files Modified This Session\n{}",
            file_paths
                .iter()
                .take(20)
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt_text = build_session_learn_prompt(transcript_excerpt, &file_context);

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_learn",
    )
    .await
    .map_err(|e| MemError::Other(format!("session-learn LLM call failed: {e}")))?;

    let response_text = result.text();

    // Extract JSON array from the response
    let json_str = response_text
        .find('[')
        .and_then(|start| {
            response_text
                .rfind(']')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or("[]");

    let drafts: Vec<SessionLearnDraft> = serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("session-learn: failed to parse drafts: {e}")))?;

    Ok(drafts)
}

#[cfg(test)]
mod session_learn_tests {
    use super::*;

    #[test]
    fn session_learn_prompt_starts_with_the_canonical_skill_body() {
        let prompt = build_session_learn_prompt("transcript excerpt", "");
        assert!(
            prompt.starts_with(SESSION_LEARN_SKILL_BODY),
            "Stop hook classifier prompt must use the embedded session-learn skill body"
        );
        assert!(
            prompt.contains("## Transcript\ntranscript excerpt"),
            "dynamic transcript must be appended after the canonical skill body"
        );
    }

    /// Confirm `SessionLearnDraft` round-trips through JSON correctly.
    /// This exercises the serde mapping without a live LLM.
    #[test]
    fn session_learn_draft_deserializes_from_json() {
        let json = r#"[
          {
            "signal": "correction",
            "entry_type": "preference",
            "scope": "global",
            "tags": ["correction", "scope-discipline"],
            "content": "When a worker flags a real gap, amend the AC rather than working around it.",
            "confidence": 0.9,
            "dedup_hits": []
          },
          {
            "signal": "pattern",
            "entry_type": "learning",
            "scope": "project",
            "tags": ["pattern", "git"],
            "content": "Single-commit branches self-cert through the verification gate; multi-commit stacks hit jail.",
            "confidence": 0.85,
            "dedup_hits": [],
            "notes": "Confirmed by cas-8edb"
          }
        ]"#;

        let drafts: Vec<SessionLearnDraft> =
            serde_json::from_str(json).expect("draft JSON must parse");
        assert_eq!(drafts.len(), 2);

        let correction = &drafts[0];
        assert_eq!(correction.signal, "correction");
        assert_eq!(correction.entry_type, "preference");
        assert_eq!(correction.scope, "global");
        assert!((correction.confidence - 0.9).abs() < f32::EPSILON);
        assert!(correction.dedup_hits.is_empty());
        assert!(correction.notes.is_none());

        let pattern = &drafts[1];
        assert_eq!(pattern.signal, "pattern");
        assert_eq!(pattern.notes.as_deref(), Some("Confirmed by cas-8edb"));
    }

    /// Empty-array response is valid and must not error.
    #[test]
    fn session_learn_draft_accepts_empty_array() {
        let drafts: Vec<SessionLearnDraft> =
            serde_json::from_str("[]").expect("empty array must parse");
        assert!(drafts.is_empty());
    }

    /// `session_learn_sync` on a too-short transcript must return Ok([]) without
    /// attempting an LLM call (the < 500 byte guard in session_learn_async).
    /// We verify this by pointing at a real temp file with tiny content.
    #[test]
    fn session_learn_sync_skips_trivial_transcript() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        writeln!(tmp, "short").expect("write");
        let path = tmp.path().to_str().unwrap().to_string();

        let result = session_learn_sync(&path, &[]);
        assert!(
            result.is_ok(),
            "trivial transcript must return Ok, not Err: {result:?}"
        );
        assert!(
            result.unwrap().is_empty(),
            "trivial transcript must return empty draft list"
        );
    }
}

// ── Worker worktree assertion tests (cas-bea2, LAYER 3) ───────────────────
#[cfg(test)]
mod worker_worktree_assertion_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    fn make_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "t@t.com"],
            vec!["config", "user.name", "T"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        std::fs::write(p.join("r.txt"), "r").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        tmp
    }

    /// Non-worker role → pass-through (no banner)
    #[test]
    fn non_worker_passes_through() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_CLONE_PATH", Some("/tmp/some-worktree")),
        ]);
        let ctx = "some context".to_string();
        let result = build_worker_worktree_assertion("/tmp/other", ctx.clone());
        assert_eq!(result, ctx, "supervisor must not be warned");
    }

    /// Worker with no CAS_CLONE_PATH → pass-through (not isolated)
    #[test]
    fn no_clone_path_passes_through() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", None),
        ]);
        let ctx = "some context".to_string();
        let result = build_worker_worktree_assertion("/tmp/foo", ctx.clone());
        assert_eq!(result, ctx);
    }

    /// CWD outside worktree → warning prepended
    #[test]
    fn cwd_outside_worktree_prepends_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt").to_string_lossy().to_string();
        let other = tmp.path().join("other").to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&worktree)),
        ]);

        let result = build_worker_worktree_assertion(&other, String::new());
        assert!(
            result.contains("CWD MISMATCH"),
            "expected CWD MISMATCH warning: {result}"
        );
        assert!(
            result.contains("worker-worktree-alert"),
            "expected XML wrapper: {result}"
        );
    }

    /// CWD inside worktree on a non-factory branch (e.g. main) → branch warning prepended
    #[test]
    fn non_factory_branch_prepends_warning() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&p)),
        ]);

        let result = build_worker_worktree_assertion(&p, String::new());
        assert!(
            result.contains("WRONG BRANCH"),
            "expected WRONG BRANCH warning for 'main': {result}"
        );
        assert!(
            result.contains("main"),
            "expected branch name 'main' in warning: {result}"
        );
    }

    /// CWD inside worktree on an epic branch → branch warning prepended
    /// (Regression guard: epic/* used to bypass the denylist.)
    #[test]
    fn epic_branch_prepends_warning() {
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "epic/cas-073f"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let result = build_worker_worktree_assertion(&ps, String::new());
        assert!(
            result.contains("WRONG BRANCH"),
            "expected WRONG BRANCH warning for epic branch: {result}"
        );
        assert!(
            result.contains("epic/cas-073f"),
            "expected branch name in warning: {result}"
        );
    }

    /// CWD inside worktree on detached HEAD → branch warning prepended (fail-closed)
    #[test]
    fn detached_head_prepends_warning() {
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
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let result = build_worker_worktree_assertion(&ps, String::new());
        assert!(
            result.contains("DETACHED HEAD"),
            "expected DETACHED HEAD warning: {result}"
        );
    }

    /// CWD inside worktree on factory branch → no warning
    #[test]
    fn factory_branch_is_clean() {
        let tmp = make_git_repo();
        let p = tmp.path();

        std::process::Command::new("git")
            .args(["checkout", "-b", "factory/test-w"])
            .current_dir(p)
            .output()
            .unwrap();

        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&ps)),
        ]);

        let original_ctx = "existing context".to_string();
        let result = build_worker_worktree_assertion(&ps, original_ctx.clone());
        assert_eq!(
            result, original_ctx,
            "no warning on factory branch, got: {result}"
        );
    }

    /// Existing context is preserved (warning is prepended, not replacing)
    #[test]
    fn warning_prepends_not_replaces_context() {
        let tmp = make_git_repo(); // on main
        let p = tmp.path().to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(&p)),
        ]);

        let original = "## Important context\nDo this first.".to_string();
        let result = build_worker_worktree_assertion(&p, original.clone());
        assert!(
            result.contains("## Important context"),
            "original context must be preserved"
        );
        assert!(
            result.starts_with("<worker-worktree-alert"),
            "alert must be prepended"
        );
    }

    /// SessionStart bundle size must stay under 12KB after adding the assertion
    #[test]
    fn worker_session_start_with_assertion_stays_under_12kb() {
        // Simulate the largest plausible warning: both cwd mismatch + wrong branch (non-factory)
        let tmp = make_git_repo();
        let p = tmp.path();
        let wt = "/some/very/long/absolute/path/to/worktrees/worker-name";
        let ps = p.to_string_lossy().to_string();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_CLONE_PATH", Some(wt)),
            ("CAS_AGENT_NAME", Some("some-worker")),
        ]);

        // Simulate a near-12KB context (just below 12KB)
        let large_ctx = "x".repeat(11_000);
        let result = build_worker_worktree_assertion(&ps, large_ctx);
        assert!(
            result.len() < 12_288,
            "bundle with assertion must stay under 12KB, got {} bytes",
            result.len()
        );
    }
}
