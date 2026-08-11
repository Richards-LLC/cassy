use crate::hooks::handlers::handlers_middle::capture_transcript_for_prompts;
use crate::hooks::handlers::handlers_middle::session_stop::{
    build_duplicate_detection_context, build_learning_review_context, build_rule_review_context,
    build_session_summary_context, handle_loop_iteration, synthesize_buffered_observations,
};
use crate::hooks::handlers::handlers_middle::utils::{
    find_similar_entry, find_similar_rule, is_architectural_file, truncate_list, truncate_str,
};
use crate::hooks::handlers::*;

pub fn handle_stop(input: &HookInput, cas_root: Option<&Path>) -> Result<HookOutput, MemError> {
    let timer = TraceTimer::new();

    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // cas-f3e3: Claude Code sets `stop_hook_active: true` when the session is
    // ALREADY continuing because a previous Stop hook returned
    // decision: "block". The harness's documented loop-prevention contract is
    // that a Stop hook must not block again in that state — a blocker the model
    // cannot clear by continuing (e.g. draft rules it will not review) would
    // otherwise re-block on every stop, forever. CAS blocks Stop in five places
    // below and, until this change, declared and read the field nowhere, so it
    // had no brake at all.
    //
    // Honest scope note: no live loop was ever reproduced. This wires the
    // documented brake; it is not a fix for a measured incident.
    let stop_is_reentrant = input.stop_hook_is_reentrant();

    // Check for exit blockers (open tasks, active children) before allowing stop
    // Skip for factory agents (Worker, Supervisor, Director) - supervisor manages task reassignment
    let config = Config::load(cas_root).unwrap_or_default();
    if config.block_exit_on_open() && !stop_is_reentrant {
        // Check if this is a factory agent
        let is_factory_agent = if let Ok(agent_store) = open_agent_store(cas_root) {
            let agent_id = current_agent_id(input);
            agent_store
                .get(&agent_id)
                .ok()
                .map(|a| {
                    matches!(
                        a.role,
                        AgentRole::Worker | AgentRole::Supervisor | AgentRole::Director
                    )
                })
                .unwrap_or(false)
        } else {
            false
        };

        // Factory agents can always exit - supervisor handles task reassignment
        if !is_factory_agent {
            match get_exit_blockers(cas_root, &input.session_id) {
                Ok(blockers) if blockers.has_blockers() => {
                    // Use decision: "block" to prevent Claude from stopping
                    // This tells Claude to continue working on the remaining tasks
                    return Ok(HookOutput::block_stop(blockers.format_message()));
                }
                Ok(_) => {} // No blockers, continue with stop
                Err(e) => {
                    // Log error but don't block stop on failure to check
                    eprintln!("cas: Warning: Failed to check exit blockers: {e}");
                }
            }
        }
    }

    // Check for active loop - if present, handle iteration
    if let Ok(loop_store) = open_loop_store(cas_root) {
        if let Ok(Some(mut active_loop)) = loop_store.get_active_for_session(&input.session_id) {
            return handle_loop_iteration(input, cas_root, loop_store, &mut active_loop);
        }
    }

    let store = open_store(cas_root)?;

    // Get observations for title generation before ending session
    let session_entries = store.list_by_session(&input.session_id)?;
    let session_obs_for_title: Vec<&Entry> = session_entries.iter().take(20).collect();

    // End session for analytics and generate title
    if let Ok(sqlite_store) = SqliteStore::open(cas_root) {
        // Generate title before ending (if not already set)
        if let Ok(Some(session)) = sqlite_store.get_session(&input.session_id) {
            if session.title.is_none() && !session_obs_for_title.is_empty() {
                match generate_session_title_sync(&session_obs_for_title) {
                    Ok(title) => {
                        let _ = sqlite_store.update_session_title(&input.session_id, &title);
                        eprintln!("cas: Session title: {title}");
                    }
                    Err(e) => {
                        eprintln!("cas: Title generation failed: {e}");
                    }
                }
            }
        }

        if sqlite_store.end_session(&input.session_id).is_ok() {
            // Get session duration for logging
            if let Ok(Some(session)) = sqlite_store.get_session(&input.session_id) {
                if let Some(duration) = session.duration_secs {
                    let mins = duration / 60;
                    let secs = duration % 60;
                    eprintln!(
                        "cas: Session {} ended ({}m {}s)",
                        &input.session_id[..8.min(input.session_id.len())],
                        mins,
                        secs
                    );
                }
            }
        }
    }

    // NOTE: Agent cleanup is deferred until after all blocking checks pass.
    // See end of function for cleanup_agent_leases and related cleanup.

    // === BLAME V2: Capture conversation transcript for prompts ===
    // This enables rich context in `cas blame --verbose` by storing
    // the full conversation (minus tool results) with each prompt.
    capture_transcript_for_prompts(cas_root, input);

    // Limit processing to prevent slow hooks on very active sessions
    const MAX_OBSERVATIONS: usize = 50;
    let session_observations: Vec<&Entry> = session_entries.iter().take(MAX_OBSERVATIONS).collect();
    let total_obs_count = session_entries.len();
    let obs_count = session_observations.len();

    // Load config to check settings
    let config = Config::load(cas_root).unwrap_or_default();
    #[allow(unused_variables)]
    let generate_summaries = config
        .hooks
        .as_ref()
        .map(|h| h.generate_summaries)
        .unwrap_or(false);

    // Build summary content from observations using structured extraction
    let mut summary_parts = Vec::new();

    // Categorize observations by tool and extract patterns
    let mut writes: Vec<&str> = Vec::new();
    let mut edits: Vec<&str> = Vec::new();
    let mut bash_cmds: Vec<&str> = Vec::new();
    let mut reads: Vec<&str> = Vec::new();
    let mut architectural_changes: Vec<String> = Vec::new();
    let mut unresolved_issues: Vec<String> = Vec::new();

    for obs in &session_observations {
        let content = obs.content.as_str();
        if content.starts_with("Write:") {
            let file = content.strip_prefix("Write: ").unwrap_or(content);
            writes.push(file);
            // Detect architectural files
            if is_architectural_file(file) {
                architectural_changes.push(format!("Created: {file}"));
            }
        } else if content.starts_with("Edit:") {
            let file = content.strip_prefix("Edit: ").unwrap_or(content);
            edits.push(file);
            if is_architectural_file(file) {
                architectural_changes.push(format!("Modified: {file}"));
            }
        } else if content.starts_with("Bash:") {
            let cmd = content.strip_prefix("Bash: ").unwrap_or(content);
            bash_cmds.push(cmd);
            // Detect failed tests or builds as unresolved issues
            if cmd.contains("cargo test") || cmd.contains("npm test") || cmd.contains("pytest") {
                // Check if there was a test-related failure captured
                if obs
                    .tags
                    .iter()
                    .any(|t| t.contains("fail") || t.contains("error"))
                {
                    unresolved_issues.push(format!("Tests may need attention: {cmd}"));
                }
            }
        } else if content.starts_with("Read:") {
            reads.push(content.strip_prefix("Read: ").unwrap_or(content));
        } else if content.contains("TODO") || content.contains("FIXME") || content.contains("HACK")
        {
            unresolved_issues.push(content.to_string());
        }
    }

    // Build structured summary
    if !writes.is_empty() {
        summary_parts.push(format!(
            "**Files Created ({}):** {}",
            writes.len(),
            truncate_list(&writes, 5)
        ));
    }
    if !edits.is_empty() {
        summary_parts.push(format!(
            "**Files Modified ({}):** {}",
            edits.len(),
            truncate_list(&edits, 5)
        ));
    }
    if !bash_cmds.is_empty() {
        summary_parts.push(format!(
            "**Commands Run ({}):** {}",
            bash_cmds.len(),
            truncate_list(&bash_cmds, 5)
        ));
    }
    if !reads.is_empty() && reads.len() <= 10 {
        summary_parts.push(format!(
            "**Files Explored ({}):** {}",
            reads.len(),
            truncate_list(&reads, 5)
        ));
    }

    // Add architectural changes section
    if !architectural_changes.is_empty() {
        summary_parts.push(String::new()); // blank line
        summary_parts.push("**Architectural Changes:**".to_string());
        for change in architectural_changes.iter().take(5) {
            summary_parts.push(format!("- {change}"));
        }
    }

    // Add unresolved issues section (important for future context!)
    if !unresolved_issues.is_empty() {
        summary_parts.push(String::new());
        summary_parts.push("**Unresolved Issues:**".to_string());
        for issue in unresolved_issues.iter().take(5) {
            summary_parts.push(format!("- {}", truncate_str(issue, 100)));
        }
    }

    // Store session summary if we have observations
    if obs_count > 0 {
        let session_short = &input.session_id[..8.min(input.session_id.len())];

        // Check if this session already has summaries to avoid duplicates
        // session_entries already contains only this session's entries
        let has_activity_summary = session_entries
            .iter()
            .any(|e| e.tags.iter().any(|t| t == "session-activity"));
        let has_ai_summary = session_entries
            .iter()
            .any(|e| e.tags.iter().any(|t| t == "session-summary"));

        // Only generate AI-powered summary if enabled and no existing summary
        if generate_summaries && obs_count >= 3 && !has_ai_summary {
            if let Ok(ai_summary) = generate_session_summary_sync(&session_observations) {
                // Store AI-generated summary
                let id = store.generate_id()?;
                let mut content = format!(
                    "## Session {} Summary\n\n{}\n",
                    session_short, ai_summary.summary
                );

                if !ai_summary.decisions.is_empty() {
                    content.push_str("\n### Key Decisions\n");
                    for decision in &ai_summary.decisions {
                        content.push_str(&format!("- {decision}\n"));
                    }
                }

                if !ai_summary.tasks_completed.is_empty() {
                    content.push_str("\n### Completed\n");
                    for task in &ai_summary.tasks_completed {
                        content.push_str(&format!("- {task}\n"));
                    }
                }

                if !ai_summary.key_learnings.is_empty() {
                    content.push_str("\n### Learnings\n");
                    for learning in &ai_summary.key_learnings {
                        content.push_str(&format!("- {learning}\n"));
                    }
                }

                let entry = Entry {
                    id: id.clone(),
                    entry_type: EntryType::Context,
                    content,
                    tags: vec!["session-summary".to_string(), "ai-generated".to_string()],
                    session_id: Some(input.session_id.clone()),
                    importance: 0.7, // Session summaries are valuable
                    ..Default::default()
                };

                if store.add(&entry).is_ok() {
                    // Index the summary
                    let index_dir = cas_root.join("index/tantivy");
                    if let Ok(search) = SearchIndex::open(&index_dir) {
                        let _ = search.index_entry(&entry);
                    }
                    eprintln!("cas: Generated AI session summary: {id}");
                }
            }
        }

        // Extract learnings from transcript if available
        if generate_summaries {
            if let Some(ref transcript_path) = input.transcript_path {
                // Collect file paths from session observations for context
                let file_paths: Vec<String> = session_observations
                    .iter()
                    .filter_map(|e| {
                        let content = &e.content;
                        if content.starts_with("Write: ") || content.starts_with("Edit: ") {
                            content.split(": ").nth(1).map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();

                match extract_learnings_sync(transcript_path, &file_paths) {
                    Ok(learnings) if !learnings.is_empty() => {
                        eprintln!(
                            "cas: Extracted {} learnings from transcript",
                            learnings.len()
                        );

                        // Open rule store for adding rules
                        if let Ok(rule_store) = open_rule_store(cas_root) {
                            for learning in learnings {
                                // Check for duplicate rules using BM25 search
                                if find_similar_rule(cas_root, &learning.content) {
                                    eprintln!(
                                        "cas: Skipping similar rule: {}",
                                        truncate_str(&learning.content, 50)
                                    );
                                    continue;
                                }

                                // Create a new rule from the learning
                                let rule_id = match rule_store.generate_id() {
                                    Ok(id) => id,
                                    Err(_) => continue,
                                };

                                let mut rule = Rule::new(rule_id.clone(), learning.content.clone());
                                rule.paths = learning.path_pattern.unwrap_or_default();
                                rule.tags = learning.tags;
                                // Rule starts as Draft - needs user validation via mcp__cas__rule action=helpful

                                if rule_store.add(&rule).is_ok() {
                                    eprintln!(
                                        "cas: Created rule {}: {}",
                                        rule_id,
                                        truncate_str(&learning.content, 60)
                                    );
                                }
                            }
                        }
                    }
                    Ok(_) => {
                        // No learnings found, that's fine
                    }
                    Err(e) => {
                        eprintln!("cas: Learning extraction failed: {e}");
                    }
                }
            }
        }

        // session-learn: 7-signal memory classifier (cas-6156 / EPIC cas-ebea)
        // Gated on [memory] session_learn_auto = true in .cas/config.toml.
        // The obs_count >= 5 guard mirrors the SKILL.md "< 5 tool calls = skip"
        // floor so we never pay a Haiku call on a trivial session.
        let session_learn_auto = config
            .memory
            .as_ref()
            .is_some_and(|m| m.session_learn_auto);

        if session_learn_auto && obs_count >= 5 {
            if let Some(ref transcript_path) = input.transcript_path {
                let sl_file_paths: Vec<String> = session_observations
                    .iter()
                    .filter_map(|e| {
                        let content = &e.content;
                        if content.starts_with("Write: ") || content.starts_with("Edit: ") {
                            content.split(": ").nth(1).map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();

                match session_learn_sync(transcript_path, &sl_file_paths) {
                    Ok(drafts) if !drafts.is_empty() => {
                        eprintln!(
                            "cas: session-learn: {} draft(s) from transcript",
                            drafts.len()
                        );

                        let confidence_floor = |d: &SessionLearnDraft| {
                            if d.signal == "correction" {
                                d.confidence >= 0.5
                            } else {
                                d.confidence >= 0.6
                            }
                        };

                        let mut stored = 0usize;
                        for draft in drafts.iter().filter(|d| {
                            confidence_floor(d) && d.dedup_hits.is_empty()
                        }) {
                            // BM25 overlap-detection gate
                            if find_similar_entry(cas_root, &draft.content) {
                                eprintln!(
                                    "cas: session-learn: skipping near-duplicate: {}",
                                    truncate_str(&draft.content, 50)
                                );
                                continue;
                            }

                            let entry_type = draft
                                .entry_type
                                .parse::<EntryType>()
                                .unwrap_or(EntryType::Learning);
                            let scope = if draft.scope.eq_ignore_ascii_case("global") {
                                crate::types::Scope::Global
                            } else {
                                crate::types::Scope::Project
                            };

                            let id = match store.generate_id() {
                                Ok(id) => id,
                                Err(_) => continue,
                            };

                            let entry = Entry {
                                id: id.clone(),
                                entry_type,
                                scope,
                                content: draft.content.clone(),
                                tags: draft.tags.clone(),
                                session_id: Some(input.session_id.clone()),
                                importance: draft.confidence,
                                ..Default::default()
                            };

                            if store.add(&entry).is_ok() {
                                let index_dir = cas_root.join("index/tantivy");
                                if let Ok(search) = SearchIndex::open(&index_dir) {
                                    let _ = search.index_entry(&entry);
                                }
                                stored += 1;
                                eprintln!(
                                    "cas: session-learn: stored {} [{}] {}",
                                    id,
                                    draft.signal,
                                    truncate_str(&draft.content, 60)
                                );
                            }
                        }

                        if stored > 0 {
                            eprintln!("cas: session-learn: {stored} memory entries written");
                        }
                    }
                    Ok(_) => {
                        // No drafts — trivial session or no signal-worthy findings
                    }
                    Err(e) => {
                        eprintln!("cas: session-learn failed: {e}");
                    }
                }
            }
        }

        // Store a basic activity summary if we don't have one yet
        if !summary_parts.is_empty() && !has_activity_summary {
            let id = store.generate_id()?;
            let content = format!(
                "## Session {} Activity\n\n{}\n\nTotal observations: {}",
                session_short,
                summary_parts.join("\n"),
                obs_count
            );

            let entry = Entry {
                id: id.clone(),
                entry_type: EntryType::Context,
                content,
                tags: vec!["session-activity".to_string(), "compacted".to_string()],
                session_id: Some(input.session_id.clone()),
                importance: 0.5, // Activity summaries have moderate importance
                ..Default::default()
            };

            if store.add(&entry).is_ok() {
                let index_dir = cas_root.join("index/tantivy");
                if let Ok(search) = SearchIndex::open(&index_dir) {
                    let _ = search.index_entry(&entry);
                }
            }
        }

        // Archive raw observations after creating summary (they're now redundant)
        // Keep only the last few for debugging
        let obs_to_archive: Vec<_> = session_observations
            .iter()
            .filter(|e| e.entry_type == EntryType::Observation)
            .skip(5) // Keep the 5 most recent
            .collect();

        for obs in obs_to_archive {
            let _ = store.archive(&obs.id);
        }

        let archived_count = session_entries
            .iter()
            .filter(|e| e.entry_type == EntryType::Observation)
            .count()
            .saturating_sub(5);
        if total_obs_count > MAX_OBSERVATIONS {
            eprintln!(
                "cas: Session {session_short} complete ({obs_count}/{total_obs_count} observations processed, archived {archived_count})"
            );
        } else {
            eprintln!(
                "cas: Session {session_short} complete ({total_obs_count} observations, archived {archived_count})"
            );
        }
    }

    // === SYNTHESIZE BUFFERED OBSERVATIONS ===
    // Process observations that were buffered during the session
    let mut buffer_count = 0;
    if let Some(tracer) = DevTracer::get() {
        if let Ok(buffered) = tracer.get_buffered_observations_for_session(&input.session_id) {
            buffer_count = buffered.len();

            if !buffered.is_empty() {
                // Synthesize buffered observations into learnings
                let synthesis_result =
                    synthesize_buffered_observations(cas_root, &buffered, &input.session_id);

                match synthesis_result {
                    Ok(learnings_created) => {
                        if learnings_created > 0 {
                            eprintln!(
                                "cas: Synthesized {learnings_created} learnings from {buffer_count} buffered observations"
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("cas: Buffer synthesis error: {e}");
                    }
                }

                // Clear the buffer after processing
                let _ = tracer.clear_observation_buffer_for_session(&input.session_id);
            }
        }
    }

    // Record trace if dev mode is enabled
    if let Some(tracer) = DevTracer::get() {
        if tracer.should_trace_hooks() {
            let input_json = serde_json::json!({
                "session_id": input.session_id,
                "cwd": input.cwd,
            });
            let output_json = serde_json::json!({
                "observations_count": obs_count,
                "buffer_count": buffer_count,
            });

            let _ = tracer.record_hook(
                "Stop",
                &input_json,
                &output_json,
                None,
                None,
                timer.elapsed_ms(),
                true,
                None,
            );
        }
    }

    // Check for factory worker role - workers skip maintenance agents
    // Maintenance tasks (learning-reviewer, rule-reviewer, duplicate-detector) should run
    // on supervisor only. Workers should focus on their assigned tasks.
    let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
        .map(|role| role.to_lowercase() == "worker")
        .unwrap_or(false);

    // Only trigger maintenance agents for non-worker agents.
    //
    // cas-f3e3: also skipped when `stop_hook_active` is true. All four blockers
    // below ask the agent to spawn a maintenance subagent; if it declines, the
    // condition still holds at the next Stop and the block repeats. These are
    // exactly the non-self-clearing blockers the harness's loop-prevention
    // signal exists for.
    if !is_factory_worker && !stop_is_reentrant {
        // Check for unreviewed learnings that need review before stop
        if let Some(review_context) = build_learning_review_context(store.as_ref(), &config) {
            eprintln!("cas: Blocking stop - unreviewed learnings need review");
            return Ok(HookOutput::block_stop_with_context(
                "You have unreviewed learnings that should be analyzed. Please spawn a learning-reviewer subagent to process them before stopping.".to_string(),
                review_context,
            ));
        }

        // Check for draft rules that need review before stop
        if let Ok(rule_store) = open_rule_store(cas_root) {
            if let Some(review_context) = build_rule_review_context(rule_store.as_ref(), &config) {
                eprintln!("cas: Blocking stop - draft rules need review");
                return Ok(HookOutput::block_stop_with_context(
                    "You have draft rules that should be reviewed. Please spawn a rule-reviewer subagent to process them before stopping.".to_string(),
                    review_context,
                ));
            }
        }

        // Check for potential duplicates that need cleanup before stop
        if let Some(cleanup_context) = build_duplicate_detection_context(store.as_ref(), &config) {
            eprintln!("cas: Blocking stop - duplicate detection recommended");
            return Ok(HookOutput::block_stop_with_context(
                "You have accumulated many entries that may contain duplicates. Please spawn a duplicate-detector subagent to consolidate them before stopping.".to_string(),
                cleanup_context,
            ));
        }

        // Check if session summary generation is enabled
        if let Some(summary_context) =
            build_session_summary_context(store.as_ref(), &config, &input.session_id)
        {
            eprintln!("cas: Blocking stop - session summary required");
            return Ok(HookOutput::block_stop_with_context(
                "Session summary generation is enabled. Please spawn a session-summarizer subagent to create a summary before stopping.".to_string(),
                summary_context,
            ));
        }
    }

    // Best-effort codemap reminder
    let codemap_reminder =
        crate::hooks::handlers::handlers_events::codemap_stop_reminder(cas_root);
    if let Some(ref reminder) = codemap_reminder {
        eprintln!("cas: {reminder}");
    }

    // cas-5c0a (B3): Emit final git-state event for factory workers so the
    // supervisor can see branch/HEAD/pushed-ref/PR without manual forensics.
    // Fires on the normal-stop path (i.e. not on blocked loop-iteration returns).
    if is_factory_worker {
        let worker_name = std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "unknown".to_string());
        let cwd_path = if input.cwd.is_empty() {
            std::path::Path::new(".")
        } else {
            std::path::Path::new(&input.cwd)
        };
        crate::hooks::handlers::handlers_middle::session_stop::emit_worker_final_git_state(
            cas_root,
            &worker_name,
            cwd_path,
            &input.session_id,
        );
    }

    // Only the normal, unblocked Stop path finalizes still-unreferenced cards.
    // Failures are intentionally swallowed inside the bounded helper.
    crate::ambient_recall::finalize_ambient_recall_feedback(input, cas_root);

    // Clean up session files
    // NOTE: Agent cleanup (graceful_shutdown) happens in SessionEnd, NOT here.
    // Stop hook can be blocked and the agent continues working, so we can't
    // mark agents as shutdown here - they'd disappear from sidecar while still active.
    // PID → session mapping is handled by daemon via SessionEnd socket event.
    clear_session_files(cas_root);
    let _ = std::fs::remove_file(cas_root.join(".verifier_unjail_marker"));

    // Return codemap reminder as context if present, otherwise empty.
    // Stop hooks don't support hookSpecificOutput.additionalContext in Claude
    // Code's schema — route via systemMessage instead.
    if let Some(reminder) = codemap_reminder {
        Ok(HookOutput::with_system_context(format!(
            "<system-reminder>{reminder}</system-reminder>"
        )))
    } else {
        Ok(HookOutput::empty())
    }
}
